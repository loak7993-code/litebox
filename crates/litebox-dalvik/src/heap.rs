//! Heap arena: objects live in a growable slot vector; free slots recycle with
//! a generation bump. Tracing compacting GC driven by an allocation budget.
//! Object identity is stable across snapshots (slots serialize as-is).

use crate::value::{ObjRef, Value};

pub const NULL_IDX: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayKind {
    /// boolean[], byte[], char[], short[], int[] packed as logical ints
    Ints,
    Longs,
    Floats,
    Doubles,
    /// Object[], String[], any reference array
    Refs,
    /// char[] semantic (stored as Ints but typed) — folded into Ints at use site
    Chars,
}

impl ArrayKind {
    pub fn from_descriptor(desc: &str) -> Option<ArrayKind> {
        // desc like "[I", "[Ljava/lang/String;", "[[[D", "[B"
        let last = desc.as_bytes().last().copied()?;
        match last {
            b'I' | b'Z' | b'B' | b'C' | b'S' => Some(ArrayKind::Ints),
            b'J' => Some(ArrayKind::Longs),
            b'F' => Some(ArrayKind::Floats),
            b'D' => Some(ArrayKind::Doubles),
            _ => Some(ArrayKind::Refs),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ObjectData {
    Instance {
        class: u32,
        fields: Box<[Value]>,
    },
    Array {
        kind: ArrayKind,
        data: Box<[Value]>,
    },
    Str(Box<str>),
    /// java/lang/Class instance referring to a runtime class id
    Class(u32),
}

impl ObjectData {
    pub fn type_name(&self) -> &'static str {
        match self {
            ObjectData::Instance { .. } => "instance",
            ObjectData::Array { .. } => "array",
            ObjectData::Str(_) => "string",
            ObjectData::Class(_) => "class",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HeapSlotMeta {
    pub gen: u32,
    pub size: u32,
}

#[derive(Debug, Default)]
pub struct Heap {
    pub(crate) slots: Vec<Option<ObjectData>>,
    pub(crate) gen: Vec<u32>,
    pub(crate) sizes: Vec<u32>,
    pub(crate) free: Vec<u32>,
    pub live_bytes: u64,
    pub alloc_count: u64,
    pub gc_count: u64,
    /// Exception stack traces, keyed by throwable object slot.
    pub exception_frames: std::collections::HashMap<u32, Vec<ExFrame>>,
}

#[derive(Debug, Clone)]
pub struct ExFrame {
    pub class_name: String,
    pub method_name: String,
    pub line: u32,
}

impl Heap {
    pub fn new() -> Self {
        Heap {
            slots: Vec::new(),
            gen: Vec::new(),
            sizes: Vec::new(),
            free: Vec::new(),
            live_bytes: 0,
            alloc_count: 0,
            gc_count: 0,
            exception_frames: std::collections::HashMap::new(),
        }
    }

    pub fn alloc(&mut self, data: ObjectData, size: u32) -> ObjRef {
        self.alloc_count += 1;
        self.live_bytes += size as u64;
        let (idx, gen) = if let Some(idx) = self.free.pop() {
            let g = self.gen[idx as usize].wrapping_add(1);
            self.gen[idx as usize] = g;
            self.sizes[idx as usize] = size;
            self.slots[idx as usize] = Some(data);
            (idx, g)
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Some(data));
            self.gen.push(0);
            self.sizes.push(size);
            (idx, 0)
        };
        ObjRef { idx, gen }
    }

    #[inline]
    pub fn get(&self, r: ObjRef) -> Option<&ObjectData> {
        if r.idx == NULL_IDX {
            return None;
        }
        match self.slots.get(r.idx as usize) {
            Some(Some(d)) if self.gen[r.idx as usize] == r.gen => Some(d),
            _ => None,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, r: ObjRef) -> Option<&mut ObjectData> {
        if r.idx == NULL_IDX {
            return None;
        }
        let g_ok = self.gen.get(r.idx as usize) == Some(&r.gen);
        if g_ok {
            self.slots.get_mut(r.idx as usize).and_then(|s| s.as_mut())
        } else {
            None
        }
    }

    pub fn is_live(&self, r: ObjRef) -> bool {
        r.idx != NULL_IDX
            && self.gen.get(r.idx as usize) == Some(&r.gen)
            && self.slots[r.idx as usize].is_some()
    }

    /// Free an object explicitly (used by compaction and by finalization of
    /// interned structures only; Java code never frees).
    pub fn release(&mut self, idx: u32) {
        if let Some(d) = self.slots[idx as usize].take() {
            self.live_bytes -= self.sizes[idx as usize] as u64;
            self.free.push(idx);
            let _ = d;
        }
    }

    /// Replace an object's content in place (identity preserved). Used by
    /// constructors of first-class-native types like String.
    pub fn replace(&mut self, r: ObjRef, data: ObjectData) -> bool {
        if r.idx == NULL_IDX {
            return false;
        }
        let g_ok = self.gen.get(r.idx as usize) == Some(&r.gen);
        if g_ok && self.slots[r.idx as usize].is_some() {
            self.live_bytes -= self.sizes[r.idx as usize] as u64;
            let size = match &data {
                ObjectData::Str(s) => (s.len() + 24) as u32,
                ObjectData::Instance { fields, .. } => 64 + fields.len() as u32 * 16,
                ObjectData::Array { data, .. } => (32 + data.len() * 16) as u32,
                ObjectData::Class(_) => 48,
            };
            self.live_bytes += size as u64;
            self.sizes[r.idx as usize] = size;
            self.slots[r.idx as usize] = Some(data);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Mark-and-sweep free-list GC (arena stays address-stable, no moves, so
    /// refs never invalidate; simpler and snapshot-friendly).
    /// `roots` yields every reference-typed Value the VM can reach directly.
    pub fn collect<I: FnMut(&mut dyn FnMut(ObjRef))>(&mut self, mut roots: I) {
        self.gc_count += 1;
        let n = self.slots.len();
        let mut mark = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        let n32 = n as u32;
        fn push(stack: &mut Vec<u32>, n: u32, r: ObjRef) {
            if r.idx != NULL_IDX && r.idx < n {
                stack.push(r.idx);
            }
        }
        roots(&mut |r| push(&mut stack, n32, r));
        while let Some(idx) = stack.pop() {
            if mark[idx as usize] {
                continue;
            }
            mark[idx as usize] = true;
            if let Some(Some(d)) = self.slots.get(idx as usize) {
                match d {
                    ObjectData::Instance { fields, .. } => {
                        for f in fields.iter() {
                            if let Value::R(r) = f {
                                push(&mut stack, n32, *r);
                            }
                        }
                    }
                    ObjectData::Array { data, .. } => {
                        for v in data.iter() {
                            if let Value::R(r) = v {
                                push(&mut stack, n32, *r);
                            }
                        }
                    }
                    ObjectData::Str(_) | ObjectData::Class(_) => {}
                }
            }
        }
        let mut freed: Vec<u32> = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_some() && !mark[i] {
                self.live_bytes -= self.sizes[i] as u64;
                freed.push(i as u32);
            }
        }
        for idx in &freed {
            self.slots[*idx as usize] = None;
        }
        // keep the arena dense-ish: free list LIFO keeps hot slots reused
        self.free.extend(freed);
        self.exception_frames.retain(|k, _| {
            *k < n as u32 && self.slots[*k as usize].is_some()
        });
    }

    /// Direct iteration for snapshotting.
    pub fn iter_slots(&self) -> impl Iterator<Item = (u32, &ObjectData)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|d| (i as u32, d)))
    }

    pub fn restore(&mut self, slots: Vec<(u32, ObjectData, u32)>, gens: Vec<u32>) {
        let n = slots.len();
        let mut rebuilt: Vec<Option<ObjectData>> = vec![None; n];
        for (idx, data, size) in slots {
            rebuilt[idx as usize] = Some(data);
            if self.sizes.len() <= idx as usize {
                self.sizes.resize(idx as usize + 1, 0);
            }
            self.sizes[idx as usize] = size;
            self.live_bytes += size as u64;
        }
        self.slots = rebuilt;
        self.gen = gens;
        self.free = (0..n as u32).filter(|&i| self.slots[i as usize].is_none()).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::NULL;

    #[test]
    fn alloc_and_read() {
        let mut h = Heap::new();
        let r = h.alloc(ObjectData::Str("hi".into()), 16);
        match h.get(r) {
            Some(ObjectData::Str(s)) => assert_eq!(&**s, "hi"),
            other => panic!("{other:?}"),
        }
        let nul = NULL.as_ref();
        assert!(h.get(nul).is_none());
    }

    #[test]
    fn generation_invalidates_stale_refs() {
        let mut h = Heap::new();
        let r1 = h.alloc(ObjectData::Str("a".into()), 8);
        h.release(r1.idx);
        let r2 = h.alloc(ObjectData::Str("b".into()), 8);
        assert_eq!(r2.idx, r1.idx);
        assert_ne!(r2.gen, r1.gen);
        assert!(h.get(r1).is_none(), "stale ref must not resolve");
        assert!(h.get(r2).is_some());
    }

    #[test]
    fn gc_frees_unreachable_keeps_reachable() {
        let mut h = Heap::new();
        let keep = h.alloc(ObjectData::Str("keep".into()), 8);
        let drop_me = h.alloc(ObjectData::Str("drop".into()), 8);
        let _ = drop_me;
        let arr = h.alloc(
            ObjectData::Array { kind: ArrayKind::Refs, data: vec![Value::R(keep)].into() },
            16,
        );
        h.collect(|visit| {
            visit(arr);
        });
        assert!(h.is_live(keep));
        assert_eq!(h.len(), 2); // arr + keep
        assert_eq!(h.gc_count, 1);
    }
}
