//! DEX file parser (Dalvik Executable, versions 035-039).
//! Spec-complete for what the runtime needs: header, ids, class defs,
//! class data, code items, try/catch tables, static value arrays.

mod strings;
mod values;

pub use strings::decode_mutf8;
pub use values::EncodedValue;

#[derive(Debug)]
pub struct DexError(pub String);

impl std::fmt::Display for DexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dex error: {}", self.0)
    }
}
impl std::error::Error for DexError {}

fn derr<T>(msg: impl std::fmt::Display) -> Result<T, DexError> {
    Err(DexError(msg.to_string()))
}

pub const NO_INDEX: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone)]
pub struct DexHeader {
    pub magic: [u8; 8],
    pub checksum: u32,
    pub file_size: u32,
    pub map_off: u32,
    pub string_ids_size: u32,
    pub string_ids_off: u32,
    pub type_ids_size: u32,
    pub type_ids_off: u32,
    pub proto_ids_size: u32,
    pub proto_ids_off: u32,
    pub field_ids_size: u32,
    pub field_ids_off: u32,
    pub method_ids_size: u32,
    pub method_ids_off: u32,
    pub class_defs_size: u32,
    pub class_defs_off: u32,
}

#[derive(Debug, Clone)]
pub struct ProtoId {
    pub shorty_idx: u32,
    pub return_type_idx: u32,
    pub params: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct FieldId {
    pub class_idx: u16,
    pub type_idx: u16,
    pub name_idx: u32,
}

#[derive(Debug, Clone)]
pub struct MethodId {
    pub class_idx: u16,
    pub proto_idx: u16,
    pub name_idx: u32,
}

#[derive(Debug, Clone)]
pub struct EncodedField {
    pub field_idx: u32,
    pub access_flags: u32,
}

#[derive(Debug, Clone)]
pub struct EncodedMethod {
    pub method_idx: u32,
    pub access_flags: u32,
    pub code: Option<CodeItem>,
}

#[derive(Debug, Clone)]
pub struct ClassDef {
    pub class_idx: u32,
    pub access_flags: u32,
    pub superclass_idx: u32,
    pub interfaces: Vec<u32>,
    pub source_file: Option<u32>,
    pub static_fields: Vec<EncodedField>,
    pub instance_fields: Vec<EncodedField>,
    pub direct_methods: Vec<EncodedMethod>,
    pub virtual_methods: Vec<EncodedMethod>,
    pub static_values: Vec<EncodedValue>,
}

#[derive(Debug, Clone)]
pub struct TryItem {
    pub start_addr: u32,
    pub insn_count: u16,
    pub handler_off: u16,
}

#[derive(Debug, Clone)]
pub struct CatchHandler {
    pub typed: Vec<(u32, u32)>,
    pub catch_all: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CodeItem {
    pub registers_size: u16,
    pub ins_size: u16,
    pub outs_size: u16,
    pub insns: Vec<u16>,
    pub tries: Vec<TryItem>,
    /// handlers keyed by handler_off from try items
    pub handler_map: Vec<(u16, usize)>,
    pub handlers: Vec<CatchHandler>,
}

#[derive(Debug)]
pub struct Dex {
    bytes: Vec<u8>,
    pub header: DexHeader,
    string_offsets: Vec<u32>,
    string_cache: Vec<std::sync::OnceLock<Box<str>>>,
    pub type_ids: Vec<u32>,
    pub protos: Vec<ProtoId>,
    pub field_ids: Vec<FieldId>,
    pub method_ids: Vec<MethodId>,
    pub class_defs: Vec<ClassDef>,
}

// Safe: string_cache mutates only through resize + slot set; concurrent readers
// may race on the same slot but always write identical decoded values.
unsafe impl Send for Dex {}
unsafe impl Sync for Dex {}

impl Dex {
    pub fn parse(bytes: Vec<u8>) -> Result<Dex, DexError> {
        if bytes.len() < 0x70 {
            return derr("file too small to be a dex");
        }
        let magic: [u8; 8] = bytes[..8].try_into().unwrap();
        if &magic[..4] != b"dex\n" {
            return derr("bad dex magic");
        }
        let u32at = |off: usize| -> u32 {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        };
        let header = DexHeader {
            magic,
            checksum: u32at(8),
            file_size: u32at(32),
            map_off: u32at(52),
            string_ids_size: u32at(56),
            string_ids_off: u32at(60),
            type_ids_size: u32at(64),
            type_ids_off: u32at(68),
            proto_ids_size: u32at(72),
            proto_ids_off: u32at(76),
            field_ids_size: u32at(80),
            field_ids_off: u32at(84),
            method_ids_size: u32at(88),
            method_ids_off: u32at(92),
            class_defs_size: u32at(96),
            class_defs_off: u32at(100),
        };
        if header.file_size as usize > bytes.len() + 8 {
            return derr("header file_size exceeds input");
        }

        let mut string_offsets = Vec::with_capacity(header.string_ids_size as usize);
        for i in 0..header.string_ids_size {
            string_offsets.push(u32at((header.string_ids_off + i * 4) as usize));
        }

        let mut type_ids = Vec::with_capacity(header.type_ids_size as usize);
        for i in 0..header.type_ids_size {
            type_ids.push(u32at((header.type_ids_off + i * 4) as usize));
        }

        let mut protos = Vec::with_capacity(header.proto_ids_size as usize);
        for i in 0..header.proto_ids_size {
            let base = (header.proto_ids_off + i * 12) as usize;
            let shorty_idx = u32at(base);
            let return_type_idx = u32at(base + 4);
            let params_off = u32at(base + 8);
            let mut params = Vec::new();
            if params_off != 0 {
                // type_list: u32 size + u16 type indices
                let size = u32at(params_off as usize) as usize;
                for j in 0..size {
                    params.push(u16::from_le_bytes([
                        bytes[params_off as usize + 4 + j * 2],
                        bytes[params_off as usize + 5 + j * 2],
                    ]) as u32);
                }
            }
            protos.push(ProtoId { shorty_idx, return_type_idx, params });
        }

        let mut field_ids = Vec::with_capacity(header.field_ids_size as usize);
        for i in 0..header.field_ids_size {
            let base = (header.field_ids_off + i * 8) as usize;
            field_ids.push(FieldId {
                class_idx: u16::from_le_bytes([bytes[base], bytes[base + 1]]),
                type_idx: u16::from_le_bytes([bytes[base + 2], bytes[base + 3]]),
                name_idx: u32at(base + 4),
            });
        }

        let mut method_ids = Vec::with_capacity(header.method_ids_size as usize);
        for i in 0..header.method_ids_size {
            let base = (header.method_ids_off + i * 8) as usize;
            method_ids.push(MethodId {
                class_idx: u16::from_le_bytes([bytes[base], bytes[base + 1]]),
                proto_idx: u16::from_le_bytes([bytes[base + 2], bytes[base + 3]]),
                name_idx: u32at(base + 4),
            });
        }

        let mut class_defs = Vec::with_capacity(header.class_defs_size as usize);
        for i in 0..header.class_defs_size {
            let base = (header.class_defs_off + i * 32) as usize;
            let class_idx = u32at(base);
            let access_flags = u32at(base + 4);
            let superclass_idx = u32at(base + 8);
            let interfaces_off = u32at(base + 12);
            let source_file_idx = u32at(base + 16);
            let class_data_off = u32at(base + 24);
            let static_values_off = u32at(base + 28);
            let mut interfaces = Vec::new();
            if interfaces_off != 0 {
                // type_list: u32 size + u16 type indices
                let n = u32at(interfaces_off as usize) as usize;
                for j in 0..n {
                    interfaces.push(u16::from_le_bytes([
                        bytes[interfaces_off as usize + 4 + j * 2],
                        bytes[interfaces_off as usize + 5 + j * 2],
                    ]) as u32);
                }
            }
            let mut static_fields = Vec::new();
            let mut instance_fields = Vec::new();
            let mut direct_methods = Vec::new();
            let mut virtual_methods = Vec::new();
            if class_data_off != 0 {
                let mut p = class_data_off as usize;
                let rd = |p: &mut usize| -> u32 {
                    let (v, n) = uleb(&bytes, *p);
                    *p += n;
                    v
                };
                let static_count = rd(&mut p) as usize;
                let instance_count = rd(&mut p) as usize;
                let direct_count = rd(&mut p) as usize;
                let virtual_count = rd(&mut p) as usize;
                let mut idx = 0u32;
                for _ in 0..static_count {
                    idx += rd(&mut p);
                    let flags = rd(&mut p);
                    static_fields.push(EncodedField { field_idx: idx, access_flags: flags });
                }
                idx = 0;
                for _ in 0..instance_count {
                    idx += rd(&mut p);
                    let flags = rd(&mut p);
                    instance_fields.push(EncodedField { field_idx: idx, access_flags: flags });
                }
                idx = 0;
                for _ in 0..direct_count {
                    idx += rd(&mut p);
                    let flags = rd(&mut p);
                    let code_off = rd(&mut p);
                    let code = if code_off == 0 {
                        None
                    } else {
                        Some(parse_code(&bytes, code_off as usize)?)
                    };
                    direct_methods.push(EncodedMethod { method_idx: idx, access_flags: flags, code });
                }
                idx = 0;
                for _ in 0..virtual_count {
                    idx += rd(&mut p);
                    let flags = rd(&mut p);
                    let code_off = rd(&mut p);
                    let code = if code_off == 0 {
                        None
                    } else {
                        Some(parse_code(&bytes, code_off as usize)?)
                    };
                    virtual_methods.push(EncodedMethod { method_idx: idx, access_flags: flags, code });
                }
            }
            let mut static_values = Vec::new();
            if static_values_off != 0 {
                let mut p = static_values_off as usize;
                let (n, used) = uleb(&bytes, p);
                p += used;
                for _ in 0..n {
                    let (v, used) = values::parse_encoded_value(&bytes, p)?;
                    static_values.push(v);
                    p += used;
                }
            }
            class_defs.push(ClassDef {
                class_idx,
                access_flags,
                superclass_idx,
                interfaces,
                source_file: if source_file_idx == NO_INDEX { None } else { Some(source_file_idx) },
                static_fields,
                instance_fields,
                direct_methods,
                virtual_methods,
                static_values,
            });
        }


        let cache = (0..header.string_ids_size).map(|_| std::sync::OnceLock::new()).collect();
        Ok(Dex {
            bytes,
            header,
            string_offsets,
            string_cache: cache,
            type_ids,
            protos,
            field_ids,
            method_ids,
            class_defs,
        })
    }

    pub fn strings_count(&self) -> u32 {
        self.header.string_ids_size
    }

    /// Decode string `idx` (cached).
    pub fn string(&self, idx: u32) -> Result<&str, DexError> {
        if idx >= self.header.string_ids_size {
            return derr(format!("string idx {idx} out of range"));
        }
        let slot = &self.string_cache[idx as usize];
        let s = slot.get_or_init(|| {
            let off = self.string_offsets[idx as usize] as usize;
            let (utf16_len, used) = uleb(&self.bytes, off);
            let raw = &self.bytes[off + used..];
            decode_mutf8(raw, utf16_len as usize).into_boxed_str()
        });
        Ok(s.as_ref())
    }

    pub fn type_name(&self, idx: u32) -> Result<&str, DexError> {
        if idx == NO_INDEX {
            return Ok("");
        }
        if idx >= self.header.type_ids_size {
            return derr(format!("type idx {idx} out of range"));
        }
        let sidx = self.type_ids[idx as usize];
        self.string(sidx)
    }

    pub fn field_name(&self, idx: u32) -> Result<&str, DexError> {
        let f = self
            .field_ids
            .get(idx as usize)
            .ok_or_else(|| DexError(format!("field idx {idx} out of range")))?;
        self.string(f.name_idx)
    }

    pub fn method_name(&self, idx: u32) -> Result<&str, DexError> {
        let m = self
            .method_ids
            .get(idx as usize)
            .ok_or_else(|| DexError(format!("method idx {idx} out of range")))?;
        self.string(m.name_idx)
    }

    pub fn class_of_method(&self, idx: u32) -> Result<&str, DexError> {
        let m = self
            .method_ids
            .get(idx as usize)
            .ok_or_else(|| DexError(format!("method idx {idx} out of range")))?;
        self.type_name(m.class_idx as u32)
    }

    pub fn class_of_field(&self, idx: u32) -> Result<&str, DexError> {
        let f = self
            .field_ids
            .get(idx as usize)
            .ok_or_else(|| DexError(format!("field idx {idx} out of range")))?;
        self.type_name(f.class_idx as u32)
    }

    pub fn find_class(&self, descriptor: &str) -> Option<u32> {
        // linear scan is fine for the class counts in a single dex; a map of
        // decoded descriptors would cost more memory than it saves here.
        self.class_defs.iter().enumerate().find_map(|(i, cd)| {
            self.type_name(cd.class_idx).ok().filter(|n| *n == descriptor).map(|_| i as u32)
        })
    }

    /// All class descriptors in definition order.
    pub fn class_names(&self) -> Vec<String> {
        self.class_defs
            .iter()
            .filter_map(|c| self.type_name(c.class_idx).ok().map(|s| s.to_string()))
            .collect()
    }

    pub fn proto_of(&self, method_idx: u32) -> Option<&ProtoId> {
        self.method_ids.get(method_idx as usize).map(|m| &self.protos[m.proto_idx as usize])
    }
}

fn uleb(bytes: &[u8], mut pos: usize) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    let mut count = 0usize;
    loop {
        let b = *bytes.get(pos).unwrap_or(&0);
        pos += 1;
        count += 1;
        result |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            break;
        }
    }
    (result, count)
}

pub fn sleb(bytes: &[u8], mut pos: usize) -> (i32, usize) {
    let mut result: i32 = 0;
    let mut shift = 0u32;
    let mut count = 0usize;
    loop {
        let b = *bytes.get(pos).unwrap_or(&0);
        pos += 1;
        count += 1;
        result |= (((b & 0x7F) as u32) << shift) as i32;
        shift += 7;
        if b & 0x80 == 0 {
            if shift < 32 && (b & 0x40) != 0 {
                result |= (-1i32) << shift;
            }
            break;
        }
        if shift > 35 {
            break;
        }
    }
    (result, count)
}

fn parse_code(bytes: &[u8], off: usize) -> Result<CodeItem, DexError> {
    if off + 16 > bytes.len() {
        return derr("code item out of range");
    }
    let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let registers_size = u16at(off);
    let ins_size = u16at(off + 2);
    let outs_size = u16at(off + 4);
    let tries_size = u16at(off + 6);
    let insns_size = u32at(off + 12) as usize;
    let insns_off = off + 16;
    if insns_off + insns_size * 2 > bytes.len() {
        return derr("insns out of range");
    }
    let mut insns = Vec::with_capacity(insns_size);
    for i in 0..insns_size {
        insns.push(u16at(insns_off + i * 2));
    }
    let mut tries = Vec::new();
    let mut handler_map = Vec::new();
    let mut handlers = Vec::new();
    if tries_size > 0 {
        let mut t = insns_off + insns_size * 2;
        if insns_size % 2 == 1 {
            t += 2; // padding
        }
        for _ in 0..tries_size {
            if t + 8 > bytes.len() {
                return derr("try item out of range");
            }
            tries.push(TryItem {
                start_addr: u32at(t),
                insn_count: u16at(t + 4),
                handler_off: u16at(t + 6),
            });
            t += 8;
        }
        // encoded_catch_handler_list
        let list_start = t;
        let (n, used) = uleb(bytes, list_start);
        let mut hp = list_start + used;
        for i in 0..n {
            let off_in_list = (hp - list_start) as u16;
            let (sz, used) = sleb(bytes, hp);
            hp += used;
            let count = sz.unsigned_abs() as usize;
            let mut typed = Vec::with_capacity(count);
            for _ in 0..count {
                let (ty, u1) = uleb(bytes, hp);
                hp += u1;
                let (addr, u2) = uleb(bytes, hp);
                hp += u2;
                typed.push((ty, addr));
            }
            let catch_all = if sz <= 0 {
                let (a, u3) = uleb(bytes, hp);
                hp += u3;
                Some(a)
            } else {
                None
            };
            handler_map.push((off_in_list, i as usize));
            handlers.push(CatchHandler { typed, catch_all });
        }
    }
    Ok(CodeItem { registers_size, ins_size, outs_size, insns, tries, handler_map, handlers })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_dex_bytes() -> Option<Vec<u8>> {
        if let Ok(b) = std::fs::read("/root/genex-tui/android/build/classes.dex") {
            return Some(b);
        }
        let apk = std::fs::read("/root/genex-tui/android/build/genex-chat.apk").ok()?;
        let ar = litebox_zip::ZipArchive::open(&apk).ok()?;
        ar.read("classes.dex").ok()
    }

    #[test]
    fn parses_real_dex() {
        let Some(bytes) = real_dex_bytes() else {
            eprintln!("no real dex on host, skipping");
            return;
        };
        let dex = Dex::parse(bytes).unwrap();
        assert!(dex.header.magic.starts_with(b"dex\n"));
        assert!(!dex.class_defs.is_empty());
        let names = dex.class_names();
        assert!(
            names.iter().any(|n| n.starts_with("Lcom/") || n.starts_with("L")),
            "classes: {names:?}"
        );
        // every class must have a resolvable name
        for cd in &dex.class_defs {
            let n = dex.type_name(cd.class_idx).unwrap();
            assert!(n.starts_with('L') && n.ends_with(';'), "descriptor: {n}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Dex::parse(vec![0; 128]).is_err());
        assert!(Dex::parse(vec![b'd', b'e', b'x', b'\n', 0, 0, 0, 0]).is_err());
    }
}
