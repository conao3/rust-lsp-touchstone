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

theorem replaceRange_keeps_prefix (text insert : String) (start stop : Nat)
    (h : start ≤ text.toList.length) :
    (replaceRange text start stop insert).toList.take start = text.toList.take start := by
  simpa only [replaceRange, String.toList_ofList] using
    replace_keeps_prefix text.toList insert.toList start stop h

theorem replaceRange_keeps_suffix (text insert : String) (start stop : Nat)
    (h : start ≤ text.toList.length) :
    (replaceRange text start stop insert).toList.drop (start + insert.toList.length) =
      text.toList.drop stop := by
  simpa only [replaceRange, String.toList_ofList] using
    replace_keeps_suffix text.toList insert.toList start stop h

theorem replaceRange_inserts (text insert : String) (start stop : Nat)
    (h : start ≤ text.toList.length) :
    ((replaceRange text start stop insert).toList.drop start).take insert.toList.length =
      insert.toList := by
  simpa only [replaceRange, String.toList_ofList] using
    replace_inserts text.toList insert.toList start stop h

theorem replaceRange_length (text insert : String) (start stop : Nat)
    (h1 : start ≤ stop) (h2 : stop ≤ text.toList.length) :
    (replaceRange text start stop insert).toList.length =
      text.toList.length - (stop - start) + insert.toList.length := by
  simpa only [replaceRange, String.toList_ofList] using
    replace_length text.toList insert.toList start stop h1 h2

theorem replaceRange_with_same_is_identity (text : String) (start stop : Nat)
    (h : start ≤ stop) :
    replaceRange text start stop
      (String.ofList ((text.toList.drop start).take (stop - start))) = text := by
  simp only [replaceRange, String.toList_ofList,
    replace_with_same_is_identity text.toList start stop h, String.ofList_toList]

theorem replaceRange_whole (text insert : String) :
    replaceRange text 0 text.toList.length insert = insert := by
  simp only [replaceRange, replace_whole, String.ofList_toList]


end proved
