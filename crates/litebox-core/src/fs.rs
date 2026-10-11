//! Per-app sandbox filesystem. Apps see a private POSIX-flavored tree; nothing
//! escapes it. Paths are normalized, directories materialize on write, and
//! every byte lives in VM memory (snapshot-friendly).

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    /// virtual milliseconds since boot
    pub mtime_ms: u64,
}

#[derive(Debug, Clone)]
pub struct FileData {
    pub bytes: Vec<u8>,
    pub mtime_ms: u64,
}

#[derive(Debug, Clone)]
pub struct SandboxFs {
    files: BTreeMap<String, FileData>,
    dirs: Vec<String>,
    dirs_sorted: bool,
    now_ms: u64,
}

fn normalize(path: &str) -> String {
    let mut out = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if joined.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn parent_of(p: &str) -> Option<String> {
    match p.rfind('/') {
        Some(0) | None => None,
        Some(i) => Some(p[..i].to_string()),
    }
}

#[derive(Debug)]
pub enum FsError {
    NotFound(String),
    IsDirectory(String),
    NotDirectory(String),
    OutsideSandbox(String),
}

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FsError::NotFound(p) => write!(f, "no such file: {p}"),
            FsError::IsDirectory(p) => write!(f, "is a directory: {p}"),
            FsError::NotDirectory(p) => write!(f, "not a directory: {p}"),
            FsError::OutsideSandbox(p) => write!(f, "path outside sandbox: {p}"),
        }
    }
}
impl std::error::Error for FsError {}

impl SandboxFs {
    pub fn new() -> Self {
        SandboxFs {
            files: BTreeMap::new(),
            dirs: vec!["/".to_string()],
            dirs_sorted: true,
            now_ms: 0,
        }
    }

    pub fn set_now_ms(&mut self, ms: u64) {
        self.now_ms = ms;
    }

    fn check(&self, path: &str) -> Result<String, FsError> {
        let n = normalize(path);
        if !n.starts_with('/') {
            return Err(FsError::OutsideSandbox(n));
        }
        Ok(n)
    }

    fn ensure_parents(&mut self, path: &str) {
        let mut cur = path.to_string();
        while let Some(parent) = parent_of(&cur) {
            if parent == "/" {
                break;
            }
            if !self.dirs.iter().any(|d| d == &parent) {
                self.dirs.push(parent.clone());
                self.dirs_sorted = false;
            }
            cur = parent;
        }
    }

    pub fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        let p = self.check(path)?;
        if self.dirs.iter().any(|d| d == &p) {
            return Err(FsError::IsDirectory(p));
        }
        self.ensure_parents(&p);
        self.files.insert(p, FileData { bytes: bytes.to_vec(), mtime_ms: self.now_ms });
        Ok(())
    }

    pub fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        let mut existing = self.read(path).unwrap_or_default();
        existing.extend_from_slice(bytes);
        self.write(path, &existing)
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, FsError> {
        let p = self.check(path)?;
        if self.dirs.iter().any(|d| d == &p) {
            return Err(FsError::IsDirectory(p));
        }
        self.files
            .get(&p)
            .map(|f| f.bytes.clone())
            .ok_or(FsError::NotFound(p))
    }

    pub fn exists(&self, path: &str) -> bool {
        match self.check(path) {
            Ok(p) => self.files.contains_key(&p) || self.dirs.iter().any(|d| d == &p),
            Err(_) => false,
        }
    }

    pub fn is_file(&self, path: &str) -> bool {
        self.check(path).is_ok_and(|p| self.files.contains_key(&p))
    }

    pub fn is_dir(&self, path: &str) -> bool {
        self.check(path).is_ok_and(|p| self.dirs.iter().any(|d| d == &p))
    }

    pub fn stat(&self, path: &str) -> Option<FileMeta> {
        let p = normalize(path);
        self.files.get(&p).map(|f| FileMeta { size: f.bytes.len() as u64, mtime_ms: f.mtime_ms })
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        let p = self.check(path)?;
        if self.files.contains_key(&p) {
            return Err(FsError::NotDirectory(p));
        }
        self.ensure_parents(&p);
        if !self.dirs.iter().any(|d| d == &p) {
            self.dirs.push(p);
            self.dirs_sorted = false;
        }
        Ok(())
    }

    pub fn mkdirs(&mut self, path: &str) -> Result<(), FsError> {
        self.mkdir(path)
    }

    pub fn delete(&mut self, path: &str) -> Result<bool, FsError> {
        let p = self.check(path)?;
        if self.dirs.iter().any(|d| d == &p) {
            return Err(FsError::IsDirectory(p));
        }
        Ok(self.files.remove(&p).is_some())
    }

    pub fn delete_recursive(&mut self, path: &str) -> Result<(), FsError> {
        let p = self.check(path)?;
        self.files.retain(|k, _| !(k == &p || k.starts_with(&format!("{p}/"))));
        self.dirs.retain(|k| !(k == &p || k.starts_with(&format!("{p}/"))));
        Ok(())
    }

    /// Immediate children names of a directory (files and dirs).
    pub fn list(&mut self, path: &str) -> Result<Vec<String>, FsError> {
        let p = self.check(path)?;
        if !self.dirs.iter().any(|d| d == &p) {
            return Err(FsError::NotDirectory(p));
        }
        if !self.dirs_sorted {
            self.dirs.sort();
            self.dirs_sorted = true;
        }
        let prefix = if p == "/" { "/" } else { &format!("{p}/") };
        let mut out: Vec<String> = self
            .files
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter(|(k, _)| !k[prefix.len()..].contains('/'))
            .map(|(k, _)| k[prefix.len()..].to_string())
            .collect();
        for d in self.dirs.iter() {
            if d.as_str() != "/" && d.starts_with(prefix) && !d[prefix.len()..].contains('/') {
                let name = d[prefix.len()..].to_string();
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|f| f.bytes.len() as u64).sum()
    }

    /// Direct iteration for snapshotting.
    pub fn iter_files(&self) -> impl Iterator<Item = (&String, &FileData)> {
        self.files.iter()
    }

    pub fn restore_files(&mut self, files: BTreeMap<String, FileData>) {
        self.files = files;
        self.dirs_sorted = false;
    }
}

impl Default for SandboxFs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_roundtrip() {
        let mut fs = SandboxFs::new();
        fs.write("/data/data/com.x/files/a.txt", b"hello").unwrap();
        assert_eq!(fs.read("/data/data/com.x/files/a.txt").unwrap(), b"hello");
        assert!(fs.exists("/data/data/com.x/files/a.txt"));
        assert!(fs.is_dir("/data/data/com.x/files"));
        assert_eq!(fs.stat("/data/data/com.x/files/a.txt").unwrap().size, 5);
    }

    #[test]
    fn normalizes_paths() {
        let mut fs = SandboxFs::new();
        fs.write("/data/x/./y/../z.txt", b"1").unwrap();
        assert_eq!(fs.read("/data/x/z.txt").unwrap(), b"1");
        assert!(!fs.exists("/data/x/y"));
    }

    #[test]
    fn list_and_delete() {
        let mut fs = SandboxFs::new();
        fs.write("/d/b.txt", b"1").unwrap();
        fs.write("/d/a.txt", b"2").unwrap();
        fs.write("/d/sub/c.txt", b"3").unwrap();
        let names = fs.list("/d").unwrap();
        assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
        assert!(fs.delete("/d/b.txt").unwrap());
        assert!(!fs.exists("/d/b.txt"));
        assert!(matches!(fs.delete("/d"), Err(FsError::IsDirectory(_))));
        fs.delete_recursive("/d/sub").unwrap();
        assert!(!fs.exists("/d/sub/c.txt"));
    }

    #[test]
    fn append_and_rejects() {
        let mut fs = SandboxFs::new();
        fs.write("/f", b"a").unwrap();
        fs.append("/f", b"b").unwrap();
        assert_eq!(fs.read("/f").unwrap(), b"ab");
        assert!(matches!(fs.read("/f/x"), Err(FsError::NotFound(_))));
        assert!(matches!(fs.read("/nope"), Err(FsError::NotFound(_))));
        fs.mkdir("/dd").unwrap();
        assert!(matches!(fs.write("/dd", b"x"), Err(FsError::IsDirectory(_))));
    }
}
