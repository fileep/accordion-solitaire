/-
  Accordion solitaire: the rules, the deal generator, and the proof that a deal is lost from an automaton certificate.
  Core Lean 4 only, and no `sorry`: every theorem here is proved, for any certificate.

  Part 1  rules on piles, and the reduction to top cards
  Part 2  the deal generator
  Part 3  lost by invariant
  Part 4  the automaton certificate, and the theorem that a passing certificate proves the loss
  Part 5  a table the kernel can read quickly; nothing about it needs trusting
  Part 6  replaying a winning line, to show the rules are not too strict
-/

set_option autoImplicit false

namespace Accordion

/-! ## Part 1. Rules -/

/-- A card is its index 0..51: suit * 13 + rank, as `Card::index` in crates/core/src/card.rs. -/
abbrev Card := Nat

/-- Two cards match if they share a suit or a rank. -/
def sameSuitOrRank (a b : Card) : Bool :=
  Nat.beq (a / 13) (b / 13) || Nat.beq (a % 13) (b % 13)

/-- A pile: its top card, then the rest top-down. Never empty by construction. -/
structure Pile where
  top   : Card
  below : List Card

/-- One legal move on piles: a whole pile goes onto the pile 1 or 3 places to its left, if the two top cards match.
    Moving pile `p` onto `t` gives a pile whose top is p's top; what lay in `t` stays under it. -/
inductive PileStep : List Pile → List Pile → Prop
  | slide (pre : List Pile) (t p : Pile) (post : List Pile) (h : sameSuitOrRank t.top p.top = true) :
      PileStep (pre ++ t :: p :: post) (pre ++ ⟨p.top, p.below ++ t.top :: t.below⟩ :: post)
  | leap (pre : List Pile) (t x y p : Pile) (post : List Pile) (h : sameSuitOrRank t.top p.top = true) :
      PileStep (pre ++ t :: x :: y :: p :: post)
               (pre ++ ⟨p.top, p.below ++ t.top :: t.below⟩ :: x :: y :: post)

/-- The same moves on the row of top cards alone. -/
inductive Step : List Card → List Card → Prop
  | slide (pre : List Card) (a b : Card) (post : List Card) (h : sameSuitOrRank a b = true) :
      Step (pre ++ a :: b :: post) (pre ++ b :: post)
  | leap (pre : List Card) (a x y b : Card) (post : List Card) (h : sameSuitOrRank a b = true) :
      Step (pre ++ a :: x :: y :: b :: post) (pre ++ b :: x :: y :: post)

/-- What is under a pile never affects the moves, so a game of piles projects onto a game of top cards. -/
theorem tops_step {s t : List Pile} (h : PileStep s t) :
    Step (s.map Pile.top) (t.map Pile.top) := by
  cases h with
  | slide pre t p post h => simpa using Step.slide (pre.map Pile.top) t.top p.top (post.map Pile.top) h
  | leap pre t x y p post h =>
    simpa using Step.leap (pre.map Pile.top) t.top x.top y.top p.top (post.map Pile.top) h

/-- Zero or more steps. -/
inductive Reach : List Card → List Card → Prop
  | refl (w) : Reach w w
  | tail {u v w} : Reach u v → Step v w → Reach u w

/-- A deal is lost if no line of play leaves a single pile. -/
def Lost (deal : List Card) : Prop := ∀ c, ¬ Reach deal [c]

/-- Zero or more steps on piles. -/
inductive PileReach : List Pile → List Pile → Prop
  | refl (s) : PileReach s s
  | tail {s t u} : PileReach s t → PileStep t u → PileReach s u

/-- The game as it starts: one card to a pile. -/
def start (deal : List Card) : List Pile := deal.map fun c => ⟨c, []⟩

/-- The statement to read: no line of play from the deal leaves a single pile. -/
def DealLost (deal : List Card) : Prop := ∀ p, ¬ PileReach (start deal) [p]

theorem tops_reach {s t : List Pile} (h : PileReach s t) :
    Reach (s.map Pile.top) (t.map Pile.top) := by
  induction h with
  | refl => exact Reach.refl _
  | tail _ hs ih => exact Reach.tail ih (tops_step hs)

theorem dealLost_of_lost {deal : List Card} (h : Lost deal) : DealLost deal := by
  intro p hp
  have hr := tops_reach hp
  have e : (start deal).map Pile.top = deal := by simp [start, Function.comp_def]
  rw [e] at hr
  exact h p.top (by simpa using hr)

/-! ## Part 2. The generator (crates/core/src/rng.rs, deal.rs)

Everything is plain `Nat` modulo 2^64, because the kernel has fast big-number `Nat.add`, `Nat.mul`, `Nat.mod`,
`Nat.land`, `Nat.lor`, `Nat.xor`, `Nat.shiftLeft`, `Nat.shiftRight`. A theorem about a seed computes `deal seed` itself, so
it can only hold if this text gives the deal the certificate was built for. -/

def wrap (x : Nat) : Nat := x % 18446744073709551616
def rotl (x k : Nat) : Nat := wrap (Nat.lor (Nat.shiftLeft x k) (Nat.shiftRight x (64 - k)))

/-- SplitMix64: returns the new state and one output. -/
def splitmix (s : Nat) : Nat × Nat :=
  let s' := wrap (s + 0x9E3779B97F4A7C15)
  let z := wrap (Nat.xor s' (Nat.shiftRight s' 30) * 0xBF58476D1CE4E5B9)
  let z := wrap (Nat.xor z (Nat.shiftRight z 27) * 0x94D049BB133111EB)
  (s', Nat.xor z (Nat.shiftRight z 31))

structure Xo where
  (s0 s1 s2 s3 : Nat)

def Xo.seed (seed : Nat) : Xo :=
  let (a, z0) := splitmix seed
  let (b, z1) := splitmix a
  let (c, z2) := splitmix b
  let (_, z3) := splitmix c
  ⟨z0, z1, z2, z3⟩

/-- xoshiro256** next: the output and the new state. -/
def Xo.next (x : Xo) : Nat × Xo :=
  let r := wrap (rotl (wrap (x.s1 * 5)) 7 * 9)
  let t := wrap (Nat.shiftLeft x.s1 17)
  let s2 := Nat.xor x.s2 x.s0
  let s3 := Nat.xor x.s3 x.s1
  let s1 := Nat.xor x.s1 s2
  let s0 := Nat.xor x.s0 s3
  let s2 := Nat.xor s2 t
  let s3 := rotl s3 45
  (r, ⟨s0, s1, s2, s3⟩)

/-- Lemire's rejection, with fuel: a redraw has chance about 2^-58, so fuel is never used up on a real seed. -/
def Xo.below (x : Xo) (n : Nat) : Nat × Xo :=
  let rec loop (fuel : Nat) (x : Xo) (threshold : Nat) (m : Nat) : Nat × Xo :=
    match fuel with
    | 0 => (Nat.shiftRight m 64, x)
    | f + 1 =>
      if wrap m < threshold then
        let (r, x') := x.next
        loop f x' threshold (r * n)
      else (Nat.shiftRight m 64, x)
  let (r, x') := x.next
  let m := r * n
  if wrap m < n then loop 8 x' ((18446744073709551616 - n) % n) m
  else (Nat.shiftRight m 64, x')

/-- Fisher-Yates from the last position down. -/
def swap (l : List Nat) (i j : Nat) : List Nat :=
  let a := l.getD i 0
  let b := l.getD j 0
  (l.set i b).set j a

/-- Runs i = 51, 50, ..., 1; `n` steps left means i = n. -/
def shuffle : Nat → Xo → List Nat → List Nat
  | 0, _, l => l
  | i + 1, x, l =>
    let (j, x') := x.below (i + 2)
    shuffle i x' (swap l (i + 1) j)

def deal (seed : Nat) : List Card := shuffle 51 (Xo.seed seed) (List.range 52)

/-! ## Part 3. Lost by invariant -/

theorem reach_inv {S : List Card → Prop} (hc : ∀ w w', S w → Step w w' → S w')
    {u v : List Card} (hu : S u) (h : Reach u v) : S v := by
  induction h with
  | refl => exact hu
  | tail _ hs ih => exact hc _ _ ih hs

theorem lost_of_invariant {d : List Card} (S : List Card → Prop)
    (h0 : S d) (hc : ∀ w w', S w → Step w w' → S w') (hw : ∀ c, ¬ S [c]) : Lost d :=
  fun c hr => hw c (reach_inv hc h0 hr)

/-! ## Part 4. The automaton certificate

The set of positions is the language of a deterministic automaton over cards, given as an untrusted table from outside
Lean. `sup r` lists nodes claimed to accept everything `r` accepts. The automaton is deterministic, so one pass over the
nodes is enough to show that moves stay in the language, and nothing is enumerated. Letters are card numbers as in
`sameSuitOrRank`, suit * 13 + rank; dawg.py numbers them rank * 4 + suit, so lean_certificates.py converts them. -/

structure Aut where
  size  : Nat
  edges : Nat → List (Nat × Nat)    -- (letter, target)
  acc   : Nat → Bool                -- a word may end here
  sup   : Nat → List Nat

def lookup (a : Nat) : List (Nat × Nat) → Option Nat
  | [] => none
  | e :: rest => bif Nat.beq a e.1 then some e.2 else lookup a rest

def memNat (s : Nat) : List Nat → Bool
  | [] => false
  | x :: xs => Nat.beq s x || memNat s xs

namespace Aut

variable (A : Aut)

def next (q a : Nat) : Option Nat := lookup a (A.edges q)

def run : Nat → List Nat → Option Nat
  | q, [] => some q
  | q, a :: w => match A.next q a with
    | some q' => run q' w
    | none => none

def accepts : Nat → List Nat → Bool
  | q, [] => A.acc q
  | q, a :: w => match A.next q a with
    | some q' => accepts q' w
    | none => false

/-- Is everything `r` accepts claimed to be accepted by `s`? -/
def sub (r s : Nat) : Bool := Nat.beq r s || memNat s (A.sup r)

def inclAt (r s : Nat) : Bool :=
  (!(A.acc r) || A.acc s) &&
  (A.edges r).all fun e =>
    match A.next s e.1 with
    | some s' => A.sub e.2 s'
    | none => false

def covered (p : Nat) (path : List Nat) (r : Nat) : Bool :=
  match A.run p path with
  | some s => A.sub r s
  | none => false

/-- The local rule at node `p`: whatever a slide or a leap onto letter `a` produces is back in the language of `p`. -/
def nodeOK (p : Nat) : Bool :=
  (A.edges p).all fun e =>
    (A.edges e.2).all fun f =>
      (!(sameSuitOrRank e.1 f.1) || A.covered p [f.1] f.2) &&
      (A.edges f.2).all fun g =>
        (A.edges g.2).all fun h =>
          !(sameSuitOrRank e.1 h.1) || A.covered p [h.1, f.1, g.1] h.2

/-- Reading a word can't leave the nodes that `check` looks at. Without this, a table with `size = 0` would pass. -/
def inBounds (root : Nat) : Bool :=
  Nat.ble (root + 1) A.size &&
  (List.range A.size).all fun q => (A.edges q).all fun e => Nat.ble (e.2 + 1) A.size

def inclOK : Bool := (List.range A.size).all fun r => (A.sup r).all fun s => A.inclAt r s

def nodesOK : Bool := (List.range A.size).all A.nodeOK

/-- All of it: no word of one letter is accepted, so no position in the language is a win. -/
def check (root : Nat) : Bool :=
  A.inBounds root && A.inclOK && A.nodesOK && (A.edges root).all fun e => !(A.acc e.2)

end Aut

theorem and_left {a b : Bool} (h : (a && b) = true) : a = true := by cases a <;> simp_all
theorem and_right {a b : Bool} (h : (a && b) = true) : b = true := by cases a <;> simp_all
theorem of_not_or {a b : Bool} (h : (!a || b) = true) (ha : a = true) : b = true := by cases a <;> simp_all
theorem all_mem {α : Type} {l : List α} {p : α → Bool} {x : α} (h : l.all p = true) (hx : x ∈ l) : p x = true :=
  List.all_eq_true.mp h x hx
theorem all_range {n : Nat} {p : Nat → Bool} {x : Nat} (h : (List.range n).all p = true) (hx : x < n) : p x = true :=
  all_mem h (List.mem_range.mpr hx)

theorem lookup_mem {a q : Nat} : ∀ {es : List (Nat × Nat)}, lookup a es = some q → (a, q) ∈ es
  | [], h => by cases h
  | (b, r) :: rest, h => by
    have e : lookup a ((b, r) :: rest) = cond (Nat.beq a b) (some r) (lookup a rest) := rfl
    rw [e] at h
    cases hb : Nat.beq a b with
    | true =>
      rw [hb] at h
      have hab : a = b := Nat.eq_of_beq_eq_true hb
      have hrq : r = q := Option.some.inj h
      subst hab; subst hrq
      exact List.mem_cons_self
    | false =>
      rw [hb] at h
      exact List.mem_cons_of_mem _ (lookup_mem h)

theorem mem_of_memNat {s : Nat} : ∀ {xs : List Nat}, memNat s xs = true → s ∈ xs
  | [], h => by cases h
  | x :: xs, h => by
    have e : memNat s (x :: xs) = (Nat.beq s x || memNat s xs) := rfl
    rw [e] at h
    cases hb : Nat.beq s x with
    | true =>
      have hsx : s = x := Nat.eq_of_beq_eq_true hb
      subst hsx
      exact List.mem_cons_self
    | false =>
      rw [hb] at h
      exact List.mem_cons_of_mem _ (mem_of_memNat h)

namespace Aut

variable {A : Aut}

theorem next_mem {q a q' : Nat} (h : A.next q a = some q') : (a, q') ∈ A.edges q := lookup_mem h

theorem accepts_cons {q a : Nat} {w : List Nat} :
    A.accepts q (a :: w) = true ↔ ∃ q', A.next q a = some q' ∧ A.accepts q' w = true := by
  show (match A.next q a with | some q' => A.accepts q' w | none => false) = true ↔ _
  cases A.next q a with
  | none => simp
  | some q' => simp

theorem accepts_of_run {s : Nat} {w : List Nat} (ha : A.accepts s w = true) :
    ∀ (path : List Nat) (q : Nat), A.run q path = some s → A.accepts q (path ++ w) = true := by
  intro path
  induction path with
  | nil =>
    intro q h
    have hq : q = s := Option.some.inj h
    subst hq
    exact ha
  | cons a path ih =>
    intro q h
    have h' : (match A.next q a with | some q' => A.run q' path | none => none) = some s := h
    cases hn : A.next q a with
    | none => rw [hn] at h'; cases h'
    | some q' =>
      rw [hn] at h'
      exact accepts_cons.mpr ⟨q', hn, ih q' h'⟩

theorem exists_of_accepts_append {rest : List Nat} :
    ∀ (pre : List Nat) (q : Nat), A.accepts q (pre ++ rest) = true →
      ∃ p, A.run q pre = some p ∧ A.accepts p rest = true := by
  intro pre
  induction pre with
  | nil => intro q h; exact ⟨q, rfl, h⟩
  | cons a pre ih =>
    intro q h
    obtain ⟨q', hn, h'⟩ := accepts_cons.mp h
    obtain ⟨p, hp, ha⟩ := ih q' h'
    refine ⟨p, ?_, ha⟩
    show (match A.next q a with | some q' => A.run q' pre | none => none) = some p
    rw [hn]
    exact hp

/-- Every edge from a node in range goes to a node in range. -/
def Bounded (A : Aut) : Prop := ∀ q, q < A.size → ∀ e ∈ A.edges q, e.2 < A.size

theorem run_lt (hb : A.Bounded) :
    ∀ (path : List Nat) (q s : Nat), q < A.size → A.run q path = some s → s < A.size := by
  intro path
  induction path with
  | nil =>
    intro q s hq h
    have e : q = s := Option.some.inj h
    subst e
    exact hq
  | cons a path ih =>
    intro q s hq h
    have h' : (match A.next q a with | some q' => A.run q' path | none => none) = some s := h
    cases hn : A.next q a with
    | none => rw [hn] at h'; cases h'
    | some q' =>
      rw [hn] at h'
      exact ih q' s (hb q hq _ (next_mem hn)) h'

theorem accepts_mono (hb : A.Bounded) (hi : A.inclOK = true) :
    ∀ (w : List Nat) (r s : Nat), r < A.size → A.sub r s = true → A.accepts r w = true → A.accepts s w = true := by
  have split : ∀ r s, r < A.size → A.sub r s = true → r = s ∨ A.inclAt r s = true := by
    intro r s hr h
    have e : A.sub r s = (Nat.beq r s || memNat s (A.sup r)) := rfl
    rw [e] at h
    cases hq : Nat.beq r s with
    | true => exact Or.inl (Nat.eq_of_beq_eq_true hq)
    | false =>
      rw [hq] at h
      exact Or.inr (all_mem (all_range hi hr) (mem_of_memNat h))
  intro w
  induction w with
  | nil =>
    intro r s hr hs ha
    cases split r s hr hs with
    | inl e => subst e; exact ha
    | inr hat => exact of_not_or (and_left hat) ha
  | cons a w ih =>
    intro r s hr hs ha
    cases split r s hr hs with
    | inl e => subst e; exact ha
    | inr hat =>
      obtain ⟨r', hn, ha'⟩ := accepts_cons.mp ha
      have hm := next_mem hn
      have h2 := all_mem (and_right hat) hm
      have h3 : (match A.next s a with | some s' => A.sub r' s' | none => false) = true := h2
      cases hs' : A.next s a with
      | none => rw [hs'] at h3; cases h3
      | some s' =>
        rw [hs'] at h3
        exact accepts_cons.mpr ⟨s', hs', ih r' s' (hb r hr _ hm) h3 ha'⟩

theorem covered_accepts (hb : A.Bounded) (hi : A.inclOK = true) {p r : Nat} {path post : List Nat}
    (hc : A.covered p path r = true) (hr : r < A.size) (ha : A.accepts r post = true) :
    A.accepts p (path ++ post) = true := by
  have h : (match A.run p path with | some s => A.sub r s | none => false) = true := hc
  cases hrun : A.run p path with
  | none => rw [hrun] at h; cases h
  | some s =>
    rw [hrun] at h
    exact accepts_of_run (accepts_mono hb hi post r s hr h ha) path p hrun

theorem closed (hb : A.Bounded) (hi : A.inclOK = true) (hn : A.nodesOK = true) {root : Nat} (hroot : root < A.size)
    {w w' : List Card} (hw : A.accepts root w = true) (hs : Step w w') : A.accepts root w' = true := by
  cases hs with
  | slide pre a b post hm =>
    obtain ⟨p, hrun, hp⟩ := exists_of_accepts_append pre root hw
    obtain ⟨c, hc, hp⟩ := accepts_cons.mp hp
    obtain ⟨r, hr, hp⟩ := accepts_cons.mp hp
    have hplt := run_lt hb pre root p hroot hrun
    have hcm := next_mem hc
    have hrm := next_mem hr
    have hclt : c < A.size := hb p hplt _ hcm
    have hrlt : r < A.size := hb c hclt _ hrm
    have h0 := all_range hn hplt
    unfold Aut.nodeOK at h0
    have h1' := all_mem h0 hcm
    have h1 : (A.edges c).all _ = true := h1'
    have h2' := all_mem h1 hrm
    have h2 := and_left h2'
    have h3 : A.covered p [b] r = true := of_not_or h2 hm
    exact accepts_of_run (covered_accepts hb hi h3 hrlt hp) pre root hrun
  | leap pre a x y b post hm =>
    obtain ⟨p, hrun, hp⟩ := exists_of_accepts_append pre root hw
    obtain ⟨c, hc, hp⟩ := accepts_cons.mp hp
    obtain ⟨c1, hc1, hp⟩ := accepts_cons.mp hp
    obtain ⟨c2, hc2, hp⟩ := accepts_cons.mp hp
    obtain ⟨r, hr, hp⟩ := accepts_cons.mp hp
    have hplt := run_lt hb pre root p hroot hrun
    have hcm := next_mem hc
    have hc1m := next_mem hc1
    have hc2m := next_mem hc2
    have hrm := next_mem hr
    have hclt : c < A.size := hb p hplt _ hcm
    have hc1lt : c1 < A.size := hb c hclt _ hc1m
    have hc2lt : c2 < A.size := hb c1 hc1lt _ hc2m
    have hrlt : r < A.size := hb c2 hc2lt _ hrm
    have h0 := all_range hn hplt
    unfold Aut.nodeOK at h0
    have h1' := all_mem h0 hcm
    have h1 : (A.edges c).all _ = true := h1'
    have h1c := all_mem h1 hc1m
    have h2' := and_right h1c
    have h2'' : (A.edges c1).all _ = true := h2'
    have h2a := all_mem h2'' hc2m
    have h2 : (A.edges c2).all _ = true := h2a
    have h3' := all_mem h2 hrm
    have h3 : A.covered p [b, x, y] r = true := of_not_or h3' hm
    exact accepts_of_run (covered_accepts hb hi h3 hrlt hp) pre root hrun

/-- The theorem every lost seed uses. -/
theorem lost_of_check {root : Nat} {d : List Card} (hc : A.check root = true) (h0 : A.accepts root d = true) :
    Lost d := by
  have h123 := and_left hc
  have h12 := and_left h123
  have hbd := and_left h12
  have hroot : root < A.size := Nat.le_of_ble_eq_true (and_left hbd)
  have hb : A.Bounded := by
    intro q hq e he
    have h1 := all_range (and_right hbd) hq
    have h2 := all_mem (show (A.edges q).all _ = true from h1) he
    exact Nat.le_of_ble_eq_true h2
  have hi := and_right h12
  have hn := and_right h123
  refine lost_of_invariant (fun w => A.accepts root w = true) h0
    (fun _ _ hw hs => closed hb hi hn hroot hw hs) ?_
  intro c ha
  obtain ⟨q, hq, ha'⟩ := accepts_cons.mp ha
  have h := all_mem (and_right hc) (next_mem hq)
  have hnot : (!(A.acc q)) = true := h
  have hacc : A.acc q = true := ha'
  rw [hacc] at hnot
  cases hnot

end Aut

/-! ## Part 5. A table for the kernel

`Aut` takes any functions, so this is only about speed: if the table says something else, the check fails or proves the
loss for the automaton that the table does describe. Nodes sit in a Braun tree, found by the bits of their number. -/

structure NodeData where
  edges : List (Nat × Nat)
  acc   : Bool
  sup   : List Nat

inductive Tree where
  | leaf
  | node (data : NodeData) (l r : Tree)

def Tree.get : Tree → Nat → NodeData
  | .leaf, _ => ⟨[], false, []⟩
  | .node d l r, i =>
    bif Nat.beq i 0 then d
    else bif Nat.beq (i % 2) 1 then l.get (i / 2) else r.get (i / 2 - 1)

def Aut.ofTree (size : Nat) (t : Tree) : Aut :=
  ⟨size, fun q => (t.get q).edges, fun q => (t.get q).acc, fun q => (t.get q).sup⟩

/-! ## Part 6. A deal can be won

Rules that were too strict would make every deal lost, so a winning line is replayed under the same `PileStep`. -/

/-- The move whose target is `k` piles from the left: a slide, or a leap if `leap` is true. -/
def applyMove : List Pile → Nat → Bool → Option (List Pile)
  | t :: p :: post, 0, false =>
    bif sameSuitOrRank t.top p.top then some (⟨p.top, p.below ++ t.top :: t.below⟩ :: post) else none
  | t :: x :: y :: p :: post, 0, true =>
    bif sameSuitOrRank t.top p.top then some (⟨p.top, p.below ++ t.top :: t.below⟩ :: x :: y :: post) else none
  | a :: rest, k + 1, leap => (applyMove rest k leap).map (a :: ·)
  | _, _, _ => none

theorem PileStep.cons (a : Pile) {s t : List Pile} (h : PileStep s t) : PileStep (a :: s) (a :: t) := by
  cases h with
  | slide pre t p post h => exact PileStep.slide (a :: pre) t p post h
  | leap pre t x y p post h => exact PileStep.leap (a :: pre) t x y p post h

theorem applyMove_step : ∀ {s : List Pile} {k : Nat} {leap : Bool} {s' : List Pile}, applyMove s k leap = some s' → PileStep s s'
  | t :: p :: post, 0, false, s', h => by
    simp only [applyMove] at h
    cases hm : sameSuitOrRank t.top p.top with
    | false => simp [hm] at h
    | true =>
      simp only [hm, Bool.cond_true, Option.some.injEq] at h
      subst h
      exact PileStep.slide [] t p post hm
  | t :: x :: y :: p :: post, 0, true, s', h => by
    simp only [applyMove] at h
    cases hm : sameSuitOrRank t.top p.top with
    | false => simp [hm] at h
    | true =>
      simp only [hm, Bool.cond_true, Option.some.injEq] at h
      subst h
      exact PileStep.leap [] t x y p post hm
  | a :: rest, k + 1, leap, s', h => by
    simp only [applyMove] at h
    cases hr : applyMove rest k leap with
    | none => simp [hr] at h
    | some r =>
      simp only [hr, Option.map_some, Option.some.injEq] at h
      subst h
      exact PileStep.cons a (applyMove_step hr)
  | [], _, _, _, h => by cases h
  | [_], 0, _, _, h => by cases h
  | [_, _], 0, true, _, h => by cases h
  | [_, _, _], 0, true, _, h => by cases h

def playLine : List Pile → List (Nat × Bool) → Option (List Pile)
  | s, [] => some s
  | s, m :: rest => match applyMove s m.1 m.2 with
    | some t => playLine t rest
    | none => none

theorem PileReach.head {s t u : List Pile} (hs : PileStep s t) (h : PileReach t u) : PileReach s u := by
  induction h with
  | refl => exact PileReach.tail (PileReach.refl _) hs
  | tail _ h2 ih => exact PileReach.tail ih h2

theorem playLine_reach : ∀ (line : List (Nat × Bool)) (s e : List Pile), playLine s line = some e → PileReach s e := by
  intro line
  induction line with
  | nil =>
    intro s e h
    have hs : s = e := Option.some.inj h
    subst hs
    exact PileReach.refl _
  | cons m rest ih =>
    intro s e h
    have h' : (match applyMove s m.1 m.2 with | some t => playLine t rest | none => none) = some e := h
    cases hm : applyMove s m.1 m.2 with
    | none => rw [hm] at h'; cases h'
    | some t =>
      rw [hm] at h'
      exact PileReach.head (applyMove_step hm) (ih t e h')

/-- Does this line of moves leave a single pile? -/
def wonBy (seed : Nat) (line : List (Nat × Bool)) : Bool :=
  match playLine (start (deal seed)) line with
  | some [_] => true
  | _ => false

theorem not_lost_of_wonBy {seed : Nat} {line : List (Nat × Bool)} (h : wonBy seed line = true) : ¬ DealLost (deal seed) := by
  intro hl
  unfold wonBy at h
  cases hp : playLine (start (deal seed)) line with
  | none => rw [hp] at h; cases h
  | some l =>
    rw [hp] at h
    cases l with
    | nil => cases h
    | cons p t =>
      cases t with
      | nil => exact hl p (playLine_reach line _ _ hp)
      | cons _ _ => cases h

end Accordion
