//! VM value representation. Tagged 16-byte union; refs are (slot, generation)
//! handles into the heap arena so GC can move objects without dangling
//! references: stale refs are caught by generation check, never by address.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjRef {
    pub idx: u32,
    pub gen: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    I(i32),
    L(i64),
    F(f32),
    D(f64),
    R(ObjRef),
    /// Uninitialized register slot (never observable by correct bytecode).
    U,
}

impl Value {
    pub fn null() -> Value {
        Value::R(ObjRef { idx: u32::MAX, gen: 0 })
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::R(r) if r.idx == u32::MAX)
    }
    pub fn as_i32(&self) -> i32 {
        match self {
            Value::I(v) => *v,
            Value::L(v) => *v as i32,
            Value::F(v) => *v as i32,
            Value::D(v) => *v as i32,
            Value::R(_) | Value::U => 0,
        }
    }
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::I(v) => *v as i64,
            Value::L(v) => *v,
            Value::F(v) => *v as i64,
            Value::D(v) => *v as i64,
            Value::R(_) | Value::U => 0,
        }
    }
    pub fn as_f32(&self) -> f32 {
        match self {
            Value::I(v) => *v as f32,
            Value::L(v) => *v as f32,
            Value::F(v) => *v,
            Value::D(v) => *v as f32,
            Value::R(_) | Value::U => 0.0,
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            Value::I(v) => *v as f64,
            Value::L(v) => *v as f64,
            Value::F(v) => *v as f64,
            Value::D(v) => *v,
            Value::R(_) | Value::U => 0.0,
        }
    }
    pub fn as_ref(&self) -> ObjRef {
        match self {
            Value::R(r) => *r,
            _ => ObjRef { idx: u32::MAX, gen: 0 },
        }
    }
    /// Truthiness per Dalvik: only the low 8 bits of the value matter for
    /// if-*z instructions on ints; references test for null.
    pub fn truthy(&self) -> bool {
        match self {
            Value::I(v) => *v != 0,
            Value::L(v) => *v != 0,
            Value::F(v) => *v != 0.0,
            Value::D(v) => *v != 0.0,
            Value::R(r) => r.idx != u32::MAX,
            Value::U => false,
        }
    }
}

pub const NULL: Value = Value::R(ObjRef { idx: u32::MAX, gen: 0 });

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimKind {
    I,
    J,
    F,
    D,
    Z,
    B,
    C,
    S,
    Ref,
}

impl PrimKind {
    /// Map a field/element descriptor to storage kind.
    pub fn from_descriptor(desc: &str) -> PrimKind {
        match desc.as_bytes().first() {
            Some(b'Z') | Some(b'B') | Some(b'C') | Some(b'S') | Some(b'I') => PrimKind::I,
            Some(b'J') => PrimKind::J,
            Some(b'F') => PrimKind::F,
            Some(b'D') => PrimKind::D,
            _ => PrimKind::Ref,
        }
    }
    pub fn is_wide(self) -> bool {
        matches!(self, PrimKind::J | PrimKind::D)
    }
    /// shorty char
    pub fn shorty(self) -> char {
        match self {
            PrimKind::I => 'I',
            PrimKind::J => 'J',
            PrimKind::F => 'F',
            PrimKind::D => 'D',
            PrimKind::Z => 'Z',
            PrimKind::B => 'B',
            PrimKind::C => 'C',
            PrimKind::S => 'S',
            PrimKind::Ref => 'L',
        }
    }
}

/// Default value for a slot of a given kind (zero/null).
pub fn zero_of(kind: PrimKind) -> Value {
    match kind {
        PrimKind::J => Value::L(0),
        PrimKind::F => Value::F(0.0),
        PrimKind::D => Value::D(0.0),
        PrimKind::Ref => NULL,
        _ => Value::I(0),
    }
}

/// Coerce an int-kind value into the exact width semantics of a descriptor.
pub fn narrow(desc_byte: u8, v: Value) -> Value {
    match desc_byte {
        b'Z' => Value::I(if v.as_i32() != 0 { 1 } else { 0 }),
        b'B' => Value::I(v.as_i32() as i8 as i32),
        b'C' => Value::I(v.as_i32() as u16 as i32),
        b'S' => Value::I(v.as_i32() as i16 as i32),
        b'I' | b'J' => v,
        _ => v,
    }
}
