//! The text of an open document as the server should see it, and positions in it.

use crate::position::{self, Encoding, Resolution};

#[derive(Debug, Clone)]
pub struct Document {
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Located {
    /// A byte offset on a character boundary.
    Exact(usize),
    /// A byte offset after clamping a character past the end of its line.
    Clamped(usize),
    /// The position is inside a character.
    SplitsChar,
    /// The line does not exist.
    LineOutOfRange,
}

pub fn encoding_of(name: &str) -> Option<Encoding> {
    match name {
        "utf-8" => Some(Encoding::Utf8),
        "utf-16" => Some(Encoding::Utf16),
        "utf-32" => Some(Encoding::Utf32),
        _ => None,
    }
}

/// The byte ranges of the lines of `text` without their end-of-line sequences (`\n`, `\r\n`,
/// `\r`). A text ending with an end-of-line has an empty last line.
pub fn lines(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                result.push((start, i));
                start = i + 1;
            }
            b'\r' => {
                result.push((start, i));
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    result.push((start, bytes.len()));
    result
}

pub fn locate(text: &str, encoding: Encoding, line: u64, character: u64) -> Located {
    let lines = lines(text);
    let Some(&(start, end)) = usize::try_from(line).ok().and_then(|l| lines.get(l)) else {
        return Located::LineOutOfRange;
    };
    let content = &text[start..end];
    let character = u32::try_from(character).unwrap_or(u32::MAX);
    let byte = |chars: usize| {
        start
            + content
                .char_indices()
                .nth(chars)
                .map_or(content.len(), |(i, _)| i)
    };
    match position::resolve(encoding, content, character) {
        Resolution::Exact(chars) => Located::Exact(byte(chars)),
        Resolution::Clamped(chars) => Located::Clamped(byte(chars)),
        Resolution::SplitsChar(_) => Located::SplitsChar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_on_every_end_of_line() {
        assert_eq!(lines("a\nb\r\nc\rd"), vec![(0, 1), (2, 3), (5, 6), (7, 8)]);
        assert_eq!(lines("a\n"), vec![(0, 1), (2, 2)]);
        assert_eq!(lines(""), vec![(0, 0)]);
    }

    #[test]
    fn locates_positions() {
        let text = "ab\r\na😀c";
        assert_eq!(locate(text, Encoding::Utf16, 1, 3), Located::Exact(9));
        assert_eq!(locate(text, Encoding::Utf16, 1, 2), Located::SplitsChar);
        assert_eq!(locate(text, Encoding::Utf16, 0, 9), Located::Clamped(2));
        assert_eq!(locate(text, Encoding::Utf16, 2, 0), Located::LineOutOfRange);
        assert_eq!(locate(text, Encoding::Utf8, 1, 5), Located::Exact(9));
    }
}
