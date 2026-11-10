namespace proved

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

end proved
