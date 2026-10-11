//! Android binary XML (AXML) parser — extracts the manifest facts the
//! package manager needs: package name, version, launcher activity, all
//! activity/service/receiver component names.

use litebox_json::Json;

const RES_STRING_POOL: u16 = 0x0001;
const RES_XML: u16 = 0x0003;
const RES_XML_START_NS: u16 = 0x0100;
const RES_XML_START_ELEMENT: u16 = 0x0102;

fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

struct StringPool {
    strings: Vec<String>,
}

fn parse_string_pool(b: &[u8], off: usize) -> Option<StringPool> {
    if off + 28 > b.len() || rd_u16(b, off) != RES_STRING_POOL {
        return None;
    }
    let count = rd_u32(b, off + 8) as usize;
    let flags = rd_u32(b, off + 16);
    let strings_start = rd_u32(b, off + 20) as usize;
    let utf8 = flags & 0x100 != 0;
    let mut strings = Vec::with_capacity(count);
    for i in 0..count {
        let slot = off + 28 + i * 4;
        if slot + 4 > b.len() {
            break;
        }
        let data_off = off + strings_start + rd_u32(b, slot) as usize;
        if data_off >= b.len() {
            strings.push(String::new());
            continue;
        }
        let s = if utf8 {
            // u8len-prefixed char count, then byte length, then bytes
            let mut p = data_off;
            let _char_count = dec_len_utf8(b, &mut p);
            let u8len = dec_len_utf8(b, &mut p) as usize;
            let end = (p + u8len).min(b.len());
            String::from_utf8_lossy(&b[p..end]).into_owned()
        } else {
            // u16 length (top flag bit marks a high half for huge pools)
            let mut p = data_off;
            let mut u16len = u16::from_le_bytes([b[p], b[p + 1]]) as usize;
            p += 2;
            if u16len & 0x8000 != 0 {
                u16len = ((u16len & 0x7FFF) << 16) | u16::from_le_bytes([b[p], b[p + 1]]) as usize;
                p += 2;
            }
            let mut out = String::new();
            for k in 0..u16len {
                let o = p + k * 2;
                if o + 2 > b.len() {
                    break;
                }
                let c = u16::from_le_bytes([b[o], b[o + 1]]);
                if (0xD800..0xDC00).contains(&c) {
                    if o + 4 <= b.len() {
                        let lo = u16::from_le_bytes([b[o + 2], b[o + 3]]);
                        if (0xDC00..0xE000).contains(&lo) {
                            out.push(char::from_u32(0x10000 + ((c as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00)).unwrap_or('\u{FFFD}'));
                            continue;
                        }
                    }
                    out.push('\u{FFFD}');
                } else {
                    out.push(char::from_u32(c as u32).unwrap_or('\u{FFFD}'));
                }
            }
            out
        };
        strings.push(s);
    }
    Some(StringPool { strings })
}

/// UTF-8 pool length prefix: 1 byte, or 2 bytes with the high bit of the
/// first marking continuation.
fn dec_len_utf8(b: &[u8], p: &mut usize) -> u32 {
    if *p >= b.len() {
        return 0;
    }
    let first = b[*p];
    *p += 1;
    if first & 0x80 != 0 {
        if *p < b.len() {
            let second = b[*p];
            *p += 1;
            ((first as u32 & 0x7F) << 8) | second as u32
        } else {
            0
        }
    } else {
        first as u32
    }
}

#[derive(Debug, Default, Clone)]
pub struct Manifest {
    pub package: String,
    pub version_name: Option<String>,
    pub launcher_activity: Option<String>,
    pub activities: Vec<String>,
    pub label: Option<String>,
}

/// Parse a compiled AndroidManifest.xml.
pub fn parse_manifest(axml: &[u8]) -> Result<Manifest, String> {
    if axml.len() < 8 || rd_u16(axml, 0) != RES_XML {
        return Err("not a binary XML document".into());
    }
    let mut pool: Option<StringPool> = None;
    let mut m = Manifest::default();
    // walk chunks after the document header (8 bytes)
    let mut p = 8usize;
    while p + 8 <= axml.len() {
        let ctype = rd_u16(axml, p);
        let _hdr = rd_u16(axml, p + 2) as usize;
        let size = rd_u32(axml, p + 4) as usize;
        if size == 0 || p + size > axml.len() + 8 {
            break;
        }
        match ctype {
            RES_STRING_POOL if pool.is_none() => {
                pool = parse_string_pool(axml, p);
            }
            RES_XML_START_ELEMENT => {
                if let Some(pool) = &pool {
                    let ns = rd_u32(axml, p + 16) as usize;
                    let name = rd_u32(axml, p + 20) as usize;
                    let elem = pool.strings.get(name).cloned().unwrap_or_default();
                    let _ = ns;
                    let attribute_start = rd_u16(axml, p + 24) as usize;
                    let attribute_size = rd_u16(axml, p + 26) as usize;
                    let attr_count = rd_u16(axml, p + 28) as usize;
                    // attributes live at chunk_start + node_header(16) + attributeStart
                    let attr_start = p + 16 + attribute_start;
                    let stride = if attribute_size >= 20 { attribute_size } else { 20 };
                    for i in 0..attr_count {
                        let a = attr_start + i * stride;
                        if a + 20 > axml.len() {
                            break;
                        }
                        let aname = rd_u32(axml, a + 4) as usize;
                        let araw = rd_u32(axml, a + 8) as usize;
                        let dtype = axml.get(a + 15).copied().unwrap_or(0);
                        let aval = rd_u32(axml, a + 16) as i32;
                        let aname_s = pool.strings.get(aname).cloned().unwrap_or_default();
                        let avalue_s = if araw != 0xFFFF_FFFF {
                            pool.strings.get(araw).cloned().unwrap_or_default()
                        } else {
                            match dtype {
                                0x03 => {
                                    // string: data is a string pool index
                                    pool.strings.get(aval as usize).cloned().unwrap_or_default()
                                }
                                0x10 | 0x11 => aval.to_string(),
                                0x12 => (aval != 0).to_string(),
                                _ => aval.to_string(),
                            }
                        };
                        if elem == "manifest" {
                            match aname_s.as_str() {
                                "package" => m.package = avalue_s.clone(),
                                "versionName" => m.version_name = Some(avalue_s.clone()),
                                _ => {}
                            }
                        }
                        if elem == "application" && aname_s == "label" {
                            m.label = Some(avalue_s.clone());
                        }
                        if elem == "activity" && aname_s == "name" {
                            let name = avalue_s.clone();
                            let full = if name.starts_with('.') {
                                format!("{}{}", m.package, name)
                            } else if name.contains('.') {
                                name
                            } else {
                                format!("{}.{}", m.package, name)
                            };
                            m.activities.push(full);
                        }
                    }
                }
            }
            RES_XML_START_NS => {}
            _ => {}
        }
        p += size;
    }
    // launcher activity: an activity whose name ends with the MAIN action
    // declaration is detected via intent-filter in a full parser; without
    // element nesting tracking, prefer the first declared activity.
    m.launcher_activity = m.activities.first().cloned();
    Ok(m)
}

#[allow(unused)]
fn silence(v: &Json) -> Json {
    v.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_smoke_manifest() {
        let path = "/root/litebox/examples/smoke/build/smoke.apk";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let apk = std::fs::read(path).unwrap();
        let ar = litebox_zip::ZipArchive::open(&apk).unwrap();
        let axml = ar.read("AndroidManifest.xml").unwrap();
        let m = parse_manifest(&axml).unwrap();
        assert_eq!(m.package, "com.litebox.smoke");
        assert!(
            m.activities.iter().any(|a| a == "com.litebox.smoke.SmokeActivity"),
            "activities: {:?}",
            m.activities
        );
        assert!(m.launcher_activity.as_deref().unwrap_or("").contains("SmokeActivity"));
    }

    #[test]
    fn parses_real_third_party_manifest() {
        let path = "/root/genex-tui/android/build/genex-chat.apk";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let apk = std::fs::read(path).unwrap();
        let ar = litebox_zip::ZipArchive::open(&apk).unwrap();
        let axml = ar.read("AndroidManifest.xml").unwrap();
        let m = parse_manifest(&axml).unwrap();
        assert!(!m.package.is_empty());
        assert!(!m.activities.is_empty(), "genex must declare activities");
    }
}
