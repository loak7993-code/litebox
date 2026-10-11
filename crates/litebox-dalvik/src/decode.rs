//! Bytecode pre-decoder. Raw DEX instruction streams are decoded once per
//! method into `Ins` vectors with pool references interned into registry ids,
//! branch targets resolved to decoded indices, and switch/array payloads
//! parsed inline. The interpreter dispatches on a flat enum with no re-parsing
//! in the hot path; direct-invoke and field targets get inline caches patched
//! on first execution.

use crate::classes::Registry;
use crate::decode_types::{
    CatchEntry, Cond, CmpOp, ConvOp, FieldRef, FillPayload, Ins, MathOp, MethodCode, MethodRef,
    SwitchKind, SwitchTable,
};
use litebox_dex::Dex;

pub struct DecodeError(pub String);

fn derr<T>(msg: impl std::fmt::Display) -> Result<T, DecodeError> {
    Err(DecodeError(msg.to_string()))
}

/// Decode one method's bytecode.
pub fn decode_method(
    dex: &Dex,
    reg: &mut Registry,
    code: &litebox_dex::CodeItem,
    shorty: &str,
) -> Result<MethodCode, DecodeError> {
    let raw = &code.insns;
    let n = raw.len();
    let mut unit_to_dec: Vec<i32> = vec![-1; n];
    let mut insns: Vec<Ins> = Vec::with_capacity(n);
    struct Patch {
        ins: usize,
        target_unit: i64,
    }
    let mut patches: Vec<Patch> = Vec::new();

    let mut i = 0usize;
    while i < n {
        let op = (raw[i] & 0xFF) as u8;
        // switch/array payloads are data, skip them entirely
        if op == 0x00 && (raw[i] >> 8) != 0 {
            let ident = raw[i];
            let skip = match ident {
                0x0100 => 4 + 2 * (raw.get(i + 1).copied().unwrap_or(0) as usize),  // packed: size, first(2), targets
                0x0200 => {
                    let size = raw.get(i + 1).copied().unwrap_or(0) as usize;
                    2 + 4 * size
                }
                0x0300 => {
                    let width = raw.get(i + 1).copied().unwrap_or(0) as usize;
                    let size = (raw.get(i + 2).copied().unwrap_or(0) as usize)
                        | ((raw.get(i + 3).copied().unwrap_or(0) as usize) << 16);
                    4 + (size * width).div_ceil(2)
                }
                _ => 1,
            };
            i += skip.max(1);
            continue;
        }
        unit_to_dec[i] = insns.len() as i32;
        let hi = (raw[i] >> 8) as u8;
        let word1 = raw.get(i + 1).copied().unwrap_or(0);
        let word2 = raw.get(i + 2).copied().unwrap_or(0);
        let s16 = word1 as i16 as i32;
        let u16v = word1 as u32;
        // Format-table convention: the FIRST operand is the LOW nibble of
        // byte1, the SECOND operand is the HIGH nibble (verified against
        // dexdump on toolchain-produced dex).
        let a4 = hi & 0x0F;
        let b4 = (hi >> 4) & 0x0F;
        let aa = hi;

        macro_rules! branch {
            ($off:expr) => {{
                insns.push(Ins::Goto { target: 0 });
                patches.push(Patch { ins: insns.len() - 1, target_unit: i as i64 + $off as i64 });
                $off
            }};
        }

        match op {
            0x00 => {
                insns.push(Ins::Nop);
                i += 1;
            }
            0x01 => {
                insns.push(Ins::Move { dst: a4, src: b4 });
                i += 1;
            }
            0x02 => {
                insns.push(Ins::Move { dst: aa, src: (word1 & 0xFF) as u8 });
                i += 2;
            }
            0x03 => {
                insns.push(Ins::Move { dst: word1 as u8, src: word2 as u8 });
                i += 3;
            }
            0x04 => {
                insns.push(Ins::MoveWide { dst: a4, src: b4 });
                i += 1;
            }
            0x05 => {
                insns.push(Ins::MoveWide { dst: aa, src: (word1 & 0xFF) as u8 });
                i += 2;
            }
            0x06 => {
                insns.push(Ins::MoveWide { dst: word1 as u8, src: word2 as u8 });
                i += 3;
            }
            0x07 => {
                insns.push(Ins::MoveObj { dst: a4, src: b4 });
                i += 1;
            }
            0x08 => {
                insns.push(Ins::MoveObj { dst: aa, src: (word1 & 0xFF) as u8 });
                i += 2;
            }
            0x09 => {
                insns.push(Ins::MoveObj { dst: word1 as u8, src: word2 as u8 });
                i += 3;
            }
            0x0a => {
                insns.push(Ins::MoveResult { dst: aa, wide: false });
                i += 1;
            }
            0x0b => {
                insns.push(Ins::MoveResult { dst: aa, wide: true });
                i += 1;
            }
            0x0c => {
                insns.push(Ins::MoveResult { dst: aa, wide: false });
                i += 1;
            }
            0x0d => {
                insns.push(Ins::MoveException { dst: aa });
                i += 1;
            }
            0x0e => {
                insns.push(Ins::ReturnVoid);
                i += 1;
            }
            0x0f..=0x11 => {
                insns.push(Ins::Return { reg: aa });
                i += 1;
            }
            0x12 => {
                let v = (b4 as i32) << 28 >> 28;
                insns.push(Ins::ConstI32 { dst: a4, v });
                i += 1;
            }
            0x13 => {
                insns.push(Ins::ConstI32 { dst: aa, v: s16 });
                i += 2;
            }
            0x14 => {
                let lo = word1 as u32 as i64;
                let hiw = word2 as u32 as i64;
                insns.push(Ins::ConstI32 { dst: aa, v: ((hiw << 16) | (lo & 0xFFFF)) as i32 });
                i += 3;
            }
            0x15 => {
                insns.push(Ins::ConstI32 { dst: aa, v: (word1 as i16 as i32) << 16 });
                i += 2;
            }
            0x16 => {
                insns.push(Ins::ConstI64 { dst: aa, v: s16 as i64 });
                i += 2;
            }
            0x17 => {
                let lo = word1 as u32 as i64;
                let hiw = word2 as u32 as i64;
                insns.push(Ins::ConstI64 { dst: aa, v: ((hiw << 16) | (lo & 0xFFFF)) });
                i += 3;
            }
            0x18 => {
                let mut v: i64 = 0;
                for k in 0..4 {
                    v |= (raw.get(i + 1 + k).copied().unwrap_or(0) as i64) << (16 * k);
                }
                insns.push(Ins::ConstI64 { dst: aa, v });
                i += 5;
            }
            0x19 => {
                insns.push(Ins::ConstI64 { dst: aa, v: ((word1 as i16 as i64) << 48) });
                i += 2;
            }
            0x1a | 0x1b => {
                let text = if op == 0x1a {
                    dex.string(u16v).unwrap_or("").to_string()
                } else {
                    let idx = (word1 as u32 as i64) | ((word2 as u32 as i64) << 16);
                    dex.string(idx as u32).unwrap_or("").to_string()
                };
                let sid = reg.intern_string(&text);
                insns.push(Ins::ConstString { dst: aa, s: sid });
                i += if op == 0x1a { 2 } else { 3 };
            }
            0x1c => {
                let tdesc = dex.type_name(u16v).unwrap_or("?").to_string();
                let class = reg.intern_type(&tdesc);
                insns.push(Ins::ConstClass { dst: aa, class });
                i += 2;
            }
            0x1f => {
                let tdesc = dex.type_name(u16v).unwrap_or("?").to_string();
                let class = reg.intern_type(&tdesc);
                insns.push(Ins::CheckCast { reg: aa, class });
                i += 2;
            }
            0x20 => {
                let tdesc = dex.type_name(u16v).unwrap_or("?").to_string();
                let class = reg.intern_type(&tdesc);
                insns.push(Ins::InstanceOf { dst: a4, obj: b4, class });
                i += 2;
            }
            0x21 => {
                insns.push(Ins::ArrayLen { dst: a4, arr: b4 });
                i += 1;
            }
            0x22 => {
                let tdesc = dex.type_name(u16v).unwrap_or("?").to_string();
                let class = reg.intern_type(&tdesc);
                insns.push(Ins::NewInstance { dst: aa, class });
                i += 2;
            }
            0x23 => {
                let edesc = dex.type_name(u16v).unwrap_or("[?").to_string();
                let elem = reg.intern_type(&edesc);
                insns.push(Ins::NewArray { dst: a4, len: b4, elem_desc: elem });
                i += 2;
            }
            0x24 | 0x25 => {
                let edesc = dex.type_name(u16v).unwrap_or("[?").to_string();
                let elem = reg.intern_type(&edesc);
                if op == 0x24 {
                    // 35c: unit0 = op|A|G, unit1 = type idx, unit2 = FEDC regs
                    let count = b4; // arg count is the high nibble of byte1
                    let mut rlist = [0u8; 5];
                    rlist[0] = (word2 & 0x0F) as u8;
                    rlist[1] = ((word2 >> 4) & 0x0F) as u8;
                    rlist[2] = ((word2 >> 8) & 0x0F) as u8;
                    rlist[3] = ((word2 >> 12) & 0x0F) as u8;
                    rlist[4] = a4; // G register (low nibble of byte1)
                    insns.push(Ins::FilledNewArray { regs: rlist, count, elem_desc: elem });
                } else {
                    insns.push(Ins::FilledNewArray { regs: [aa, 0, 0, 0, 0], count: word1 as u8, elem_desc: elem });
                }
                i += 3;
            }
            0x26 => {
                let off = s32_at(raw, i + 1);
                let payload_unit = (i as i64 + off as i64) as usize;
                let payload = parse_fill_payload(raw, payload_unit)?;
                insns.push(Ins::FillArrayData { arr: aa, data: Box::new(payload) });
                i += 3;
            }
            0x27 => {
                insns.push(Ins::Throw { reg: aa });
                i += 1;
            }
            0x28 => {
                let off = ((raw[i] >> 8) as i8) as i32;
                branch!(off);
                i += 1;
            }
            0x29 => {
                branch!(s16);
                i += 2;
            }
            0x2a => {
                branch!(s32_at(raw, i + 1));
                i += 3;
            }
            0x2b | 0x2c => {
                let off = s32_at(raw, i + 1);
                let payload_unit = (i as i64 + off as i64) as usize;
                let table = parse_switch_payload(raw, payload_unit, i as u32)?;
                let kind = if op == 0x2b {
                    SwitchKind::Packed
                } else {
                    SwitchKind::Sparse
                };
                insns.push(Ins::Switch { reg: aa, kind, table: Box::new(table) });
                i += 3;
            }
            0x2d..=0x31 => {
                let cop = match op {
                    0x2d => CmpOp::FloatL,
                    0x2e => CmpOp::FloatG,
                    0x2f => CmpOp::DoubleL,
                    0x30 => CmpOp::DoubleG,
                    _ => CmpOp::Long,
                };
                insns.push(Ins::Cmp { op: cop, dst: aa, a: word1 as u8, b: (word1 >> 8) as u8 });
                i += 2;
            }
            0x32..=0x37 => {
                let cond = match op {
                    0x32 => Cond::Eq,
                    0x33 => Cond::Ne,
                    0x34 => Cond::Lt,
                    0x35 => Cond::Ge,
                    0x36 => Cond::Gt,
                    _ => Cond::Le,
                };
                insns.push(Ins::Branch { cond, a: a4, b: b4, target: 0 });
                patches.push(Patch { ins: insns.len() - 1, target_unit: i as i64 + s16 as i64 });
                i += 2;
            }
            0x38..=0x3d => {
                let cond = match op {
                    0x38 => Cond::Eq,
                    0x39 => Cond::Ne,
                    0x3a => Cond::Lt,
                    0x3b => Cond::Ge,
                    0x3c => Cond::Gt,
                    _ => Cond::Le,
                };
                insns.push(Ins::BranchZ { cond, a: aa, target: 0 });
                patches.push(Patch { ins: insns.len() - 1, target_unit: i as i64 + s16 as i64 });
                i += 2;
            }
            0x44..=0x51 => {
                let elem = elem_type_for_array_op(reg, op);
                if op < 0x4b {
                    insns.push(Ins::AGet {
                        elem_desc: elem,
                        dst: aa,
                        arr: word1 as u8,
                        idx: (word1 >> 8) as u8,
                    });
                } else {
                    insns.push(Ins::APut {
                        elem_desc: elem,
                        src: aa,
                        arr: word1 as u8,
                        idx: (word1 >> 8) as u8,
                    });
                }
                i += 2;
            }
            0x52..=0x6d => {
                let field_idx = u16v;
                let fid = dex
                    .field_ids
                    .get(field_idx as usize)
                    .ok_or_else(|| DecodeError(format!("field idx {field_idx} out of range")))?;
                let cdesc = dex.type_name(fid.class_idx as u32).unwrap_or("?").to_string();
                let ndesc = dex.type_name(fid.type_idx as u32).unwrap_or("?").to_string();
                let name = dex.string(fid.name_idx).unwrap_or("?").to_string();
                let class = reg.intern_type(&cdesc);
                let desc = reg.intern_type(&ndesc);
                let name = reg.intern_name(&name);
                let field = FieldRef::Unresolved { class, name, desc };
                let is_instance = op < 0x60;
                if is_instance {
                    if op < 0x59 {
                        insns.push(Ins::IGet { field, dst: a4, obj: b4 });
                    } else {
                        insns.push(Ins::IPut { field, src: a4, obj: b4 });
                    }
                } else if op < 0x65 {
                    insns.push(Ins::SGet { field, dst: aa });
                } else {
                    insns.push(Ins::SPut { field, src: aa });
                }
                i += 2;
            }
            0x6e..=0x72 | 0x74..=0x78 => {
                let midx = u16v;
                let m = dex
                    .method_ids
                    .get(midx as usize)
                    .ok_or_else(|| DecodeError(format!("method idx {midx} out of range")))?;
                let name = reg.intern_name(dex.string(m.name_idx).unwrap_or("?"));
                let proto = &dex.protos[m.proto_idx as usize];
                let shorty_id = reg.intern_shorty(dex.string(proto.shorty_idx).unwrap_or(shorty));
                let is_range = op >= 0x74;
                let mref = match op & 0x7F {
                    0x6e | 0x74 => MethodRef::Virtual { name, shorty: shorty_id, is_interface: false },
                    0x6f | 0x75 => MethodRef::Super { name, shorty: shorty_id },
                    0x70 | 0x76 => MethodRef::Direct {
                        dex: reg.current_dex,
                        method_idx: midx,
                        is_static: false,
                        resolved: u32::MAX,
                    },
                    0x71 | 0x77 => MethodRef::Direct {
                        dex: reg.current_dex,
                        method_idx: midx,
                        is_static: true,
                        resolved: u32::MAX,
                    },
                    // invoke-interface / invoke-interface/range
                    _ => MethodRef::Virtual { name, shorty: shorty_id, is_interface: true },
                };
                if is_range {
                    insns.push(Ins::InvokeRange { method: mref, start: word1 as u8, count: aa });
                } else {
                    // 35c: unit0 = op|A|G, unit1 = method idx, unit2 = FEDC regs
                    let mut rlist = [0u8; 5];
                    rlist[0] = (word2 & 0x0F) as u8;
                    rlist[1] = ((word2 >> 4) & 0x0F) as u8;
                    rlist[2] = ((word2 >> 8) & 0x0F) as u8;
                    rlist[3] = ((word2 >> 12) & 0x0F) as u8;
                    rlist[4] = a4; // G register (low nibble)
                    let count = b4; // arg count (high nibble)
                    insns.push(Ins::Invoke { method: mref, regs: rlist, count });
                }
                i += 3;
            }
            0x7b..=0x8f => {
                let cop = match op {
                    0x7b => ConvOp::NegInt,
                    0x7c => ConvOp::NotInt,
                    0x7d => ConvOp::NegLong,
                    0x7e => ConvOp::NotLong,
                    0x7f => ConvOp::NegFloat,
                    0x80 => ConvOp::NegDouble,
                    0x81 => ConvOp::IntToLong,
                    0x82 => ConvOp::IntToFloat,
                    0x83 => ConvOp::IntToDouble,
                    0x84 => ConvOp::LongToInt,
                    0x85 => ConvOp::LongToFloat,
                    0x86 => ConvOp::LongToDouble,
                    0x87 => ConvOp::FloatToInt,
                    0x88 => ConvOp::FloatToLong,
                    0x89 => ConvOp::FloatToDouble,
                    0x8a => ConvOp::DoubleToInt,
                    0x8b => ConvOp::DoubleToLong,
                    0x8c => ConvOp::DoubleToFloat,
                    0x8d => ConvOp::IntToByte,
                    0x8e => ConvOp::IntToChar,
                    _ => ConvOp::IntToShort,
                };
                insns.push(Ins::Conv { op: cop, dst: a4, a: b4 });
                i += 1;
            }
            0x90..=0xaf => {
                let (mop, wide, float) = binop_full(op);
                insns.push(Ins::Math {
                    op: mop,
                    wide,
                    float,
                    dst: aa,
                    a: word1 as u8,
                    b: (word1 >> 8) as u8,
                });
                i += 2;
            }
            0xb0..=0xcf => {
                // binop/2addr (12x): 0xb0-0xba int, 0xbb-0xc5 long,
                // 0xc6-0xca float, 0xcb-0xcf double
                let (mop, wide, float) = twoaddr_full(op);
                insns.push(Ins::Math2Addr { op: mop, wide, float, a: a4, b: b4 });
                i += 1;
            }
            0xd0..=0xd7 => {
                // binop/lit16 (22s)
                let mop = match op {
                    0xd0 => MathOp::Add,
                    0xd1 => MathOp::Sub,
                    0xd2 => MathOp::Mul,
                    0xd3 => MathOp::Div,
                    0xd4 => MathOp::Rem,
                    0xd5 => MathOp::And,
                    0xd6 => MathOp::Or,
                    _ => MathOp::Xor,
                };
                insns.push(Ins::MathLit { op: mop, dst: a4, a: b4, lit: s16 });
                i += 2;
            }
            0xd8..=0xe2 => {
                // binop/lit8 (22b)
                let mop = match op {
                    0xd8 => MathOp::Add,
                    0xd9 => MathOp::Sub,
                    0xda => MathOp::Mul,
                    0xdb => MathOp::Div,
                    0xdc => MathOp::Rem,
                    0xdd => MathOp::And,
                    0xde => MathOp::Or,
                    0xdf => MathOp::Xor,
                    0xe0 => MathOp::Shl,
                    0xe1 => MathOp::Shr,
                    _ => MathOp::Ushr,
                };
                // 22b: [op|AA|BB|CC] = unit0(op,dst) unit1(src,lit)
                insns.push(Ins::MathLit {
                    op: mop,
                    dst: aa,
                    a: (word1 & 0xFF) as u8,
                    lit: (word1 >> 8) as i8 as i32,
                });
                i += 2;
            }
            0xfa..=0xfd => {
                insns.push(Ins::Invoke {
                    method: MethodRef::Unsupported { op },
                    regs: [0; 5],
                    count: 0,
                });
                i += 3;
            }
            _ => {
                return derr(format!(
                    "reserved/unknown opcode {op:#04x} at unit {i}"
                ));
            }
        }
    }

    for p in &patches {
        let target_unit = p.target_unit;
        if target_unit < 0 || target_unit as usize >= unit_to_dec.len() {
            return derr(format!("branch target unit {target_unit} out of range"));
        }
        let t = unit_to_dec[target_unit as usize];
        if t < 0 {
            return derr(format!("branch target not at instruction boundary (unit {target_unit})"));
        }
        match &mut insns[p.ins] {
            Ins::Goto { target } => *target = t as u32,
            Ins::Branch { target, .. } | Ins::BranchZ { target, .. } => *target = t as u32,
            _ => unreachable!(),
        }
    }

    // try/catch: convert raw unit ranges to decoded index ranges
    let mut tries = Vec::new();
    let mut handlers = Vec::new();
    for t in &code.tries {
        let hidx = code
            .handler_map
            .iter()
            .find(|(off, _)| *off == t.handler_off)
            .map(|(_, i)| *i);
        let Some(hi) = hidx else { continue };
        let ce = &code.handlers[hi];
        let mut typed: Vec<(u32, u32)> = Vec::new();
        for (ty, addr) in &ce.typed {
            let desc = dex.type_name(*ty).unwrap_or("?").to_string();
            let tid = reg.intern_type(&desc);
            let dpc = unit_to_dec.get(*addr as usize).copied().unwrap_or(-1);
            if dpc >= 0 {
                typed.push((tid, dpc as u32));
            }
        }
        let catch_all = ce.catch_all.and_then(|a| {
            let v = unit_to_dec.get(a as usize).copied().unwrap_or(-1);
            if v >= 0 {
                Some(v as u32)
            } else {
                None
            }
        });
        handlers.push(CatchEntry { typed, catch_all });
        let start_dec = unit_to_dec[t.start_addr as usize];
        let end_raw = t.start_addr as usize + t.insn_count as usize;
        let end_dec = if end_raw >= n {
            insns.len() as i32
        } else {
            unit_to_dec[end_raw]
        };
        if start_dec < 0 || end_dec < 0 {
            continue;
        }
        tries.push((start_dec as u32, (end_dec - start_dec) as u16, handlers.len() - 1));
    }

    if std::env::var("LB_TRACE").is_ok() {
        eprintln!("decoded method: {} insns", insns.len());
        for (idx, ins) in insns.iter().enumerate() {
            let kind = match ins {
                Ins::Nop => "nop".into(),
                Ins::Move { dst, src } => format!("move d{} s{}", dst, src),
                Ins::MoveWide { dst, src } => format!("move-wide d{} s{}", dst, src),
                Ins::MoveObj { dst, src } => format!("move-obj d{} s{}", dst, src),
                Ins::MoveResult { dst, wide } => format!("move-result d{} wide={}", dst, wide),
                Ins::MoveException { dst } => format!("move-exn d{}", dst),
                Ins::Return { reg } => format!("return r{}", reg),
                Ins::ReturnVoid => "return-void".into(),
                Ins::ConstI32 { dst, v } => format!("const/32 d{} = {}", dst, v),
                Ins::ConstI64 { dst, v } => format!("const/64 d{} = {}", dst, v),
                Ins::ConstF32 { dst, v } => format!("const-f32 d{} = {}", dst, v),
                Ins::ConstF64 { dst, v } => format!("const-f64 d{} = {}", dst, v),
                Ins::ConstString { dst, s } => format!("const-str d{} s{}", dst, s),
                Ins::ConstClass { dst, class } => format!("const-class d{} c{}", dst, class),
                Ins::CheckCast { reg, class } => format!("check-cast r{} c{}", reg, class),
                Ins::InstanceOf { dst, obj, class } => format!("instance-of d{} o{} c{}", dst, obj, class),
                Ins::ArrayLen { dst, arr } => format!("array-len d{} a{}", dst, arr),
                Ins::NewInstance { dst, class } => format!("new d{} c{}", dst, class),
                Ins::NewArray { dst, len, elem_desc } => format!("new-array d{} l{} t{}", dst, len, elem_desc),
                Ins::FilledNewArray { count, elem_desc, .. } => format!("filled-new-array n{} t{}", count, elem_desc),
                Ins::FillArrayData { arr, .. } => format!("fill-data a{}", arr),
                Ins::Switch { reg, .. } => format!("switch r{}", reg),
                Ins::Throw { reg } => format!("throw r{}", reg),
                Ins::Goto { target } => format!("goto {}", target),
                Ins::Branch { cond, a, b, target } => format!("branch {:?} {} {} -> {}", cond, a, b, target),
                Ins::BranchZ { cond, a, target } => format!("branchz {:?} {} -> {}", cond, a, target),
                Ins::Cmp { op, dst, a, b } => format!("cmp {:?} d{} {} {}", op, dst, a, b),
                Ins::Math { op, wide, float, dst, a, b } => format!("math {:?} w{} f{} d{} {} {}", op, wide, float, dst, a, b),
                Ins::MathLit { op, dst, a, lit } => format!("math-lit {:?} d{} {} {}", op, dst, a, lit),
                Ins::Math2Addr { op, wide, float, a, b } => format!("2addr {:?} w{} f{} {} {}", op, wide, float, a, b),
                Ins::Conv { op, dst, a } => format!("conv {:?} d{} a{}", op, dst, a),
                Ins::AGet { elem_desc, dst, arr, idx } => format!("aget t{} d{} a{} i{}", elem_desc, dst, arr, idx),
                Ins::APut { elem_desc, src, arr, idx } => format!("aput t{} s{} a{} i{}", elem_desc, src, arr, idx),
                Ins::IGet { field, dst, obj } => format!("iget {:?} d{} o{}", field, dst, obj),
                Ins::IPut { field, src, obj } => format!("iput {:?} s{} o{}", field, src, obj),
                Ins::SGet { field, dst } => format!("sget {:?} d{}", field, dst),
                Ins::SPut { field, src } => format!("sput {:?} s{}", field, src),
                Ins::Invoke { method, regs, count } => format!("invoke {:?} regs={:?} n={}", method, regs, count),
                Ins::InvokeRange { method, start, count } => format!("invoke-range {:?} s{} n={}", method, start, count),
                Ins::MonitorEnter { obj } => format!("mon-enter r{}", obj),
                Ins::MonitorExit { obj } => format!("mon-exit r{}", obj),
            };
            eprintln!("  [{}] {}", idx, kind);
        }
    }
    Ok(MethodCode {
        insns,
        registers_size: code.registers_size,
        ins_size: code.ins_size,
        outs_size: code.outs_size,
        tries,
        handlers,
    })
}

fn s32_at(raw: &[u16], i: usize) -> i32 {
    let lo = raw.get(i).copied().unwrap_or(0) as u32 as i64;
    let hi = raw.get(i + 1).copied().unwrap_or(0) as u32 as i64;
    ((hi << 16) | (lo & 0xFFFF)) as i32
}

fn elem_type_for_array_op(reg: &mut Registry, op: u8) -> u32 {
    match op {
        0x44 | 0x4b => reg.intern_type("I"),
        0x45 | 0x4c => reg.intern_type("J"),
        0x46 | 0x4d => reg.intern_type("Ljava/lang/Object;"),
        0x47 | 0x4e => reg.intern_type("Z"),
        0x48 | 0x4f => reg.intern_type("B"),
        0x49 | 0x50 => reg.intern_type("C"),
        _ => reg.intern_type("S"),
    }
}

fn binop_full(op: u8) -> (crate::decode_types::MathOp, bool, bool) {
    use MathOp::*;
    match op {
        0x90 => (Add, false, false),
        0x91 => (Sub, false, false),
        0x92 => (Mul, false, false),
        0x93 => (Div, false, false),
        0x94 => (Rem, false, false),
        0x95 => (And, false, false),
        0x96 => (Or, false, false),
        0x97 => (Xor, false, false),
        0x98 => (Shl, false, false),
        0x99 => (Shr, false, false),
        0x9a => (Ushr, false, false),
        0x9b => (Add, true, false),
        0x9c => (Sub, true, false),
        0x9d => (Mul, true, false),
        0x9e => (Div, true, false),
        0x9f => (Rem, true, false),
        0xa0 => (And, true, false),
        0xa1 => (Or, true, false),
        0xa2 => (Xor, true, false),
        0xa3 => (Shl, true, false),
        0xa4 => (Shr, true, false),
        0xa5 => (Ushr, true, false),
        0xa6 => (Add, false, true),
        0xa7 => (Sub, false, true),
        0xa8 => (Mul, false, true),
        0xa9 => (Div, false, true),
        0xaa => (Rem, false, true),
        0xab => (Add, true, true),
        0xac => (Sub, true, true),
        0xad => (Mul, true, true),
        0xae => (Div, true, true),
        _ => (Rem, true, true),
    }
}

fn twoaddr_full(op: u8) -> (MathOp, bool, bool) {
    use MathOp::*;
    match op {
        0xb0 => (Add, false, false),
        0xb1 => (Sub, false, false),
        0xb2 => (Mul, false, false),
        0xb3 => (Div, false, false),
        0xb4 => (Rem, false, false),
        0xb5 => (And, false, false),
        0xb6 => (Or, false, false),
        0xb7 => (Xor, false, false),
        0xb8 => (Shl, false, false),
        0xb9 => (Shr, false, false),
        0xba => (Ushr, false, false),
        0xbb => (Add, true, false),
        0xbc => (Sub, true, false),
        0xbd => (Mul, true, false),
        0xbe => (Div, true, false),
        0xbf => (Rem, true, false),
        0xc0 => (And, true, false),
        0xc1 => (Or, true, false),
        0xc2 => (Xor, true, false),
        0xc3 => (Shl, true, false),
        0xc4 => (Shr, true, false),
        0xc5 => (Ushr, true, false),
        0xc6 => (Add, false, true),
        0xc7 => (Sub, false, true),
        0xc8 => (Mul, false, true),
        0xc9 => (Div, false, true),
        0xca => (Rem, false, true),
        0xcb => (Add, true, true),
        0xcc => (Sub, true, true),
        0xcd => (Mul, true, true),
        0xce => (Div, true, true),
        _ => (Rem, true, true),
    }
}

fn parse_switch_payload(
    raw: &[u16],
    unit: usize,
    switch_unit: u32,
) -> Result<SwitchTable, DecodeError> {
    let ident = raw.get(unit).copied().unwrap_or(0);
    let size = raw.get(unit + 1).copied().unwrap_or(0) as usize;
    let rel = |k: usize| -> u32 {
        let v = s32_at(raw, unit + k);
        (switch_unit as i32 + v) as u32
    };
    match ident {
        0x0100 => {
            let first = s32_at(raw, unit + 2);
            let mut targets = Vec::with_capacity(size);
            for k in 0..size {
                targets.push(rel(4 + k * 2));
            }
            Ok(SwitchTable::Packed { first, targets })
        }
        0x0200 => {
            let mut keys = Vec::with_capacity(size);
            for k in 0..size {
                keys.push(s32_at(raw, unit + 2 + k * 2));
            }
            let mut targets = Vec::with_capacity(size);
            for k in 0..size {
                targets.push(rel(2 + size * 2 + k * 2));
            }
            Ok(SwitchTable::Sparse { keys, targets })
        }
        _ => derr(format!("bad switch payload ident {ident:#x}")),
    }
}

fn parse_fill_payload(raw: &[u16], unit: usize) -> Result<FillPayload, DecodeError> {
    let ident = raw.get(unit).copied().unwrap_or(0);
    if ident != 0x0300 {
        return derr(format!("bad fill payload ident {ident:#x}"));
    }
    let width = raw.get(unit + 1).copied().unwrap_or(0);
    let sz = (raw.get(unit + 2).copied().unwrap_or(0) as u32)
        | ((raw.get(unit + 3).copied().unwrap_or(0) as u32) << 16);
    let mut data = Vec::with_capacity((sz as usize) * (width as usize));
    for k in 0..(sz as usize) * (width as usize).div_ceil(2) {
        let w = raw.get(unit + 4 + k).copied().unwrap_or(0);
        data.push((w & 0xFF) as u8);
        data.push((w >> 8) as u8);
    }
    data.truncate((sz as usize) * (width as usize));
    Ok(FillPayload::Data { width, data })
}


