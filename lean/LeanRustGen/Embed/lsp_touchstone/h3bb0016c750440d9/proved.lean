namespace proved

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


end proved
