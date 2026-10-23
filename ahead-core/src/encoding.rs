#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextEncoding {
    #[default]
    Utf8,
    Utf16LeBom,
    Utf16BeBom,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DecodedText {
    pub text: String,
    pub encoding: TextEncoding,
}

pub fn decode_text(bytes: &[u8]) -> Option<DecodedText> {
    if let Some(bytes) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Some(DecodedText {
            text: decode_utf16(bytes, true)?,
            encoding: TextEncoding::Utf16LeBom,
        });
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Some(DecodedText {
            text: decode_utf16(bytes, false)?,
            encoding: TextEncoding::Utf16BeBom,
        });
    }
    String::from_utf8(bytes.to_vec())
        .ok()
        .map(|text| DecodedText {
            text,
            encoding: TextEncoding::Utf8,
        })
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Option<String> {
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let code_units = pairs
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes(*pair)
            } else {
                u16::from_be_bytes(*pair)
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&code_units).ok()
}

pub fn encode_text(text: &str, encoding: TextEncoding) -> Vec<u8> {
    match encoding {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf16LeBom => encode_utf16(text, true),
        TextEncoding::Utf16BeBom => encode_utf16(text, false),
    }
}

fn encode_utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if little_endian {
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    } else {
        bytes.extend_from_slice(&[0xFE, 0xFF]);
        bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    }
    bytes
}

/// Convert a utf8 offset into a utf16 offset, if possible  
/// `text` is what the offsets are into
pub fn offset_utf8_to_utf16(
    char_indices: impl Iterator<Item = (usize, char)>,
    offset: usize,
) -> usize {
    if offset == 0 {
        return 0;
    }

    let mut utf16_offset = 0;
    let mut last_ich = None;
    for (utf8_offset, ch) in char_indices {
        last_ich = Some((utf8_offset, ch));

        match utf8_offset.cmp(&offset) {
            std::cmp::Ordering::Less => {}
            // We found the right offset
            std::cmp::Ordering::Equal => {
                return utf16_offset;
            }
            // Implies that the offset was inside of a character
            std::cmp::Ordering::Greater => return utf16_offset,
        }

        utf16_offset += ch.len_utf16();
    }

    // TODO: We could use TrustedLen when that is stabilized and it is impl'd on
    // the iterators we use

    // We did not find the offset. This means that it is either at the end
    // or past the end.
    let text_len = last_ich.map(|(i, c)| i + c.len_utf8());
    if text_len == Some(offset) {
        // Since the utf16 offset was being incremented each time, by now it is equivalent to the length
        // but in utf16 characters
        return utf16_offset;
    }

    utf16_offset
}

pub fn offset_utf8_to_utf16_str(text: &str, offset: usize) -> usize {
    offset_utf8_to_utf16(text.char_indices(), offset)
}

/// Convert a utf16 offset into a utf8 offset, if possible  
/// `char_indices` is an iterator over utf8 offsets and the characters
/// It is cloneable so that it can be iterated multiple times. Though it should be cheaply cloneable.
pub fn offset_utf16_to_utf8(
    char_indices: impl Iterator<Item = (usize, char)>,
    offset: usize,
) -> usize {
    if offset == 0 {
        return 0;
    }

    // We accumulate the utf16 char lens until we find the utf8 offset that matches it
    // or, we find out that it went into the middle of sometext
    // We also keep track of the last offset and char in order to calculate the length of the text
    // if we the index was at the end of the string
    let mut utf16_offset = 0;
    let mut last_ich = None;
    for (utf8_offset, ch) in char_indices {
        last_ich = Some((utf8_offset, ch));

        let ch_utf16_len = ch.len_utf16();

        match utf16_offset.cmp(&offset) {
            std::cmp::Ordering::Less => {}
            // We found the right offset
            std::cmp::Ordering::Equal => {
                return utf8_offset;
            }
            // This implies that the offset was in the middle of a character as we skipped over it
            std::cmp::Ordering::Greater => return utf8_offset,
        }

        utf16_offset += ch_utf16_len;
    }

    // We did not find the offset, this means that it was either at the end
    // or past the end
    // Since we've iterated over all the char indices, the utf16_offset is now the
    // utf16 length
    if let Some((last_utf8_offset, last_ch)) = last_ich {
        last_utf8_offset + last_ch.len_utf8()
    } else {
        0
    }
}

pub fn offset_utf16_to_utf8_str(text: &str, offset: usize) -> usize {
    offset_utf16_to_utf8(text.char_indices(), offset)
}

#[cfg(test)]
mod tests {
    // TODO: more tests with unicode characters

    use crate::encoding::{
        DecodedText, TextEncoding, decode_text, encode_text,
        offset_utf8_to_utf16_str, offset_utf16_to_utf8_str,
    };

    #[test]
    fn utf16_bom_text_round_trips_without_loss() {
        let text = "café 🦀\r\n";
        for encoding in [TextEncoding::Utf16LeBom, TextEncoding::Utf16BeBom] {
            let bytes = encode_text(text, encoding);
            let decoded = decode_text(&bytes).expect("decode UTF-16 text");
            assert_eq!(
                decoded,
                DecodedText {
                    text: text.into(),
                    encoding
                }
            );
        }
    }

    #[test]
    fn malformed_utf16_bom_is_rejected_and_unmarked_bytes_remain_utf8() {
        assert!(decode_text(&[0xFF, 0xFE, 0x00]).is_none());
        assert!(decode_text(&[0xFF, 0xFE, 0x00, 0xD8]).is_none());
        assert_eq!(
            decode_text(&[b'h', 0, b'i', 0])
                .expect("valid UTF-8 bytes")
                .encoding,
            TextEncoding::Utf8
        );
    }

    #[test]
    fn utf8_to_utf16() {
        let text = "hello world";

        assert_eq!(offset_utf8_to_utf16_str(text, 0), 0);
        assert_eq!(offset_utf8_to_utf16_str("", 0), 0);

        assert_eq!(offset_utf8_to_utf16_str("", 1), 0);

        assert_eq!(offset_utf8_to_utf16_str("h", 0), 0);
        assert_eq!(offset_utf8_to_utf16_str("h", 1), 1);

        assert_eq!(offset_utf8_to_utf16_str(text, text.len()), text.len());

        assert_eq!(
            offset_utf8_to_utf16_str(text, text.len() - 1),
            text.len() - 1
        );

        assert_eq!(offset_utf8_to_utf16_str(text, text.len() + 1), text.len());

        assert_eq!(offset_utf8_to_utf16_str("×", 0), 0);
        assert_eq!(offset_utf8_to_utf16_str("×", 1), 1);
        assert_eq!(offset_utf8_to_utf16_str("×", 2), 1);
        assert_eq!(offset_utf8_to_utf16_str("a×", 0), 0);
        assert_eq!(offset_utf8_to_utf16_str("a×", 1), 1);
        assert_eq!(offset_utf8_to_utf16_str("a×", 2), 2);
        assert_eq!(offset_utf8_to_utf16_str("a×", 3), 2);
    }

    #[test]
    fn utf16_to_utf8() {
        let text = "hello world";

        assert_eq!(offset_utf16_to_utf8_str(text, 0), 0);
        assert_eq!(offset_utf16_to_utf8_str("", 0), 0);

        assert_eq!(offset_utf16_to_utf8_str("", 1), 0);

        assert_eq!(offset_utf16_to_utf8_str("h", 0), 0);
        assert_eq!(offset_utf16_to_utf8_str("h", 1), 1);

        assert_eq!(offset_utf16_to_utf8_str(text, text.len()), text.len());

        assert_eq!(
            offset_utf16_to_utf8_str(text, text.len() - 1),
            text.len() - 1
        );

        assert_eq!(offset_utf16_to_utf8_str(text, text.len() + 1), text.len());

        assert_eq!(offset_utf16_to_utf8_str("×", 0), 0);
        assert_eq!(offset_utf16_to_utf8_str("×", 1), 2);
        assert_eq!(offset_utf16_to_utf8_str("a×", 0), 0);
        assert_eq!(offset_utf16_to_utf8_str("a×", 1), 1);
        assert_eq!(offset_utf16_to_utf8_str("a×", 2), 3);
        assert_eq!(offset_utf16_to_utf8_str("×a", 1), 2);
        assert_eq!(offset_utf16_to_utf8_str("×a", 2), 3);
    }
}
