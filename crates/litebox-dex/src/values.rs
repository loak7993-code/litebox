//! encoded_value parsing (dex static initializers and annotation elements).

use crate::{sleb, uleb, DexError, NO_INDEX};

#[derive(Debug, Clone, PartialEq)]
pub enum EncodedValue {
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    MethodType(u32),
    Field(u32),
    Method(u32),
    Enum(u32),
    Array(Vec<EncodedValue>),
    Null,
    Bool(bool),
}

const T_BYTE: u8 = 0x00;
const T_SHORT: u8 = 0x02;
const T_CHAR: u8 = 0x03;
const T_INT: u8 = 0x04;
const T_LONG: u8 = 0x06;
const T_FLOAT: u8 = 0x10;
const T_DOUBLE: u8 = 0x11;
const T_METHOD_TYPE: u8 = 0x15;
const T_FIELD: u8 = 0x17;
const T_METHOD: u8 = 0x18;
const T_ENUM: u8 = 0x19;
const T_ARRAY: u8 = 0x1a;
const T_ANNOTATION: u8 = 0x1b;
const T_NULL: u8 = 0x1c;
const T_BOOL: u8 = 0x1d;

fn sign_extend(v: u64, bits: usize) -> i64 {
    if bits == 0 || bits >= 64 {
        return v as i64;
    }
    if (v >> (bits - 1)) & 1 == 1 {
        (v | !((1u64 << bits) - 1)) as i64
    } else {
        v as i64
    }
}

/// Parse one encoded_value at `pos`; returns value and bytes consumed.
pub fn parse_encoded_value(bytes: &[u8], pos: usize) -> Result<(EncodedValue, usize), DexError> {
    if pos >= bytes.len() {
        return Err(DexError("encoded_value out of range".into()));
    }
    let b = bytes[pos];
    let arg = (b >> 5) as usize;
    let typ = b & 0x1F;
    let mut p = pos + 1;
    let nbytes = arg + 1;
    let need = |p: usize, cnt: usize| -> Result<(), DexError> {
        if p + cnt > bytes.len() {
            Err(DexError("encoded_value truncated".into()))
        } else {
            Ok(())
        }
    };
    let read_le = |bytes: &[u8], p: usize, cnt: usize| -> u64 {
        let mut v: u64 = 0;
        for k in 0..cnt {
            v |= (bytes[p + k] as u64) << (8 * k);
        }
        v
    };
    match typ {
        T_BYTE => {
            need(p, 1)?;
            Ok((EncodedValue::Byte(bytes[p] as i8), 2))
        }
        T_SHORT => {
            need(p, nbytes)?;
            let v = sign_extend(read_le(bytes, p, nbytes), nbytes * 8);
            Ok((EncodedValue::Short(v as i16), 1 + nbytes))
        }
        T_CHAR => {
            need(p, nbytes)?;
            Ok((EncodedValue::Char(read_le(bytes, p, nbytes) as u16), 1 + nbytes))
        }
        T_INT => {
            need(p, nbytes)?;
            let v = sign_extend(read_le(bytes, p, nbytes), nbytes * 8);
            Ok((EncodedValue::Int(v as i32), 1 + nbytes))
        }
        T_LONG => {
            need(p, nbytes)?;
            let v = sign_extend(read_le(bytes, p, nbytes), nbytes * 8);
            Ok((EncodedValue::Long(v), 1 + nbytes))
        }
        T_FLOAT => {
            // value_arg+1 bytes, zero-extended to the right (low bytes present)
            need(p, nbytes)?;
            let mut v: u32 = 0;
            for k in 0..nbytes {
                v |= (bytes[p + k] as u32) << (8 * k);
            }
            Ok((EncodedValue::Float(f32::from_bits(v)), 1 + nbytes))
        }
        T_DOUBLE => {
            need(p, nbytes)?;
            let mut v: u64 = 0;
            for k in 0..nbytes {
                v |= (bytes[p + k] as u64) << (8 * k);
            }
            Ok((EncodedValue::Double(f64::from_bits(v)), 1 + nbytes))
        }
        T_METHOD_TYPE | T_FIELD | T_METHOD | T_ENUM => {
            need(p, nbytes)?;
            let v = read_le(bytes, p, nbytes) as u32;
            let ev = match typ {
                T_METHOD_TYPE => EncodedValue::MethodType(v),
                T_FIELD => EncodedValue::Field(v),
                T_METHOD => EncodedValue::Method(v),
                _ => EncodedValue::Enum(v),
            };
            Ok((ev, 1 + nbytes))
        }
        T_ARRAY => {
            let (n, used) = uleb(bytes, p);
            p += used;
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let (v, used) = parse_encoded_value(bytes, p)?;
                items.push(v);
                p += used;
            }
            Ok((EncodedValue::Array(items), p - pos))
        }
        T_ANNOTATION => {
            // encoded_annotation: type_idx uleb, size uleb, elements
            let (_ty, used) = uleb(bytes, p);
            p += used;
            let (n, used) = uleb(bytes, p);
            p += used;
            for _ in 0..n {
                let (_name, used) = uleb(bytes, p);
                p += used;
                let (_v, used) = parse_encoded_value(bytes, p)?;
                p += used;
            }
            Ok((EncodedValue::Null, p - pos)) // annotation static values are not used by the runtime
        }
        T_NULL => Ok((EncodedValue::Null, 1)),
        T_BOOL => Ok((EncodedValue::Bool(arg != 0), 1)),
        _ => Err(DexError(format!("unknown encoded_value type {typ:#x}"))),
    }
}

#[allow(dead_code)]
fn unused_helpers_silence() {
    let _ = NO_INDEX;
    let _ = sleb(&[0x00], 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_forms() {
        // bool true: (1<<5)|0x1d = 0x3d
        assert_eq!(parse_encoded_value(&[0x3d], 0).unwrap(), (EncodedValue::Bool(true), 1));
        // null: 0x1c
        assert_eq!(parse_encoded_value(&[0x1c], 0).unwrap(), (EncodedValue::Null, 1));
        // int 7, one byte: (0<<5)|0x04
        assert_eq!(parse_encoded_value(&[0x04, 0x07], 0).unwrap(), (EncodedValue::Int(7), 2));
        // int -1, one byte (sign-extended): (0<<5)|0x04 value 0xFF
        assert_eq!(parse_encoded_value(&[0x04, 0xFF], 0).unwrap(), (EncodedValue::Int(-1), 2));
        // byte 0x7F: (0<<5)|0x00
        assert_eq!(parse_encoded_value(&[0x00, 0x7F], 0).unwrap(), (EncodedValue::Byte(0x7F), 2));
        // char 'A' two bytes: (1<<5)|0x03 = 0x23
        assert_eq!(parse_encoded_value(&[0x23, 0x41, 0x00], 0).unwrap(), (EncodedValue::Char(0x41), 3));
        // float 1.0 = 0x3F800000: 4 bytes, (3<<5)|0x10 = 0x70
        let f = 1.0f32.to_bits().to_le_bytes();
        let mut v = vec![0x70];
        v.extend_from_slice(&f);
        assert_eq!(parse_encoded_value(&v, 0).unwrap(), (EncodedValue::Float(1.0), 5));
        // long with 8 bytes: (7<<5)|0x06 = 0xE6
        let mut v = vec![0xE6];
        v.extend_from_slice(&100i64.to_le_bytes());
        assert_eq!(parse_encoded_value(&v, 0).unwrap(), (EncodedValue::Long(100), 9));
    }

    #[test]
    fn array_form() {
        // array of 2 bools: 0x1a(size arg encoded), then two bool bytes
        let bytes = [0x1a, 0x02, 0x1c, 0x3d];
        let (v, used) = parse_encoded_value(&bytes, 0).unwrap();
        assert_eq!(v, EncodedValue::Array(vec![EncodedValue::Null, EncodedValue::Bool(true)]));
        assert_eq!(used, 4);
    }
}
