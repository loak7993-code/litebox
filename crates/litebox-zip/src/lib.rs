//! Dependency-free ZIP + DEFLATE.
//! Reader: local headers, central directory, ZIP64-lite, stored + deflate,
//! CRC32 verification. Writer: stored entries (valid ZIP/PNG-compatible zlib).

pub mod crc32;
pub mod inflate;
pub mod zip;

pub use crc32::{adler32, crc32};
pub use zip::ZipError;
pub use zip::{ZipArchive, ZipEntry, ZipWriter};
