//! MUTF-8 (Modified UTF-8, JVMS/dex string form): NUL is C0 80, supplementary
//! code points appear as two 3-byte surrogate sequences (CESU-8 style).
//! Decodes defensively: malformed sequences become U+FFFD, never panic.

pub fn decode_mutf8(bytes: &[u8], utf16_len_hint: usize) -> String {
    let mut out = String::with_capacity(utf16_len_hint.max(8));
    let mut i = 0usize;
    let n = bytes.len();
    while i < n {
        // stop after the logical string when we can detect a terminator:
        // dex strings are NUL-terminated, and the NUL is encoded as C0 80.
        if bytes[i] == 0xC0 && i + 1 < n && bytes[i + 1] == 0x80 {
            out.push('\u{0000}');
            i += 2;
            // keep going: the string_data_item ends with a 0x00 byte terminator;
            // in MUTF-8, that terminator is a single 0x00 byte (not C0 80).
            if i < n && bytes[i] == 0x00 {
                break;
            }
            continue;
        }
        let b0 = bytes[i];
        if b0 == 0 {
            break; // end-of-data terminator
        }
        if b0 < 0x80 {
            out.push(b0 as char);
            i += 1;
            continue;
        }
        if b0 & 0xE0 == 0xC0 && i + 1 < n && (bytes[i + 1] & 0xC0) == 0x80 {
            // two-byte: covers U+0080..=U+07FF and the C0 80 NUL form
            let cp = (((b0 & 0x1F) as u32) << 6) | (bytes[i + 1] & 0x3F) as u32;
            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
            i += 2;
            continue;
        }
        if b0 & 0xF0 == 0xE0 {
            if i + 2 >= n {
                out.push('\u{FFFD}');
                break; // truncated 3-byte sequence: one replacement, end of input
            }
            if (bytes[i + 1] & 0xC0) != 0x80 {
                out.push('\u{FFFD}');
                i += 1;
                continue;
            }
            if (bytes[i + 2] & 0xC0) != 0x80 {
                out.push('\u{FFFD}');
                i += 2;
                continue;
            }
            let cp = (((b0 & 0x0F) as u32) << 12)
                | (((bytes[i + 1] & 0x3F) as u32) << 6)
                | (bytes[i + 2] & 0x3F) as u32;
            i += 3;
            if (0xD800..0xDC00).contains(&cp) {
                // high surrogate: try to pair with the next 3-byte sequence
                if i + 2 < n
                    && bytes[i] & 0xF0 == 0xE0
                    && bytes[i + 1] & 0xC0 == 0x80
                    && bytes[i + 2] & 0xC0 == 0x80
                {
                    let lo = (((bytes[i] & 0x0F) as u32) << 12)
                        | (((bytes[i + 1] & 0x3F) as u32) << 6)
                        | (bytes[i + 2] & 0x3F) as u32;
                    if (0xDC00..0xE000).contains(&lo) {
                        let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                        i += 3;
                        continue;
                    }
                }
                out.push('\u{FFFD}');
            } else if (0xDC00..0xE000).contains(&cp) {
                out.push('\u{FFFD}');
            } else {
                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
            }
            continue;
        }
        // malformed: replace one byte
        out.push('\u{FFFD}');
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii() {
        assert_eq!(decode_mutf8(b"hello", 5), "hello");
    }

    #[test]
    fn nul_form() {
        assert_eq!(decode_mutf8(&[b'a', 0xC0, 0x80, b'b', 0x00], 4), "a\u{0}b");
    }

    #[test]
    fn two_byte() {
        // U+00E9 é = C3 A9
        assert_eq!(decode_mutf8(&[0xC3, 0xA9], 1), "\u{00E9}");
    }

    #[test]
    fn three_byte() {
        // U+4E2D 中 = E4 B8 AD
        assert_eq!(decode_mutf8(&[0xE4, 0xB8, 0xAD], 1), "\u{4E2D}");
    }

    #[test]
    fn surrogate_pair() {
        // U+1F600 as CESU-8: ED A0 BD ED B8 80
        let bytes = [0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80];
        assert_eq!(decode_mutf8(&bytes, 2), "\u{1F600}");
    }

    #[test]
    fn malformed_replaces() {
        assert_eq!(decode_mutf8(&[0xE4, 0xB8], 1), "\u{FFFD}");
        assert_eq!(decode_mutf8(&[0x80], 1), "\u{FFFD}");
    }
}
