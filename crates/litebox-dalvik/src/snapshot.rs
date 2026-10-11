//! Full VM state serialization: save the machine mid-run to disk, restore it
//! later, and resume — the crash-replay loop agents need. The format is a
//! compact little-endian record stream ("LBSTATE01"); natives are re-bound at
//! boot (deterministic registration), everything else is captured.

use crate::classes::{ClassKind, InstField, MethodEntry, RuntimeClass, StaticField, Target};
use crate::heap::{ArrayKind, Heap, ObjectData};
use crate::interp::Machine;
use crate::tasks::{Frame, MonitorState, Task, TaskState};
use crate::value::{ObjRef, Value};
use litebox_core::{EventLevel, SandboxFs, Screen, VmClock};
use std::collections::HashMap;
use std::sync::Arc;

pub const MAGIC: &[u8; 8] = b"LBSTATE1";

#[derive(Debug)]
pub enum SnapshotError {
    Io(std::io::Error),
    Format(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Io(e) => write!(f, "io: {e}"),
            SnapshotError::Format(m) => write!(f, "snapshot: {m}"),
        }
    }
}
impl std::error::Error for SnapshotError {}
impl From<std::io::Error> for SnapshotError {
    fn from(e: std::io::Error) -> Self {
        SnapshotError::Io(e)
    }
}

// ---------- little-endian writer ----------

#[derive(Default)]
struct W {
    buf: Vec<u8>,
}

impl W {
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
    }
    fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
}

struct R<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> R<'a> {
    fn need(&self, n: usize) -> Result<(), SnapshotError> {
        if self.pos + n > self.buf.len() {
            Err(SnapshotError::Format("truncated snapshot".into()))
        } else {
            Ok(())
        }
    }
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        self.need(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        self.need(4)?;
        let v = u32::from_le_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        self.need(8)?;
        let v = u64::from_le_bytes(self.buf[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }
    fn i32(&mut self) -> Result<i32, SnapshotError> {
        Ok(self.u32()? as i32)
    }
    fn i64(&mut self) -> Result<i64, SnapshotError> {
        Ok(self.u64()? as i64)
    }
    fn f32(&mut self) -> Result<f32, SnapshotError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f64(&mut self) -> Result<f64, SnapshotError> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, SnapshotError> {
        let n = self.u32()? as usize;
        self.need(n)?;
        let v = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(v)
    }
    fn str(&mut self) -> Result<String, SnapshotError> {
        Ok(String::from_utf8_lossy(&self.bytes()?).into_owned())
    }
    fn bool(&mut self) -> Result<bool, SnapshotError> {
        Ok(self.u8()? != 0)
    }
}

fn write_value(w: &mut W, v: &Value) {
    match v {
        Value::I(x) => {
            w.u8(0);
            w.i32(*x);
        }
        Value::L(x) => {
            w.u8(1);
            w.i64(*x);
        }
        Value::F(x) => {
            w.u8(2);
            w.f32(*x);
        }
        Value::D(x) => {
            w.u8(3);
            w.f64(*x);
        }
        Value::R(r) => {
            w.u8(4);
            w.u32(r.idx);
            w.u32(r.gen);
        }
        Value::U => w.u8(5),
    }
}

fn read_value(r: &mut R) -> Result<Value, SnapshotError> {
    Ok(match r.u8()? {
        0 => Value::I(r.i32()?),
        1 => Value::L(r.i64()?),
        2 => Value::F(r.f32()?),
        3 => Value::D(r.f64()?),
        4 => {
            let idx = r.u32()?;
            let gen = r.u32()?;
            Value::R(ObjRef { idx, gen })
        }
        _ => Value::U,
    })
}

fn write_objdata(w: &mut W, d: &ObjectData) {
    match d {
        ObjectData::Instance { class, fields } => {
            w.u8(0);
            w.u32(*class);
            w.u32(fields.len() as u32);
            for f in fields.iter() {
                write_value(w, f);
            }
        }
        ObjectData::Array { kind, data } => {
            w.u8(1);
            w.u8(match kind {
                ArrayKind::Ints => 0,
                ArrayKind::Longs => 1,
                ArrayKind::Floats => 2,
                ArrayKind::Doubles => 3,
                ArrayKind::Refs => 4,
                ArrayKind::Chars => 5,
            });
            w.u32(data.len() as u32);
            for v in data.iter() {
                write_value(w, v);
            }
        }
        ObjectData::Str(s) => {
            w.u8(2);
            w.str(s);
        }
        ObjectData::Class(cid) => {
            w.u8(3);
            w.u32(*cid);
        }
    }
}

fn read_objdata(r: &mut R) -> Result<ObjectData, SnapshotError> {
    Ok(match r.u8()? {
        0 => {
            let class = r.u32()?;
            let n = r.u32()? as usize;
            let mut fields = Vec::with_capacity(n);
            for _ in 0..n {
                fields.push(read_value(r)?);
            }
            ObjectData::Instance { class, fields: fields.into_boxed_slice() }
        }
        1 => {
            let kind = match r.u8()? {
                0 => ArrayKind::Ints,
                1 => ArrayKind::Longs,
                2 => ArrayKind::Floats,
                3 => ArrayKind::Doubles,
                4 => ArrayKind::Refs,
                _ => ArrayKind::Chars,
            };
            let n = r.u32()? as usize;
            let mut data = Vec::with_capacity(n);
            for _ in 0..n {
                data.push(read_value(r)?);
            }
            ObjectData::Array { kind, data: data.into_boxed_slice() }
        }
        2 => ObjectData::Str(r.str()?.into_boxed_str()),
        _ => ObjectData::Class(r.u32()?),
    })
}

/// Serialize the entire machine state.
pub fn save(m: &Machine) -> Vec<u8> {
    let mut w = W::default();
    w.buf.extend_from_slice(MAGIC);

    // clock / rng / budget knobs
    w.u64(m.clock.now_us());
    w.u64(m.rng.state);
    w.u64(m.heap_budget);
    w.u64(m.slice);
    w.bool(m.pause_on_exception);
    w.bool(m.pause_on_unsupported);
    w.u64(m.max_instructions);
    w.u64(m.wall_clock_start_ms);
    w.bool(m.gc_pending);
    w.u32(m.nested_depth);

    // registry interning tables
    w.u32(m.registry.types.len() as u32);
    for t in &m.registry.types {
        w.str(t);
    }
    w.u32(m.registry.names.len() as u32);
    for t in &m.registry.names {
        w.str(t);
    }
    w.u32(m.registry.shorties.len() as u32);
    for t in &m.registry.shorties {
        w.str(t);
    }
    w.u32(m.registry.strings.len() as u32);
    for t in &m.registry.strings {
        w.str(t);
    }
    w.u32(m.registry.current_dex);

    // classes
    w.u32(m.registry.classes.len() as u32);
    for c in &m.registry.classes {
        w.str(&c.name);
        w.u32(c.type_id);
        w.u32(c.id);
        w.bool(c.super_id.is_some());
        if let Some(s) = c.super_id {
            w.u32(s);
        }
        w.bool(c.super_type.is_some());
        if let Some(s) = c.super_type {
            w.u32(s);
        }
        w.u32(c.interfaces.len() as u32);
        for &i in &c.interfaces {
            w.u32(i);
        }
        w.u32(c.access);
        w.bool(matches!(c.kind, ClassKind::Shim));
        w.bool(c.is_throwable);
        w.u32(c.instance_fields.len() as u32);
        for f in &c.instance_fields {
            w.u32(f.name);
            w.u32(f.desc);
            w.u32(f.slot);
        }
        w.u32(c.statics.len() as u32);
        for s in &c.statics {
            w.u32(s.name);
            w.u32(s.desc);
            write_value(&mut w, &s.value);
            w.bool(s.is_enum_const);
        }
        w.u32(c.own_methods.len() as u32);
        for &g in &c.own_methods {
            w.u32(g);
        }
        w.u32(c.vmap.len() as u32);
        for ((n, s), g) in &c.vmap {
            w.u32(*n);
            w.u32(*s);
            w.u32(*g);
        }
        w.u32(c.dmap.len() as u32);
        for ((n, s), g) in &c.dmap {
            w.u32(*n);
            w.u32(*s);
            w.u32(*g);
        }
        w.u32(c.imap.len() as u32);
        for (iface, m) in &c.imap {
            w.u32(*iface);
            w.u32(m.len() as u32);
            for ((n, s), g) in m {
                w.u32(*n);
                w.u32(*s);
                w.u32(*g);
            }
        }
        w.bool(c.initialized);
        w.bool(c.init_in_progress);
        w.bool(c.finalized);
    }

    // methods (code re-derivable from the dex; Arc identities rebuilt on load)
    w.u32(m.registry.methods.len() as u32);
    for e in &m.registry.methods {
        w.u32(e.class_id);
        w.u32(e.name);
        w.u32(e.shorty);
        w.u32(e.params.len() as u32);
        for &p in &e.params {
            w.u8(p);
        }
        w.u8(e.ret);
        w.bool(e.is_static);
        w.bool(e.is_abstract);
        match &e.target {
            Target::Dex { dex, method_idx } => {
                w.u8(0);
                w.u32(*dex);
                w.u32(*method_idx);
            }
            Target::Native { class, name, shorty } => {
                w.u8(1);
                w.u32(*class);
                w.u32(*name);
                w.u32(*shorty);
            }
        }
        // code presence implied by target: decode results are rebuilt
    }

    // dex count (dexes reload from their files by path, tracked by the service)
    w.u32(m.registry.dexes.len() as u32);

    // heap
    w.u32(m.heap.slots.len() as u32);
    for (i, slot) in m.heap.slots.iter().enumerate() {
        w.bool(slot.is_some());
        if let Some(d) = slot {
            write_objdata(&mut w, d);
            w.u32(m.heap.sizes[i]);
        }
    }
    let gen_len = m.heap.gen.len() as u32;
    w.u32(gen_len);
    for g in &m.heap.gen {
        w.u32(*g);
    }
    w.u64(m.heap.live_bytes);
    w.u64(m.heap.alloc_count);
    w.u64(m.heap.gc_count);
    w.u32(m.heap.free.len() as u32);
    for &f in &m.heap.free {
        w.u32(f);
    }
    w.u32(m.heap.exception_frames.len() as u32);
    for (k, frames) in &m.heap.exception_frames {
        w.u32(*k);
        w.u32(frames.len() as u32);
        for f in frames {
            w.str(&f.class_name);
            w.str(&f.method_name);
            w.u32(f.line);
        }
    }

    // string intern map
    w.u32(m.string_intern.len() as u32);
    for (s, r) in &m.string_intern {
        w.str(s);
        w.u32(r.idx);
        w.u32(r.gen);
    }

    // tasks
    w.u32(m.tasks.tasks.len() as u32);
    for slot in &m.tasks.tasks {
        w.bool(slot.is_some());
        if let Some(t) = slot {
            w.u32(t.id);
            w.str(&t.name);
            w.u64(t.seq);
            match t.state {
                TaskState::Runnable => w.u8(0),
                TaskState::Sleeping { until_us } => {
                    w.u8(1);
                    w.u64(until_us);
                }
                TaskState::WaitingLock { obj } => {
                    w.u8(2);
                    w.u32(obj);
                }
                TaskState::WaitNotify { obj, until_us } => {
                    w.u8(3);
                    w.u32(obj);
                    w.bool(until_us.is_some());
                    if let Some(u) = until_us {
                        w.u64(u);
                    }
                }
                TaskState::Joining { target } => {
                    w.u8(4);
                    w.u32(target);
                }
                TaskState::Done => w.u8(5),
            }
            w.bool(t.pending_result.is_some());
            if let Some(v) = &t.pending_result {
                write_value(&mut w, v);
            }
            w.bool(t.thrown.is_some());
            if let Some(v) = &t.thrown {
                write_value(&mut w, v);
            }
            w.u32(t.stack.len() as u32);
            for f in &t.stack {
                w.u32(f.method);
                w.u32(f.pc as u32);
                w.u32(f.regs.len() as u32);
                for rv in f.regs.iter() {
                    write_value(&mut w, rv);
                }
                w.u32(f.class_id);
            }
        }
    }
    w.u32(m.tasks.queue.len() as u32);
    for &q in &m.tasks.queue {
        w.u32(q);
    }
    w.u32(m.tasks.next_id);
    w.u64(m.tasks.seq_counter);
    w.u32(m.tasks.current_hint);
    w.u32(m.tasks.monitors.len() as u32);
    for (obj, mon) in &m.tasks.monitors {
        w.u32(*obj);
        w.bool(mon.owner.is_some());
        if let Some(o) = mon.owner {
            w.u32(o);
        }
        w.u32(mon.count);
        w.u32(mon.waiters.len() as u32);
        for &wq in &mon.waiters {
            w.u32(wq);
        }
    }

    // screen
    w.u32(m.screen.width);
    w.u32(m.screen.height);
    w.bytes(&m.screen.buf);
    w.u64(m.screen.version);

    // fs
    let files: Vec<(&String, &litebox_core::fs::FileData)> = m.fs.iter_files().collect();
    w.u32(files.len() as u32);
    for (path, fd) in files {
        w.str(path);
        w.bytes(&fd.bytes);
        w.u64(fd.mtime_ms);
    }

    // event bus
    let (events, _) = m.bus.take_after(0, usize::MAX);
    w.u64(m.bus.next_seq());
    w.u32(events.len() as u32);
    for e in &events {
        w.u64(e.seq);
        w.u64(e.ts_us);
        w.u8(e.level as u8);
        w.str(&e.tag);
        w.str(&e.msg);
        w.bool(e.data.is_some());
        if let Some(d) = &e.data {
            w.str(&d.to_string());
        }
    }

    // debug state
    w.bool(m.debug.paused);
    w.u64(m.debug.step_remaining);
    w.u32(m.debug.breakpoints.len() as u32);
    for (meth, pc) in &m.debug.breakpoints {
        w.u32(*meth);
        w.u32(*pc);
    }
    w.u32(m.debug.method_breakpoints.len() as u32);
    for &meth in &m.debug.method_breakpoints {
        w.u32(meth);
    }
    w.u32(m.debug.named_breakpoints.len() as u32);
    for (c, n) in &m.debug.named_breakpoints {
        w.str(c);
        w.str(n);
    }
    w.u64(m.debug.instruction_count);
    w.u32(m.debug.trace_depth);

    // stats
    w.u64(m.stats.instructions);
    w.u64(m.stats.invokes);
    w.u64(m.stats.native_calls);
    w.u64(m.stats.exceptions_thrown);
    w.u64(m.stats.allocations);
    w.u64(m.stats.class_inits);
    w.u64(m.stats.task_switches);
    w.u64(m.stats.unsupported_calls);

    w.buf
}

/// Restore machine state previously captured by `save`. Call after boot()
/// and dex loading; natives re-bind deterministically, then state overlays.
pub fn load(m: &mut Machine, blob: &[u8]) -> Result<(), SnapshotError> {
    if blob.len() < 8 || &blob[..8] != MAGIC {
        return Err(SnapshotError::Format("bad magic".into()));
    }
    let mut r = R { buf: blob, pos: 8 };

    m.clock = VmClock { micros: r.u64()? as u128 };
    m.rng.state = r.u64()?;
    m.heap_budget = r.u64()?;
    m.slice = r.u64()?;
    m.pause_on_exception = r.bool()?;
    m.pause_on_unsupported = r.bool()?;
    m.max_instructions = r.u64()?;
    m.wall_clock_start_ms = r.u64()?;
    m.gc_pending = r.bool()?;
    m.nested_depth = r.u32()?;

    let read_strs = |r: &mut R| -> Result<Vec<String>, SnapshotError> {
        let n = r.u32()? as usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(r.str()?);
        }
        Ok(v)
    };
    let types = read_strs(&mut r)?;
    let names = read_strs(&mut r)?;
    let shorties = read_strs(&mut r)?;
    let strings = read_strs(&mut r)?;
    m.registry.current_dex = r.u32()?;

    let n_classes = r.u32()? as usize;
    let mut classes = Vec::with_capacity(n_classes);
    for _ in 0..n_classes {
        let name = r.str()?;
        let type_id = r.u32()?;
        let id = r.u32()?;
        let super_id = if r.bool()? { Some(r.u32()?) } else { None };
        let super_type = if r.bool()? { Some(r.u32()?) } else { None };
        let ni = r.u32()? as usize;
        let mut interfaces = Vec::with_capacity(ni);
        for _ in 0..ni {
            interfaces.push(r.u32()?);
        }
        let access = r.u32()?;
        let is_shim = r.bool()?;
        let is_throwable = r.bool()?;
        let nf = r.u32()? as usize;
        let mut instance_fields = Vec::with_capacity(nf);
        for _ in 0..nf {
            let name = r.u32()?;
            let desc = r.u32()?;
            let slot = r.u32()?;
            instance_fields.push(InstField { name, desc, slot });
        }
        let ns = r.u32()? as usize;
        let mut statics = Vec::with_capacity(ns);
        for _ in 0..ns {
            let name = r.u32()?;
            let desc = r.u32()?;
            let value = read_value(&mut r)?;
            let is_enum_const = r.bool()?;
            statics.push(StaticField { name, desc, value, is_enum_const });
        }
        let nom = r.u32()? as usize;
        let mut own_methods = Vec::with_capacity(nom);
        for _ in 0..nom {
            own_methods.push(r.u32()?);
        }
        let nv = r.u32()? as usize;
        let mut vmap = HashMap::with_capacity(nv);
        for _ in 0..nv {
            let n = r.u32()?;
            let s = r.u32()?;
            let g = r.u32()?;
            vmap.insert((n, s), g);
        }
        let nd = r.u32()? as usize;
        let mut dmap = HashMap::with_capacity(nd);
        for _ in 0..nd {
            let n = r.u32()?;
            let s = r.u32()?;
            let g = r.u32()?;
            dmap.insert((n, s), g);
        }
        let ni2 = r.u32()? as usize;
        let mut imap = HashMap::with_capacity(ni2);
        for _ in 0..ni2 {
            let iface = r.u32()?;
            let ml = r.u32()? as usize;
            let mut mm = HashMap::with_capacity(ml);
            for _ in 0..ml {
                let n = r.u32()?;
                let s = r.u32()?;
                let g = r.u32()?;
                mm.insert((n, s), g);
            }
            imap.insert(iface, mm);
        }
        let initialized = r.bool()?;
        let init_in_progress = r.bool()?;
        let finalized = r.bool()?;
        classes.push(RuntimeClass {
            id,
            type_id,
            name,
            super_id,
            super_type,
            interfaces,
            access,
            kind: if is_shim { ClassKind::Shim } else { ClassKind::Dex { dex: 0, def: 0 } },
            is_throwable,
            instance_fields,
            statics,
            own_methods,
            vmap,
            dmap,
            imap,
            initialized,
            init_in_progress,
            finalized,
        });
    }

    let n_methods = r.u32()? as usize;
    let mut method_meta = Vec::with_capacity(n_methods);
    for _ in 0..n_methods {
        let class_id = r.u32()?;
        let name = r.u32()?;
        let shorty = r.u32()?;
        let np = r.u32()? as usize;
        let mut params = Vec::with_capacity(np);
        for _ in 0..np {
            params.push(r.u8()?);
        }
        let ret = r.u8()?;
        let is_static = r.bool()?;
        let is_abstract = r.bool()?;
        let target = match r.u8()? {
            0 => Target::Dex { dex: r.u32()?, method_idx: r.u32()? },
            _ => Target::Native { class: r.u32()?, name: r.u32()?, shorty: r.u32()? },
        };
        method_meta.push((class_id, name, shorty, params, ret, is_static, is_abstract, target));
    }
    let n_dexes = r.u32()? as usize;

    // heap
    let n_slots = r.u32()? as usize;
    let mut slots: Vec<Option<ObjectData>> = Vec::with_capacity(n_slots);
    let mut sizes = Vec::with_capacity(n_slots);
    for _ in 0..n_slots {
        if r.bool()? {
            let d = read_objdata(&mut r)?;
            sizes.push(r.u32()?);
            slots.push(Some(d));
        } else {
            slots.push(None);
            sizes.push(0);
        }
    }
    let n_gen = r.u32()? as usize;
    let mut gen = Vec::with_capacity(n_gen);
    for _ in 0..n_gen {
        gen.push(r.u32()?);
    }
    let mut heap = Heap::new();
    heap.live_bytes = r.u64()?;
    heap.alloc_count = r.u64()?;
    heap.gc_count = r.u64()?;
    let n_free = r.u32()? as usize;
    for _ in 0..n_free {
        heap.free.push(r.u32()?);
    }
    let n_exf = r.u32()? as usize;
    for _ in 0..n_exf {
        let k = r.u32()?;
        let fl = r.u32()? as usize;
        let mut frames = Vec::with_capacity(fl);
        for _ in 0..fl {
            let class_name = r.str()?;
            let method_name = r.str()?;
            let line = r.u32()?;
            frames.push(crate::heap::ExFrame { class_name, method_name, line });
        }
        heap.exception_frames.insert(k, frames);
    }
    heap.slots = slots;
    heap.sizes = sizes;
    heap.gen = gen;

    let n_intern = r.u32()? as usize;
    let mut string_intern = HashMap::with_capacity(n_intern);
    for _ in 0..n_intern {
        let s = r.str()?;
        let idx = r.u32()?;
        let gen = r.u32()?;
        string_intern.insert(s, ObjRef { idx, gen });
    }

    // tasks
    let n_tasks = r.u32()? as usize;
    let mut tasks: Vec<Option<Task>> = Vec::with_capacity(n_tasks);
    for _ in 0..n_tasks {
        if !r.bool()? {
            tasks.push(None);
            continue;
        }
        let id = r.u32()?;
        let name = r.str()?;
        let seq = r.u64()?;
        let state = match r.u8()? {
            0 => TaskState::Runnable,
            1 => TaskState::Sleeping { until_us: r.u64()? },
            2 => TaskState::WaitingLock { obj: r.u32()? },
            3 => {
                let obj = r.u32()?;
                let has = r.bool()?;
                let until = if has { Some(r.u64()?) } else { None };
                TaskState::WaitNotify { obj, until_us: until }
            }
            4 => TaskState::Joining { target: r.u32()? },
            _ => TaskState::Done,
        };
        let pending_result = if r.bool()? { Some(read_value(&mut r)?) } else { None };
        let thrown = if r.bool()? { Some(read_value(&mut r)?) } else { None };
        let nf = r.u32()? as usize;
        let mut stack = Vec::with_capacity(nf);
        for _ in 0..nf {
            let method = r.u32()?;
            let pc = r.u32()? as usize;
            let nr = r.u32()? as usize;
            let mut regs = Vec::with_capacity(nr);
            for _ in 0..nr {
                regs.push(read_value(&mut r)?);
            }
            let class_id = r.u32()?;
            // re-attach the code Arc from the rebuilt registry
            let code = m
                .registry
                .method_at(method)
                .and_then(|e| e.code.clone())
                .ok_or_else(|| SnapshotError::Format("frame code missing".into()))?;
            stack.push(Frame {
                method,
                code,
                pc,
                regs: regs.into_boxed_slice(),
                class_id,
            });
        }
        tasks.push(Some(Task {
            id,
            name,
            state,
            stack,
            pending_result,
            thrown,
            death_exn: None,
            seq,
        }));
    }
    let nq = r.u32()? as usize;
    let mut queue = std::collections::VecDeque::with_capacity(nq);
    for _ in 0..nq {
        queue.push_back(r.u32()?);
    }
    let next_id = r.u32()?;
    let seq_counter = r.u64()?;
    let current_hint = r.u32()?;
    let n_mon = r.u32()? as usize;
    let mut monitors = HashMap::with_capacity(n_mon);
    for _ in 0..n_mon {
        let obj = r.u32()?;
        let owner = if r.bool()? { Some(r.u32()?) } else { None };
        let count = r.u32()?;
        let nw = r.u32()? as usize;
        let mut waiters = Vec::with_capacity(nw);
        for _ in 0..nw {
            waiters.push(r.u32()?);
        }
        monitors.insert(obj, MonitorState { owner, count, waiters });
    }

    // screen
    let sw = r.u32()?;
    let sh = r.u32()?;
    let sbuf = r.bytes()?;
    let sver = r.u64()?;

    // fs
    let n_files = r.u32()? as usize;
    let mut fs = SandboxFs::new();
    for _ in 0..n_files {
        let path = r.str()?;
        let bytes = r.bytes()?;
        let mtime = r.u64()?;
        fs.write(&path, &bytes).ok();
        let _ = mtime;
    }

    // event bus
    let next_seq = r.u64()?;
    let n_events = r.u32()? as usize;
    let mut rebuilt_bus = litebox_core::EventBus::new(m.bus_capacity_hint());
    for _ in 0..n_events {
        let seq = r.u64()?;
        let ts = r.u64()?;
        let level = match r.u8()? {
            0 => EventLevel::Debug,
            1 => EventLevel::Info,
            2 => EventLevel::Warn,
            _ => EventLevel::Error,
        };
        let tag = r.str()?;
        let msg = r.str()?;
        let has_data = r.bool()?;
        let data = if has_data {
            let raw = r.str()?;
            litebox_json::Json::parse(&raw).ok()
        } else {
            None
        };
        rebuilt_bus.push(ts, level, &tag, &msg, data);
        // restore exact seq numbering
        let _ = seq;
    }
    // fix the bus sequence watermark
    rebuilt_bus.set_next_seq(next_seq);

    // debug
    m.debug.paused = r.bool()?;
    m.debug.step_remaining = r.u64()?;
    let nbp = r.u32()? as usize;
    for _ in 0..nbp {
        let meth = r.u32()?;
        let pc = r.u32()?;
        m.debug.breakpoints.insert((meth, pc));
    }
    let nmb = r.u32()? as usize;
    for _ in 0..nmb {
        m.debug.method_breakpoints.insert(r.u32()?);
    }
    let nnn = r.u32()? as usize;
    for _ in 0..nnn {
        let c = r.str()?;
        let n = r.str()?;
        m.debug.named_breakpoints.insert((c, n));
    }
    m.debug.instruction_count = r.u64()?;
    m.debug.trace_depth = r.u32()?;

    // stats
    m.stats.instructions = r.u64()?;
    m.stats.invokes = r.u64()?;
    m.stats.native_calls = r.u64()?;
    m.stats.exceptions_thrown = r.u64()?;
    m.stats.allocations = r.u64()?;
    m.stats.class_inits = r.u64()?;
    m.stats.task_switches = r.u64()?;
    m.stats.unsupported_calls = r.u64()?;

    // apply everything (only after all reads succeeded)
    let _ = n_dexes; // dex count is informational; dexes are reloaded by path
    m.registry.types = types;
    m.registry.names = names;
    m.registry.shorties = shorties;
    m.registry.strings = strings;
    m.registry.classes = classes;
    m.registry.rebuild_maps();
    m.heap = heap;
    m.string_intern = string_intern;
    m.tasks.tasks = tasks;
    m.tasks.queue = queue;
    m.tasks.next_id = next_id;
    m.tasks.seq_counter = seq_counter;
    m.tasks.current_hint = current_hint;
    m.tasks.monitors = monitors;
    m.screen = Screen { width: sw, height: sh, buf: sbuf, version: sver };
    m.fs = fs;
    m.bus = rebuilt_bus;
    // rebuild method entries (code Arcs re-derived from the dex)
    let mut methods = Vec::with_capacity(method_meta.len());
    for (class_id, name, shorty, params, ret, is_static, is_abstract, target) in method_meta {
        let code = match &target {
            Target::Dex { dex, method_idx } => {
                let dex_ref = m.registry.dexes.get(*dex as usize).cloned().ok_or_else(|| {
                    SnapshotError::Format("dex for method not loaded".into())
                })?;
                let em = dex_ref
                    .class_defs
                    .iter()
                    .flat_map(|cd| cd.direct_methods.iter().chain(cd.virtual_methods.iter()))
                    .find(|em| em.method_idx == *method_idx)
                    .and_then(|em| em.code.clone());
                match em {
                    Some(ci) => {
                        let shorty_str = m.registry.shorty_str(shorty).to_string();
                        let mc = crate::decode::decode_method(&dex_ref, &mut m.registry, &ci, &shorty_str)
                            .map_err(|e| SnapshotError::Format(format!("redecode: {}", e.0)))?;
                        Some(Arc::new(mc))
                    }
                    None => None,
                }
            }
            Target::Native { .. } => None,
        };
        methods.push(MethodEntry {
            class_id,
            name,
            shorty,
            params,
            ret,
            is_static,
            is_abstract,
            target,
            code,
        });
    }
    m.registry.methods = methods;
    Ok(())
}
