//! Positions in a line of text: a `character` counted in code units of the negotiated
//! position encoding (`positionEncoding`), resolved to a count of Unicode scalar values.
//!
//! The resolution and its properties are written and proved in Lean (`lean!`): converting a
//! character count to code units and back is the identity, a position past the end of the line
//! is clamped to the line length, a position inside a multi-unit character is reported with the
//! character it splits and every such position is reported, and the width of a character is its
//! UTF-8 byte count, two UTF-16 code units exactly when it takes four UTF-8 bytes, and one UTF-32
//! code unit.

lean_rust::lean! {
    /// The resolution of a `character` in a line, proved in Lean.
    pub mod proved;
    r#"
    inductive Encoding where
      | utf8
      | utf16
      | utf32

    def unitWidth : Encoding → Char → Nat
      | .utf8, c => c.utf8Size
      | .utf16, c => if c.val < 0x10000 then 1 else 2
      | .utf32, _ => 1

    inductive Resolution where
      | exact (chars : Nat)
      | clamped (chars : Nat)
      | splitsChar (chars : Nat)

    def Resolution.shift : Resolution → Resolution
      | .exact n => .exact (n + 1)
      | .clamped n => .clamped (n + 1)
      | .splitsChar n => .splitsChar (n + 1)

    def resolveList (e : Encoding) : List Char → Nat → Resolution
      | [], 0 => .exact 0
      | [], _ + 1 => .clamped 0
      | c :: cs, n =>
        if n = 0 then .exact 0
        else if n < unitWidth e c then .splitsChar 0
        else (resolveList e cs (n - unitWidth e c)).shift

    def offsetList (e : Encoding) : List Char → Nat → Nat
      | [], _ => 0
      | _ :: _, 0 => 0
      | c :: cs, i + 1 => unitWidth e c + offsetList e cs i

    theorem unitWidth_pos (e : Encoding) (c : Char) : 0 < unitWidth e c := by
      cases e with
      | utf8 => exact Char.utf8Size_pos c
      | utf16 => simp only [unitWidth]; split <;> omega
      | utf32 => simp [unitWidth]

    theorem resolve_offset (e : Encoding) (cs : List Char) (i : Nat) (h : i ≤ cs.length) :
        resolveList e cs (offsetList e cs i) = .exact i := by
      induction cs generalizing i with
      | nil =>
        simp at h; subst h; simp [offsetList, resolveList]
      | cons c cs ih =>
        cases i with
        | zero => simp [offsetList, resolveList]
        | succ i =>
          have hw := unitWidth_pos e c
          simp only [List.length_cons] at h
          rw [show offsetList e (c :: cs) (i + 1) = unitWidth e c + offsetList e cs i from rfl]
          simp only [resolveList]
          split
          · omega
          · split
            · omega
            · rw [Nat.add_sub_cancel_left, ih i (by omega)]; rfl

    theorem resolve_beyond (e : Encoding) (cs : List Char) (n : Nat)
        (h : offsetList e cs cs.length < n) : resolveList e cs n = .clamped cs.length := by
      induction cs generalizing n with
      | nil =>
        cases n with
        | zero => simp at h
        | succ n => simp [resolveList]
      | cons c cs ih =>
        have hw := unitWidth_pos e c
        simp only [List.length_cons, offsetList] at h
        simp only [resolveList, List.length_cons]
        split
        · omega
        · split
          · omega
          · rw [ih (n - unitWidth e c) (by omega)]; rfl

    theorem resolve_splits (e : Encoding) (cs : List Char) (n i : Nat)
        (h : resolveList e cs n = .splitsChar i) :
        i < cs.length ∧ offsetList e cs i < n ∧ n < offsetList e cs (i + 1) := by
      induction cs generalizing n i with
      | nil =>
        cases n <;> simp [resolveList] at h
      | cons c cs ih =>
        have hw := unitWidth_pos e c
        simp only [resolveList] at h
        split at h
        · simp at h
        · split at h
          · simp at h; subst h; simp [offsetList]; omega
          · cases hr : resolveList e cs (n - unitWidth e c) with
            | exact k => rw [hr] at h; simp [Resolution.shift] at h
            | clamped k => rw [hr] at h; simp [Resolution.shift] at h
            | splitsChar k =>
              rw [hr] at h; simp [Resolution.shift] at h; subst h
              have := ih (n - unitWidth e c) k hr
              simp only [List.length_cons, offsetList]
              omega

    theorem unitWidth_spec (c : Char) :
        unitWidth .utf8 c = c.utf8Size ∧
          unitWidth .utf16 c = (if c.utf8Size = 4 then 2 else 1) ∧
          unitWidth .utf32 c = 1 := by
      refine ⟨rfl, ?_, rfl⟩
      simp only [unitWidth, Char.utf8Size]
      have := UInt32.lt_iff_toNat_lt (a := c.val) (b := 0x10000)
      simp only [UInt32.le_iff_toNat_le] at *
      split <;> split <;> (try split) <;> (try split) <;> simp_all <;> omega

    theorem resolve_inside (e : Encoding) (cs : List Char) (i n : Nat) (hi : i < cs.length)
        (h1 : offsetList e cs i < n) (h2 : n < offsetList e cs (i + 1)) :
        resolveList e cs n = .splitsChar i := by
      induction cs generalizing n i with
      | nil => simp at hi
      | cons c cs ih =>
        have hw := unitWidth_pos e c
        cases i with
        | zero =>
          have h0 : offsetList e cs 0 = 0 := by cases cs <;> rfl
          simp only [offsetList, h0] at h1 h2
          simp only [resolveList]
          split
          · omega
          · split
            · rfl
            · omega
        | succ i =>
          simp only [List.length_cons] at hi
          simp only [offsetList] at h1 h2
          simp only [resolveList]
          split
          · omega
          · split
            · omega
            · rw [ih i (n - unitWidth e c) (by omega) (by omega) (by omega)]; rfl

    def resolve (e : Encoding) (line : String) (character : Nat) : Resolution :=
      resolveList e line.toList character

    def offset (e : Encoding) (line : String) (chars : Nat) : Nat :=
      offsetList e line.toList chars

    "#
}

pub use proved::Encoding;

/// Where a `character` falls in a line, as a count of Unicode scalar values before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The position is on a character boundary.
    Exact(usize),
    /// The position is past the end of the line; the specification clamps it to the line length.
    Clamped(usize),
    /// The position is inside the character after this many characters.
    SplitsChar(usize),
}

pub fn resolve(encoding: Encoding, line: &str, character: u32) -> Resolution {
    let to_usize = |n: lean_rust::Nat| {
        usize::try_from(u64::try_from(n).expect("a count fits in u64"))
            .expect("a count fits in usize")
    };
    match proved::resolve(encoding, line, u64::from(character).into()) {
        proved::Resolution::Exact { chars: n } => Resolution::Exact(to_usize(n)),
        proved::Resolution::Clamped { chars: n } => Resolution::Clamped(to_usize(n)),
        proved::Resolution::SplitsChar { chars: n } => Resolution::SplitsChar(to_usize(n)),
    }
}

/// The number of code units of `encoding` in the first `chars` characters of `line`.
pub fn offset(encoding: Encoding, line: &str, chars: usize) -> u64 {
    u64::try_from(proved::offset(encoding, line, (chars as u64).into()))
        .expect("an offset fits in u64")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_counts_a_surrogate_pair_as_two_units() {
        assert_eq!(resolve(Encoding::Utf16, "a😀b", 1), Resolution::Exact(1));
        assert_eq!(
            resolve(Encoding::Utf16, "a😀b", 2),
            Resolution::SplitsChar(1)
        );
        assert_eq!(resolve(Encoding::Utf16, "a😀b", 3), Resolution::Exact(2));
        assert_eq!(offset(Encoding::Utf16, "a😀b", 3), 4);
    }

    #[test]
    fn utf8_counts_bytes() {
        assert_eq!(resolve(Encoding::Utf8, "日本", 3), Resolution::Exact(1));
        assert_eq!(
            resolve(Encoding::Utf8, "日本", 4),
            Resolution::SplitsChar(1)
        );
        assert_eq!(offset(Encoding::Utf8, "日本", 2), 6);
    }

    #[test]
    fn utf32_counts_characters() {
        assert_eq!(resolve(Encoding::Utf32, "a😀b", 2), Resolution::Exact(2));
    }

    #[test]
    fn past_the_end_is_clamped() {
        assert_eq!(resolve(Encoding::Utf16, "ab", 10), Resolution::Clamped(2));
        assert_eq!(resolve(Encoding::Utf16, "", 1), Resolution::Clamped(0));
        assert_eq!(resolve(Encoding::Utf16, "", 0), Resolution::Exact(0));
    }
}
