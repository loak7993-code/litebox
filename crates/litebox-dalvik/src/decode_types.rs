//! Decoded instruction types shared by the decoder and the interpreter.

use crate::classes::FieldTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Eq,
    Ne,
    Lt,
    Ge,
    Gt,
    Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    FloatL,
    FloatG,
    DoubleL,
    DoubleG,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Ushr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvOp {
    IntToLong,
    IntToFloat,
    IntToDouble,
    LongToInt,
    LongToFloat,
    LongToDouble,
    FloatToInt,
    FloatToLong,
    FloatToDouble,
    DoubleToInt,
    DoubleToLong,
    DoubleToFloat,
    IntToByte,
    IntToChar,
    IntToShort,
    NegInt,
    NotInt,
    NegLong,
    NotLong,
    NegFloat,
    NegDouble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchKind {
    Packed,
    Sparse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodRef {
    /// resolved against the receiver's runtime class (vmap/imap)
    Virtual { name: u32, shorty: u32, is_interface: bool },
    /// direct/static: lazy-resolved global method idx cached in `resolved`
    Direct { dex: u32, method_idx: u32, is_static: bool, resolved: u32 },
    /// invoke-super: dispatch in the defining class's superclass chain
    Super { name: u32, shorty: u32 },
    /// invoke-polymorphic / invoke-custom: not implemented
    Unsupported { op: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRef {
    Unresolved { class: u32, name: u32, desc: u32 },
    Resolved(FieldTarget),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwitchTable {
    Packed { first: i32, targets: Vec<u32> },
    Sparse { keys: Vec<i32>, targets: Vec<u32> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FillPayload {
    Data { width: u16, data: Vec<u8> },
}

#[derive(Debug, Clone)]
pub enum Ins {
    Nop,
    Move { dst: u8, src: u8 },
    MoveWide { dst: u8, src: u8 },
    MoveObj { dst: u8, src: u8 },
    MoveResult { dst: u8, wide: bool },
    MoveException { dst: u8 },
    Return { reg: u8 },
    ReturnVoid,
    ConstI32 { dst: u8, v: i32 },
    ConstI64 { dst: u8, v: i64 },
    ConstF32 { dst: u8, v: f32 },
    ConstF64 { dst: u8, v: f64 },
    ConstString { dst: u8, s: u32 },
    ConstClass { dst: u8, class: u32 },
    CheckCast { reg: u8, class: u32 },
    InstanceOf { dst: u8, obj: u8, class: u32 },
    ArrayLen { dst: u8, arr: u8 },
    NewInstance { dst: u8, class: u32 },
    NewArray { dst: u8, len: u8, elem_desc: u32 },
    FilledNewArray { regs: [u8; 5], count: u8, elem_desc: u32 },
    FillArrayData { arr: u8, data: Box<FillPayload> },
    Switch { reg: u8, kind: SwitchKind, table: Box<SwitchTable> },
    Throw { reg: u8 },
    Goto { target: u32 },
    Branch { cond: Cond, a: u8, b: u8, target: u32 },
    BranchZ { cond: Cond, a: u8, target: u32 },
    Cmp { op: CmpOp, dst: u8, a: u8, b: u8 },
    Math { op: MathOp, wide: bool, float: bool, dst: u8, a: u8, b: u8 },
    MathLit { op: MathOp, dst: u8, a: u8, lit: i32 },
    Math2Addr { op: MathOp, wide: bool, float: bool, a: u8, b: u8 },
    Conv { op: ConvOp, dst: u8, a: u8 },
    AGet { elem_desc: u32, dst: u8, arr: u8, idx: u8 },
    APut { elem_desc: u32, src: u8, arr: u8, idx: u8 },
    IGet { field: FieldRef, dst: u8, obj: u8 },
    IPut { field: FieldRef, src: u8, obj: u8 },
    SGet { field: FieldRef, dst: u8 },
    SPut { field: FieldRef, src: u8 },
    Invoke { method: MethodRef, regs: [u8; 5], count: u8 },
    InvokeRange { method: MethodRef, start: u8, count: u8 },
    MonitorEnter { obj: u8 },
    MonitorExit { obj: u8 },
}

pub struct MethodCode {
    pub insns: Vec<Ins>,
    pub registers_size: u16,
    pub ins_size: u16,
    pub outs_size: u16,
    /// (decoded start, decoded count, handler index)
    pub tries: Vec<(u32, u16, usize)>,
    pub handlers: Vec<CatchEntry>,
}

#[derive(Debug, Clone)]
pub struct CatchEntry {
    /// (interned type id, handler insn)
    pub typed: Vec<(u32, u32)>,
    pub catch_all: Option<u32>,
}
