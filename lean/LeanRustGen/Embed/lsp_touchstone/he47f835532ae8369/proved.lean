namespace proved

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


end proved
