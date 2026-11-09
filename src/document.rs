//! The text of an open document as the server should see it, and positions in it.
//!
//! A change replaces a range of characters. The replacement is written and proved in Lean
//! (`lean!`): it keeps the text before the range, puts the new text in its place, keeps the text
//! after the range, has the expected length, replacing a range with its own text changes nothing,
//! and replacing the whole text gives the new text.

use crate::position::{self, Encoding, Resolution};

lean_rust::lean! {
    /// Replacing a range of characters, proved in Lean.
    pub mod proved;
    r#"
    def replaceList (cs : List Char) (start stop : Nat) (insert : List Char) : List Char :=
      cs.take start ++ insert ++ cs.drop stop

    theorem replace_keeps_prefix (cs ins : List Char) (start stop : Nat) (h : start ≤ cs.length) :
        (replaceList cs start stop ins).take start = cs.take start := by
      simp [replaceList, Nat.min_eq_left h]

    theorem replace_keeps_suffix (cs ins : List Char) (start stop : Nat) (h : start ≤ cs.length) :
        (replaceList cs start stop ins).drop (start + ins.length) = cs.drop stop := by
      have hlen : (cs.take start ++ ins).length = start + ins.length := by
        simp [Nat.min_eq_left h]
      simp only [replaceList]
      rw [← hlen, List.drop_left]

    theorem replace_inserts (cs ins : List Char) (start stop : Nat) (h : start ≤ cs.length) :
        ((replaceList cs start stop ins).drop start).take ins.length = ins := by
      have hl : (cs.take start).length = start := by simp [Nat.min_eq_left h]
      simp [replaceList, hl]

    theorem replace_length (cs ins : List Char) (start stop : Nat)
        (h1 : start ≤ stop) (h2 : stop ≤ cs.length) :
        (replaceList cs start stop ins).length = cs.length - (stop - start) + ins.length := by
      simp [replaceList]
      omega

    theorem replace_with_same_is_identity (cs : List Char) (start stop : Nat) (h1 : start ≤ stop) :
        replaceList cs start stop ((cs.drop start).take (stop - start)) = cs := by
      have h : cs.drop stop = (cs.drop start).drop (stop - start) := by
        rw [List.drop_drop]; congr 1; omega
      simp only [replaceList, List.append_assoc]
      rw [h, List.take_append_drop, List.take_append_drop]

    theorem replace_whole (cs ins : List Char) : replaceList cs 0 cs.length ins = ins := by
      simp [replaceList]

    def replaceRange (text : String) (start stop : Nat) (insert : String) : String :=
      String.ofList (replaceList text.toList start stop insert.toList)

    "#
}
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

/// Replaces the bytes `start..end` of `text`, which are on character boundaries, with `insert`.
pub fn replace(text: &str, start: usize, end: usize, insert: &str) -> String {
    let chars = |byte: usize| text[..byte].chars().count() as u64;
    proved::replace_range(text, chars(start).into(), chars(end).into(), insert)
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
    fn replaces_ranges_of_characters() {
        let text = "a😀b\nc";
        assert_eq!(replace(text, 1, 5, "XY"), "aXYb\nc");
        assert_eq!(replace(text, 0, text.len(), "new"), "new");
        assert_eq!(replace(text, 8, 8, "!"), "a😀b\nc!");
        assert_eq!(replace(text, 7, 7, "!"), "a😀b\n!c");
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
