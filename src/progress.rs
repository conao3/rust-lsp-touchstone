//! The life of a work done progress token: `begin`, any number of `report`s, then `end`.
//!
//! The transition function and its properties are written and proved in Lean (`lean!`): nothing
//! is accepted after `end`, the only way out of a created token is `begin`, a begun token
//! accepts only `report` and `end` (so a second `begin` is rejected), and every `begin`,
//! `report`*, `end` sequence is accepted.

lean_rust::lean! {
    /// The progress token state machine, proved in Lean.
    pub mod proved;
    r#"
    inductive TokenState where
      | created
      | begun
      | ended
      deriving DecidableEq

    inductive ProgressKind where
      | begin
      | report
      | finish
      deriving DecidableEq

    def step : TokenState → ProgressKind → Option TokenState
      | .created, .begin => some .begun
      | .begun, .report => some .begun
      | .begun, .finish => some .ended
      | _, _ => none

    def run : TokenState → List ProgressKind → Option TokenState
      | s, [] => some s
      | s, k :: ks => match step s k with
        | some s' => run s' ks
        | none => none

    theorem ended_accepts_nothing (k : ProgressKind) : step .ended k = none := by
      cases k <;> rfl

    theorem only_begin_leaves_created (k : ProgressKind) (s : TokenState)
        (h : step .created k = some s) : k = .begin ∧ s = .begun := by
      cases k <;> simp [step] at h ⊢ <;> exact h.symm

    theorem only_report_or_end_leave_begun (k : ProgressKind) (s : TokenState)
        (h : step .begun k = some s) : (k = .report ∧ s = .begun) ∨ (k = .finish ∧ s = .ended) := by
      cases k <;> simp [step] at h ⊢ <;> exact h.symm

    theorem begun_never_returns_to_created (k : ProgressKind) :
        step .begun k ≠ some .created := by
      cases k <;> simp [step]

    theorem nothing_after_end (ks : List ProgressKind) (k : ProgressKind) :
        run .ended (k :: ks) = none := by
      cases k <;> rfl

    theorem accepted_runs_begin_first (k : ProgressKind) (ks : List ProgressKind) (s : TokenState)
        (h : run .created (k :: ks) = some s) : k = .begin := by
      cases k <;> simp [run, step] at h ⊢

    theorem begin_reports_end (n : Nat) :
        run .created (.begin :: List.replicate n .report ++ [.finish]) = some .ended := by
      simp only [List.cons_append, run, step]
      induction n with
      | zero => rfl
      | succ n ih => simpa [List.replicate_succ, run, step] using ih
    "#
}

pub use proved::{ProgressKind, TokenState};

/// The next state of a token, or `None` when `kind` is not allowed in `state`.
pub fn step(state: TokenState, kind: ProgressKind) -> Option<TokenState> {
    proved::step(state, kind)
}

pub fn kind_of(value: &str) -> Option<ProgressKind> {
    match value {
        "begin" => Some(ProgressKind::Begin),
        "report" => Some(ProgressKind::Report),
        "end" => Some(ProgressKind::Finish),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_begin_report_end() {
        let s = step(TokenState::Created, ProgressKind::Begin).unwrap();
        let s = step(s, ProgressKind::Report).unwrap();
        assert_eq!(step(s, ProgressKind::Finish), Some(TokenState::Ended));
        assert_eq!(step(TokenState::Created, ProgressKind::Report), None);
        assert_eq!(step(TokenState::Ended, ProgressKind::Report), None);
        assert_eq!(step(TokenState::Begun, ProgressKind::Begin), None);
    }
}
