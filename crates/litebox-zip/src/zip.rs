//! ZIP archive reader/writer. No dependencies, no compression beyond what
//! `inflate` provides. APKs are plain ZIPs with deflate entries.

use crate::crc32::crc32;
use crate::inflate::inflate;
use std::collections::HashMap;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_EOCD64: u32 = 0x0606_4b50;
const SIG_LOC64: u32 = 0x0706_4b50;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub method: u16,
    pub crc: u32,
    pub local_header_offset: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub enum ZipError {
    NotFound(String),
    Corrupt(String),
    Unsupported(String),
}

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZipError::NotFound(n) => write!(f, "zip entry not found: {n}"),
            ZipError::Corrupt(m) => write!(f, "zip corrupt: {m}"),
            ZipError::Unsupported(m) => write!(f, "zip unsupported: {m}"),
        }
    }
}
impl std::error::Error for ZipError {}

impl From<crate::inflate::InflateError> for ZipError {
    fn from(e: crate::inflate::InflateError) -> Self {
        ZipError::Corrupt(e.0)
    }
}

fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_u64le(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

pub struct ZipArchive<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry>,
    by_name: HashMap<String, usize>,
}

impl<'a> ZipArchive<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self, ZipError> {
        let eocd = find_eocd(data).ok_or_else(|| {
            ZipError::Corrupt("end of central directory not found (not a zip?)".into())
        })?;
        let mut total_entries = rd_u16(data, eocd + 10) as u64;
        let mut cd_offset = rd_u32(data, eocd + 16) as u64;
        // ZIP64: EOCD may claim 0xFFFF / 0xFFFFFFFF
        if total_entries == 0xFFFF || cd_offset == 0xFFFF_FFFF {
            // find ZIP64 EOCD locator: signature right before EOCD (20 bytes)
            if eocd >= 20 && rd_u32(data, eocd - 20) == SIG_LOC64 {
                let eocd64_off = rd_u64le(data, eocd - 20 + 8) as usize;
                if eocd64_off + 56 <= data.len() && rd_u32(data, eocd64_off) == SIG_EOCD64 {
                    total_entries = rd_u64le(data, eocd64_off + 32);
                    cd_offset = rd_u64le(data, eocd64_off + 48);
                }
            }
        }
        if cd_offset as usize > data.len() {
            return Err(ZipError::Corrupt("central directory offset out of range".into()));
        }
        let mut entries = Vec::new();
        let mut by_name = HashMap::new();
        let mut p = cd_offset as usize;
        for i in 0..total_entries as usize {
            if p + 46 > data.len() || rd_u32(data, p) != SIG_CENTRAL {
                return Err(ZipError::Corrupt("bad central directory entry".into()));
            }
            let method = rd_u16(data, p + 10);
            let crc = rd_u32(data, p + 16);
            let mut csize = rd_u32(data, p + 20) as u64;
            let mut usize_ = rd_u32(data, p + 24) as u64;
            let mut lho = rd_u32(data, p + 42) as u64;
            let name_len = rd_u16(data, p + 28) as usize;
            let extra_len = rd_u16(data, p + 30) as usize;
            let comment_len = rd_u16(data, p + 32) as usize;
            if p + 46 + name_len + extra_len + comment_len > data.len() {
                return Err(ZipError::Corrupt("truncated central directory".into()));
            }
            let name = String::from_utf8_lossy(&data[p + 46..p + 46 + name_len]).into_owned();
            // ZIP64 extra field (id 0x0001)
            let mut q = p + 46 + name_len;
            let extra_end = q + extra_len;
            while q + 4 <= extra_end.min(data.len()) {
                let fid = rd_u16(data, q);
                let fsz = rd_u16(data, q + 2) as usize;
                let mut fq = q + 4;
                if fid == 0x0001 {
                    if usize_ == 0xFFFF_FFFF && fq + 8 <= q + 4 + fsz {
                        usize_ = rd_u64le(data, fq);
                        fq += 8;
                    }
                    if csize == 0xFFFF_FFFF && fq + 8 <= q + 4 + fsz {
                        csize = rd_u64le(data, fq);
                        fq += 8;
                    }
                    if lho == 0xFFFF_FFFF && fq + 8 <= q + 4 + fsz {
                        lho = rd_u64le(data, fq);
                    }
                }
                q += 4 + fsz;
            }
            let is_dir = name.ends_with('/');
            if method != METHOD_STORED && method != METHOD_DEFLATE && !is_dir {
                return Err(ZipError::Unsupported(format!(
                    "entry {name}: compression method {method}"
                )));
            }
            by_name.insert(name.clone(), i);
            entries.push(ZipEntry {
                name,
                compressed_size: csize,
                uncompressed_size: usize_,
                method,
                crc,
                local_header_offset: lho,
                is_dir,
            });
            p += 46 + name_len + extra_len + comment_len;
        }
        Ok(ZipArchive { data, entries, by_name })
    }

    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    pub fn entry_names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    pub fn find(&self, name: &str) -> Option<&ZipEntry> {
        self.by_name.get(name).map(|&i| &self.entries[i])
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>, ZipError> {
        let idx = self
            .by_name
            .get(name)
            .copied()
            .ok_or_else(|| ZipError::NotFound(name.to_string()))?;
        self.read_at(idx)
    }

    pub fn read_entry(&self, entry: &ZipEntry) -> Result<Vec<u8>, ZipError> {
        let idx = self
            .by_name
            .get(&entry.name)
            .copied()
            .ok_or_else(|| ZipError::NotFound(entry.name.clone()))?;
        self.read_at(idx)
    }

    fn read_at(&self, idx: usize) -> Result<Vec<u8>, ZipError> {
        let e = &self.entries[idx];
        if e.is_dir {
            return Ok(Vec::new());
        }
        let lho = e.local_header_offset as usize;
        if lho + 30 > self.data.len() || rd_u32(self.data, lho) != SIG_LOCAL {
            return Err(ZipError::Corrupt(format!("bad local header for {}", e.name)));
        }
        let name_len = rd_u16(self.data, lho + 26) as usize;
        let extra_len = rd_u16(self.data, lho + 28) as usize;
        let data_start = lho + 30 + name_len + extra_len;
        let csize = e.compressed_size as usize;
        if data_start + csize > self.data.len() {
            return Err(ZipError::Corrupt(format!("truncated data for {}", e.name)));
        }
        let raw = &self.data[data_start..data_start + csize];
        let out = match e.method {
            METHOD_STORED => raw.to_vec(),
            METHOD_DEFLATE => inflate(raw, e.uncompressed_size as usize)?.0,
            m => {
                return Err(ZipError::Unsupported(format!(
                    "method {m} for {}",
                    e.name
                )))
            }
        };
        if out.len() as u64 != e.uncompressed_size {
            return Err(ZipError::Corrupt(format!(
                "size mismatch for {}: got {}, expected {}",
                e.name,
                out.len(),
                e.uncompressed_size
            )));
        }
        let crc = crc32(&out);
        if crc != e.crc {
            return Err(ZipError::Corrupt(format!(
                "crc mismatch for {}: got {crc:08x}, expected {:08x}",
                e.name, e.crc
            )));
        }
        Ok(out)
    }
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let max_scan = data.len().saturating_sub(22).min(22 + 65_535);
    let mut i = data.len() - 22;
    loop {
        if rd_u32(data, i) == SIG_EOCD {
            let comment_len = rd_u16(data, i + 20) as usize;
            if i + 22 + comment_len == data.len() {
                return Some(i);
            }
        }
        if i == data.len() - max_scan || max_scan == data.len() - 22 && false {
            // comment may be shorter than max scan; keep scanning to the first match
        }
        if i == 0 {
            return None;
        }
        i -= 1;
        if data.len() - i > max_scan && rd_u32(data, i + 1) != SIG_EOCD {
            // we have scanned beyond max_scan without a full-comment match
            break;
        }
    }
    // fallback: accept first EOCD with sane comment length even if trailing bytes exist
    let mut j = data.len() - 22;
    while j + 1 > data.len() - max_scan {
        if rd_u32(data, j) == SIG_EOCD {
            return Some(j);
        }
        j -= 1;
    }
    None
}

/// Writes ZIP archives with STORED entries (no compression). Enough for APKs
/// we assemble in tests, PNG assets, and round-trip tooling.
pub struct ZipWriter {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
    offsets: Vec<u32>,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter { out: Vec::new(), central: Vec::new(), count: 0, offsets: Vec::new() }
    }

    pub fn add(&mut self, name: &str, data: &[u8]) {
        let crc = crc32(data);
        self.offsets.push(self.out.len() as u32);
        // local header
        self.out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        self.out.extend_from_slice(&[0x14, 0x00]); // version 2.0
        self.out.extend_from_slice(&[0x08, 0x00]); // flags: UTF-8 name
        self.out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        self.out.extend_from_slice(&[0, 0]); // time
        self.out.extend_from_slice(&[0x21, 0]); // date 1980-01-01
        self.out.extend_from_slice(&crc.to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(data);

        // central record
        self.central.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        self.central.extend_from_slice(&[0x14, 0x00]); // version made by
        self.central.extend_from_slice(&[0x14, 0x00]); // version needed
        self.central.extend_from_slice(&[0x08, 0x00]); // flags
        self.central.extend_from_slice(&METHOD_STORED.to_le_bytes());
        self.central.extend_from_slice(&[0, 0, 0x21, 0]); // time+date
        self.central.extend_from_slice(&crc.to_le_bytes());
        self.central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        self.central.extend_from_slice(&0u16.to_le_bytes()); // extra
        self.central.extend_from_slice(&0u16.to_le_bytes()); // comment
        self.central.extend_from_slice(&0u16.to_le_bytes()); // disk start
        self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        self.central.extend_from_slice(&self.offsets[self.count as usize].to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_off = self.out.len() as u32;
        let cd_size = self.central.len() as u32;
        self.out.extend_from_slice(&self.central);
        self.out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // disk
        self.out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&cd_size.to_le_bytes());
        self.out.extend_from_slice(&cd_off.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_reader_roundtrip() {
        let mut w = ZipWriter::new();
        w.add("AndroidManifest.xml", b"\x03\x00\x0c\x00fake axml");
        w.add("classes.dex", &vec![0xABu8; 50_000]);
        w.add("res/x.txt", b"resource");
        let zip = w.finish();
        let ar = ZipArchive::open(&zip).unwrap();
        assert_eq!(ar.entry_names(), vec!["AndroidManifest.xml", "classes.dex", "res/x.txt"]);
        assert_eq!(ar.read("res/x.txt").unwrap(), b"resource");
        assert_eq!(ar.read("classes.dex").unwrap(), vec![0xABu8; 50_000]);
        assert!(matches!(ar.read("missing"), Err(ZipError::NotFound(_))));
    }

    #[test]
    fn reads_real_apk_on_disk() {
        let path = "/root/genex-tui/android/build/genex-chat.apk";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let data = std::fs::read(path).unwrap();
        let ar = ZipArchive::open(&data).unwrap();
        assert!(ar.find("classes.dex").is_some(), "real apk must have classes.dex");
        assert!(ar.find("AndroidManifest.xml").is_some());
        let dex = ar.read("classes.dex").unwrap();
        assert_eq!(&dex[..3], b"dex");
    }

    #[test]
    fn inflate_consumed_position_is_correct() {
        // stored deflate block of 5 bytes wrapped manually
        let mut raw = vec![0x01, 0x05, 0x00, 0xFA, 0xFF];
        raw.extend_from_slice(b"12345");
        let (out, consumed) = inflate(&raw, 5).unwrap();
        assert_eq!(out, b"12345");
        assert_eq!(consumed, raw.len());
    }
}
