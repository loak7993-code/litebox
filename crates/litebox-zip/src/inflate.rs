//! Full DEFLATE (RFC 1951) decoder and zlib (RFC 1950) wrapper.
//! Ported from the format spec directly: bit reader LSB-first, block types
//! 00/01/10, canonical Huffman decode, 32 KiB LZ77 window.

pub const MAX_WINDOW: usize = 32768;

#[derive(Debug)]
pub struct InflateError(pub String);

impl std::fmt::Display for InflateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deflate error: {}", self.0)
    }
}
impl std::error::Error for InflateError {}

fn err<T>(msg: &str) -> Result<T, InflateError> {
    Err(InflateError(msg.to_string()))
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u64,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, bitbuf: 0, bitcnt: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.bitcnt <= 56 {
            let byte = self.data.get(self.pos).copied().unwrap_or(0);
            self.pos += 1;
            self.bitbuf |= (byte as u64) << self.bitcnt;
            self.bitcnt += 8;
        }
    }

    /// Read up to 25 bits. Reads past end yield zeros (stream end handled by
    /// caller by checking consumed position), matching tolerant decoders.
    #[inline]
    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.refill();
        let v = (self.bitbuf & ((1u64 << n) - 1)) as u32;
        self.bitbuf >>= n;
        self.bitcnt -= n;
        v
    }

    fn align_byte(&mut self) {
        let drop = self.bitcnt % 8;
        self.bitbuf >>= drop;
        self.bitcnt -= drop;
    }

    fn consumed_bytes(&self) -> usize {
        self.pos - (self.bitcnt / 8) as usize
    }
}

/// Canonical Huffman decoder built from per-symbol code lengths.
struct Huffman {
    counts: [u16; 16],     // number of codes of each bit length
    symbols: Vec<u16>,     // symbols sorted by (length, symbol)
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, InflateError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l > 15 {
                return err("invalid code length");
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // over-subscribed or incomplete check
        let mut left: i32 = 1;
        for &cnt in counts[1..16].iter() {
            left <<= 1;
            left -= cnt as i32;
            if left < 0 {
                return err("over-subscribed huffman code");
            }
        }
        let mut offs = [0u16; 16];
        for l in 1..15usize {
            offs[l + 1] = offs[l] + counts[l];
        }
        let mut symbols = vec![0u16; lengths.iter().filter(|&&l| l != 0).count()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    /// Decode one symbol.
    #[inline]
    fn decode(&self, br: &mut BitReader) -> Result<u16, InflateError> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.bits(1) as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        err("invalid huffman code")
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

/// Inflate a raw DEFLATE stream. Returns decompressed bytes and bytes consumed.
pub fn inflate(input: &[u8], size_hint: usize) -> Result<(Vec<u8>, usize), InflateError> {
    let mut br = BitReader::new(input);
    let mut out: Vec<u8> = Vec::with_capacity(size_hint.max(64));

    loop {
        let bfinal = br.bits(1);
        let btype = br.bits(2);
        match btype {
            0 => {
                br.align_byte();
                let len = br.bits(16) as usize;
                let nlen = br.bits(16) as usize;
                if len ^ 0xFFFF != nlen {
                    return err("stored block length check failed");
                }
                let start = br.consumed_bytes();
                if start + len > input.len() {
                    return err("stored block exceeds input");
                }
                out.extend_from_slice(&input[start..start + len]);
                br = BitReader::new(input);
                br.pos = start + len;
            }
            1 => {
                // fixed Huffman
                let mut lit_lengths = [0u8; 288];
                for (i, l) in lit_lengths.iter_mut().enumerate() {
                    *l = if i < 144 {
                        8
                    } else if i < 256 {
                        9
                    } else if i < 280 {
                        7
                    } else {
                        8
                    };
                }
                let dist_lengths = [5u8; 30];
                let lit = Huffman::new(&lit_lengths)?;
                let dist = Huffman::new(&dist_lengths)?;
                inflate_block(&mut br, &lit, &dist, &mut out)?;
            }
            2 => {
                let hlit = br.bits(5) as usize + 257;
                let hdist = br.bits(5) as usize + 1;
                let hclen = br.bits(4) as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut cl_lengths = [0u8; 19];
                for i in 0..hclen {
                    cl_lengths[ORDER[i]] = br.bits(3) as u8;
                }
                let cl = Huffman::new(&cl_lengths)?;
                let mut lengths = vec![0u8; hlit + hdist];
                let mut i = 0usize;
                while i < hlit + hdist {
                    let sym = cl.decode(&mut br)?;
                    match sym {
                        0..=15 => {
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            if i == 0 {
                                return err("repeat with no previous length");
                            }
                            let prev = lengths[i - 1];
                            let rep = 3 + br.bits(2) as usize;
                            for _ in 0..rep {
                                if i >= lengths.len() {
                                    return err("length repeat overflow");
                                }
                                lengths[i] = prev;
                                i += 1;
                            }
                        }
                        17 => {
                            let rep = 3 + br.bits(3) as usize;
                            i += rep;
                            if i > lengths.len() {
                                return err("zero repeat overflow");
                            }
                        }
                        18 => {
                            let rep = 11 + br.bits(7) as usize;
                            i += rep;
                            if i > lengths.len() {
                                return err("zero repeat overflow");
                            }
                        }
                        _ => return err("invalid code length symbol"),
                    }
                }
                if lengths[hlit..].iter().all(|&l| l == 0) && hdist == 1 {
                    return err("missing distance codes");
                }
                let lit = Huffman::new(&lengths[..hlit])?;
                let dist = Huffman::new(&lengths[hlit..])?;
                inflate_block(&mut br, &lit, &dist, &mut out)?;
            }
            _ => return err("reserved block type"),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok((out, br.consumed_bytes()))
}

fn inflate_block(
    br: &mut BitReader,
    lit: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
) -> Result<(), InflateError> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let li = (sym - 257) as usize;
                let len = LEN_BASE[li] as usize + br.bits(LEN_EXTRA[li] as u32) as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym >= 30 {
                    return err("invalid distance symbol");
                }
                let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32) as usize;
                if distance > out.len() {
                    return err("distance beyond output start");
                }
                if distance >= len {
                    let start = out.len() - distance;
                    out.extend_from_within(start..start + len);
                } else {
                    // overlapping copy: byte-by-byte semantics
                    let start = out.len() - distance;
                    for k in 0..len {
                        let b = out[start + k];
                        out.push(b);
                    }
                }
            }
            _ => return err("invalid literal/length symbol"),
        }
    }
}

/// Decompress a zlib (RFC 1950) stream.
pub fn zlib_decompress(input: &[u8], size_hint: usize) -> Result<Vec<u8>, InflateError> {
    if input.len() < 6 {
        return err("zlib stream too short");
    }
    let cmf = input[0];
    let flg = input[1];
    if cmf & 0x0F != 8 {
        return err("only CM=8 (deflate) supported");
    }
    if !((cmf as u16) << 8 | flg as u16).is_multiple_of(31) {
        return err("zlib header check failed");
    }
    if flg & 0x20 != 0 {
        return err("preset dictionary not supported");
    }
    let (raw, _consumed) = inflate(&input[2..], size_hint)?;
    let trailer = &input[input.len() - 4..];
    let expect = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let got = crate::adler32(&raw);
    if expect != got {
        return err("adler32 mismatch");
    }
    Ok(raw)
}

/// Compress with zlib "stored" blocks (no compression, valid for ZIP/PNG).
pub fn zlib_compress_stored(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + input.len() / 65535 * 5 + 16);
    out.push(0x78); // CM=8, CINFO=7
    out.push(0x01); // FLEVEL=0, FDICT=0, FCHECK such that (0x78<<8|0x01)%31==0
    let mut chunks = input.chunks(65535).peekable();
    if chunks.peek().is_none() {
        out.push(0x01);
        out.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(if last { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crate::adler32(input).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let comp = zlib_compress_stored(data);
        let out = zlib_decompress(&comp, data.len()).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn stored_roundtrip() {
        roundtrip(b"hello world");
        roundtrip(&vec![0u8; 200_000]);
        roundtrip(&(0..70_000).map(|i| (i % 251) as u8).collect::<Vec<u8>>());
        roundtrip(b"");
    }

    #[test]
    fn golden_inflate_fixed_block() {
        // Hand-built fixed-Huffman block encoding the literals "AB".
        // Canonical fixed table: symbols 0-143 are 8 bits, codes 0x30-0xBF.
        let mut acc: u32 = 0;
        let mut nbits: u32 = 0;
        let mut out_bytes = Vec::new();
        let push = |code: u16, len: u32, acc: &mut u32, nbits: &mut u32, out: &mut Vec<u8>| {
            for i in (0..len).rev() {
                let bit = ((code >> i) & 1) as u32;
                *acc |= bit << *nbits;
                *nbits += 1;
                if *nbits == 8 {
                    out.push(*acc as u8);
                    *acc = 0;
                    *nbits = 0;
                }
            }
        };
        // header: final=1 (1 bit), type=01 (2 bits) -> bits LSB first: 1,1,0
        push(1, 1, &mut acc, &mut nbits, &mut out_bytes); // BFINAL
        push(1, 1, &mut acc, &mut nbits, &mut out_bytes); // BTYPE bit0
        push(0, 1, &mut acc, &mut nbits, &mut out_bytes); // BTYPE bit1
        // 'A' fixed: code 0x30+0x41=0x71, 8 bits
        push(0x71, 8, &mut acc, &mut nbits, &mut out_bytes);
        // 'B' fixed: code 0x30+0x42=0x72
        push(0x72, 8, &mut acc, &mut nbits, &mut out_bytes);
        // end of block: 7 bits, code 0
        push(0, 7, &mut acc, &mut nbits, &mut out_bytes);
        if nbits > 0 {
            out_bytes.push(acc as u8);
        }
        let (out, _consumed) = inflate(&out_bytes, 2).unwrap();
        assert_eq!(out, b"AB");
    }

    #[test]
    fn overlapping_back_reference() {
        // fixed-block stream: literal 'X', 'X', then <len symbol 258 = 4, dist 1>,
        // exercises the overlapping LZ77 copy path.
        let mut acc: u32 = 0;
        let mut nbits: u32 = 0;
        let mut out_bytes = Vec::new();
        let push = |code: u16, len: u32, acc: &mut u32, nbits: &mut u32, out: &mut Vec<u8>| {
            for i in (0..len).rev() {
                let bit = ((code >> i) & 1) as u32;
                *acc |= bit << *nbits;
                *nbits += 1;
                if *nbits == 8 {
                    out.push(*acc as u8);
                    *acc = 0;
                    *nbits = 0;
                }
            }
        };
        push(1, 1, &mut acc, &mut nbits, &mut out_bytes); // BFINAL
        push(1, 1, &mut acc, &mut nbits, &mut out_bytes); // BTYPE bit0
        push(0, 1, &mut acc, &mut nbits, &mut out_bytes); // BTYPE bit1
        push(0x30 + 0x58, 8, &mut acc, &mut nbits, &mut out_bytes); // 'X'
        push(0x30 + 0x58, 8, &mut acc, &mut nbits, &mut out_bytes); // 'X' again
        push(258 - 256, 7, &mut acc, &mut nbits, &mut out_bytes); // len symbol 258 -> len 4
        push(0, 5, &mut acc, &mut nbits, &mut out_bytes); // dist symbol 0 -> dist 1
        push(0, 7, &mut acc, &mut nbits, &mut out_bytes); // end
        if nbits > 0 {
            out_bytes.push(acc as u8);
        }
        let (out, _) = inflate(&out_bytes, 6).unwrap();
        assert_eq!(out, b"XXXXXX");
    }

    #[test]
    fn real_deflate_roundtrip_via_python() {
        // Cross-check against a real deflate stream generated by an external
        // compressor at test time. Skips gracefully if python3 is unavailable.
        if !std::path::Path::new("/usr/bin/python3").exists() {
            return;
        }
        let data = b"the quick brown fox jumped over the lazy dog ".repeat(40);
        let py = std::process::Command::new("python3")
            .arg("-c")
            .arg("import sys,zlib;sys.stdout.buffer.write(zlib.compress(sys.stdin.buffer.read(),9))")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn();
        if let Ok(mut child) = py {
            use std::io::Write;
            if let Some(mut si) = child.stdin.take() {
                si.write_all(&data).ok();
            }
            let out = child.wait_with_output().ok();
            if let Some(out) = out {
                let de = zlib_decompress(&out.stdout, data.len()).unwrap();
                assert_eq!(de, data, "our inflate must read real zlib output");
            }
        }
    }
}
