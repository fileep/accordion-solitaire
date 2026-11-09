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

noncomputable section

/-! ## Part 4. The automaton certificate

The set of positions is the language of a deterministic automaton over cards, given as an untrusted table from outside
Lean. `sup r` lists nodes claimed to accept everything `r` accepts. The automaton is deterministic, so one pass over the
nodes is enough to show that moves stay in the language, and nothing is enumerated. Letters are card numbers as in
`sameSuitOrRank`, suit * 13 + rank; dawg.py numbers them rank * 4 + suit, so lean_certificates.py converts them.

What the kernel runs is written for it: recursors instead of compiled recursion and `match`, connectives expanded in place,
and each loop body its own small definition, so that a beta step copies little. -/

local macro "bAnd% " a:term:max b:term:max : term => `(Bool.rec (motive := fun _ => Bool) false $b $a)
local macro "bImp% " a:term:max b:term:max : term => `(Bool.rec (motive := fun _ => Bool) true $b $a)

/-- A set of node numbers, split on one bit at each branch. A node can have thousands of inclusion claims, and a list made
    the search for one of them the largest cost of a big certificate. -/
inductive NatSet where
  | none
  | leaf (x : Nat)
  | node (bit : Nat) (l r : NatSet)

def NatSet.mem (s : Nat) (t : NatSet) : Bool :=
  NatSet.rec (motive := fun _ => Bool) false (fun x => Nat.beq s x)
    (fun bit _ _ l r => Bool.rec (motive := fun _ => Bool) l r (Nat.beq (Nat.land (Nat.shiftRight s bit) 1) 1)) t

def NatSet.all (p : Nat → Bool) (t : NatSet) : Bool :=
  NatSet.rec (motive := fun _ => Bool) true (fun x => p x) (fun _ _ _ l r => bAnd% l r) t

structure Aut where
  size  : Nat
  edges : Nat → List (Nat × Nat)    -- (letter, target)
  acc   : Nat → Bool                -- a word may end here
  sup   : Nat → NatSet

def allB {α : Type} (p : α → Bool) (l : List α) : Bool :=
  List.rec (motive := fun _ => Bool) true (fun h _ ih => bAnd% (p h) ih) l

/-- The nodes `lo` to `lo + n - 1` all pass `p`. A large certificate is checked in many small theorems, because the kernel
    keeps its caches for a whole declaration and frees them after it. -/
def countB (p : Nat → Bool) (lo n : Nat) : Bool :=
  Nat.rec (motive := fun _ => Bool) true (fun i ih => bAnd% (p (Nat.add lo i)) ih) n

def lookup (a : Nat) (es : List (Nat × Nat)) : Option Nat :=
  List.rec (motive := fun _ => Option Nat) none
    (fun e _ ih => Prod.rec (motive := fun _ => Option Nat)
      (fun b q => Bool.rec (motive := fun _ => Option Nat) ih (some q) (Nat.beq a b)) e) es

def same (a b : Nat) : Bool :=
  Bool.rec (motive := fun _ => Bool) (Nat.beq (a % 13) (b % 13)) true (Nat.beq (a / 13) (b / 13))

namespace Aut

variable (A : Aut)

def next (q a : Nat) : Option Nat := lookup a (A.edges q)

def run (q : Nat) (w : List Nat) : Option Nat :=
  List.rec (motive := fun _ => Nat → Option Nat) (fun q => some q)
    (fun a _ ih q => match A.next q a with | some q' => ih q' | none => none) w q

def accepts (q : Nat) (w : List Nat) : Bool :=
  List.rec (motive := fun _ => Nat → Bool) (fun q => A.acc q)
    (fun a _ ih q => match A.next q a with | some q' => ih q' | none => false) w q

/-- Is everything `r` accepts claimed to be accepted by `s`? -/
def sub (r s : Nat) : Bool := Bool.rec (motive := fun _ => Bool) (NatSet.mem s (A.sup r)) true (Nat.beq r s)

def inclStep (s : Nat) (e : Nat × Nat) : Bool :=
  Prod.rec (motive := fun _ => Bool)
    (fun a r' => Option.rec (motive := fun _ => Bool) false (fun s' => A.sub r' s') (A.next s a)) e

def inclAt (r s : Nat) : Bool := bAnd% (bImp% (A.acc r) (A.acc s)) (allB (A.inclStep s) (A.edges r))

/-- After a slide of `b`: the rest `r` is in what `p` allows after `b`. -/
def cov1 (p b r : Nat) : Bool := Option.rec (motive := fun _ => Bool) false (fun s => A.sub r s) (A.next p b)

/-- After a leap of `b` over `x y`: the rest `r` is in what `p` allows after `b x y`. -/
def cov3 (p b x y r : Nat) : Bool :=
  Option.rec (motive := fun _ => Bool) false (fun s1 =>
    Option.rec (motive := fun _ => Bool) false (fun s2 =>
      Option.rec (motive := fun _ => Bool) false (fun s3 => A.sub r s3) (A.next s2 y)) (A.next s1 x)) (A.next p b)

def leap4 (p a x y : Nat) (h : Nat × Nat) : Bool :=
  Prod.rec (motive := fun _ => Bool) (fun b r => bImp% (same a b) (A.cov3 p b x y r)) h

def leap3 (p a x : Nat) (g : Nat × Nat) : Bool :=
  Prod.rec (motive := fun _ => Bool) (fun y c2 => allB (A.leap4 p a x y) (A.edges c2)) g

def step2 (p a : Nat) (f : Nat × Nat) : Bool :=
  Prod.rec (motive := fun _ => Bool)
    (fun x c1 => bAnd% (bImp% (same a x) (A.cov1 p x c1)) (allB (A.leap3 p a x) (A.edges c1))) f

def step1 (p : Nat) (e : Nat × Nat) : Bool :=
  Prod.rec (motive := fun _ => Bool) (fun a c => allB (A.step2 p a) (A.edges c)) e

/-- The local rule at node `p`: whatever a slide or a leap onto a letter from `p` produces is back in the language of `p`. -/
def nodeOK (p : Nat) : Bool := allB (A.step1 p) (A.edges p)

def boundEdge (e : Nat × Nat) : Bool := Nat.ble (e.2 + 1) A.size

def boundNode (q : Nat) : Bool := allB A.boundEdge (A.edges q)

/-- Reading a word can't leave the nodes that `check` looks at. Without this, a table with `size = 0` would pass. -/
def inBounds (root : Nat) : Bool := Nat.ble (root + 1) A.size && allB A.boundNode (List.range A.size)

def inclNode (r : Nat) : Bool := NatSet.all (A.inclAt r) (A.sup r)

def inclOK : Bool := allB A.inclNode (List.range A.size)

def nodesOK : Bool := allB A.nodeOK (List.range A.size)

def notAcc (e : Nat × Nat) : Bool := !(A.acc e.2)

/-- All of it: no word of one letter is accepted, so no position in the language is a win. -/
def check (root : Nat) : Bool :=
  A.inBounds root && A.inclOK && A.nodesOK && allB A.notAcc (A.edges root)

end Aut

theorem and_left {a b : Bool} (h : (a && b) = true) : a = true := by cases a <;> simp_all
theorem and_right {a b : Bool} (h : (a && b) = true) : b = true := by cases a <;> simp_all
theorem bAnd_left {a b : Bool} (h : Bool.rec (motive := fun _ => Bool) false b a = true) : a = true := by
  cases a with
  | false => cases (show false = true from h)
  | true => rfl
theorem bAnd_right {a b : Bool} (h : Bool.rec (motive := fun _ => Bool) false b a = true) : b = true := by
  cases a with
  | false => cases (show false = true from h)
  | true => exact h
theorem bImp_of {a b : Bool} (h : Bool.rec (motive := fun _ => Bool) true b a = true) (ha : a = true) : b = true := by
  subst ha
  exact h
theorem same_of {a b : Nat} (h : sameSuitOrRank a b = true) : same a b = true := by
  have e : sameSuitOrRank a b = (Nat.beq (a / 13) (b / 13) || Nat.beq (a % 13) (b % 13)) := rfl
  rw [e] at h
  cases hd : Nat.beq (a / 13) (b / 13) with
  | true =>
    show Bool.rec (motive := fun _ => Bool) (Nat.beq (a % 13) (b % 13)) true (Nat.beq (a / 13) (b / 13)) = true
    rw [hd]
  | false =>
    rw [hd] at h
    show Bool.rec (motive := fun _ => Bool) (Nat.beq (a % 13) (b % 13)) true (Nat.beq (a / 13) (b / 13)) = true
    rw [hd]
    exact h

theorem all_mem {α : Type} {p : α → Bool} {x : α} : ∀ {l : List α}, allB p l = true → x ∈ l → p x = true
  | [], _, hx => by cases hx
  | y :: t, h, hx => by
    have e : allB p (y :: t) = Bool.rec (motive := fun _ => Bool) false (allB p t) (p y) := rfl
    rw [e] at h
    cases hx with
    | head => exact bAnd_left h
    | tail _ hm => exact all_mem (bAnd_right h) hm
theorem all_range {n : Nat} {p : Nat → Bool} {x : Nat} (h : allB p (List.range n) = true) (hx : x < n) : p x = true :=
  all_mem h (List.mem_range.mpr hx)

theorem countB_spec {p : Nat → Bool} {lo : Nat} : ∀ {n : Nat}, countB p lo n = true → ∀ i, i < n → p (lo + i) = true
  | 0, _, i, hi => absurd hi (Nat.not_lt_zero i)
  | n + 1, h, i, hi => by
    have e : countB p lo (n + 1) = Bool.rec (motive := fun _ => Bool) false (countB p lo n) (p (Nat.add lo n)) := rfl
    rw [e] at h
    by_cases hin : i = n
    · subst hin
      exact bAnd_left h
    · exact countB_spec (bAnd_right h) i (by omega)

theorem cover_zero {p : Nat → Bool} : ∀ x, x < 0 → p x = true := fun x h => absurd h (Nat.not_lt_zero x)

theorem cover_more {p : Nat → Bool} {m k hi : Nat} (h : ∀ x, x < m → p x = true) (hc : countB p m k = true) (hk : m + k = hi) :
    ∀ x, x < hi → p x = true := by
  intro x hx
  by_cases hm : x < m
  · exact h x hm
  · have h1 := countB_spec hc (x - m) (by omega)
    have e : m + (x - m) = x := by omega
    rw [e] at h1
    exact h1

theorem lookup_mem {a q : Nat} : ∀ {es : List (Nat × Nat)}, lookup a es = some q → (a, q) ∈ es
  | [], h => by cases h
  | (b, r) :: rest, h => by
    have e : lookup a ((b, r) :: rest) = Bool.rec (motive := fun _ => Option Nat) (lookup a rest) (some r) (Nat.beq a b) := rfl
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

theorem NatSet.all_of_mem {p : Nat → Bool} {s : Nat} : ∀ {t : NatSet}, NatSet.mem s t = true → NatSet.all p t = true → p s = true
  | .none, h, _ => by cases h
  | .leaf x, h, ha => by
    have e : s = x := Nat.eq_of_beq_eq_true h
    subst e
    exact ha
  | .node bit l r, h, ha => by
    have e : NatSet.mem s (.node bit l r) =
        Bool.rec (motive := fun _ => Bool) (NatSet.mem s l) (NatSet.mem s r) (Nat.beq (Nat.land (Nat.shiftRight s bit) 1) 1) := rfl
    have ea : NatSet.all p (.node bit l r) = Bool.rec (motive := fun _ => Bool) false (NatSet.all p r) (NatSet.all p l) := rfl
    rw [e] at h
    rw [ea] at ha
    cases hb : Nat.beq (Nat.land (Nat.shiftRight s bit) 1) 1 with
    | true =>
      rw [hb] at h
      exact NatSet.all_of_mem h (bAnd_right ha)
    | false =>
      rw [hb] at h
      exact NatSet.all_of_mem h (bAnd_left ha)

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

theorem accepts_mono (hb : A.Bounded) (hi : ∀ x, x < A.size → A.inclNode x = true) :
    ∀ (w : List Nat) (r s : Nat), r < A.size → A.sub r s = true → A.accepts r w = true → A.accepts s w = true := by
  have split : ∀ r s, r < A.size → A.sub r s = true → r = s ∨ A.inclAt r s = true := by
    intro r s hr h
    have e : A.sub r s = Bool.rec (motive := fun _ => Bool) (NatSet.mem s (A.sup r)) true (Nat.beq r s) := rfl
    rw [e] at h
    cases hq : Nat.beq r s with
    | true => exact Or.inl (Nat.eq_of_beq_eq_true hq)
    | false =>
      rw [hq] at h
      have h0 := hi r hr
      have h1 : NatSet.all (A.inclAt r) (A.sup r) = true := h0
      exact Or.inr (NatSet.all_of_mem (show NatSet.mem s (A.sup r) = true from h) h1)
  intro w
  induction w with
  | nil =>
    intro r s hr hs ha
    cases split r s hr hs with
    | inl e => subst e; exact ha
    | inr hat =>
      have h1 := bAnd_left hat
      exact bImp_of h1 ha
  | cons a w ih =>
    intro r s hr hs ha
    cases split r s hr hs with
    | inl e => subst e; exact ha
    | inr hat =>
      obtain ⟨r', hn, ha'⟩ := accepts_cons.mp ha
      have hm := next_mem hn
      have h1 : allB (A.inclStep s) (A.edges r) = true := bAnd_right hat
      have h2 := all_mem h1 hm
      have h3 : Option.rec (motive := fun _ => Bool) false (fun s' => A.sub r' s') (A.next s a) = true := h2
      cases hs' : A.next s a with
      | none => rw [hs'] at h3; cases (show false = true from h3)
      | some s' =>
        rw [hs'] at h3
        exact accepts_cons.mpr ⟨s', hs', ih r' s' (hb r hr _ hm) h3 ha'⟩

theorem cov1_accepts (hb : A.Bounded) (hi : ∀ x, x < A.size → A.inclNode x = true) {p b r : Nat} {post : List Nat}
    (hc : A.cov1 p b r = true) (hr : r < A.size) (ha : A.accepts r post = true) :
    A.accepts p (b :: post) = true := by
  have h : Option.rec (motive := fun _ => Bool) false (fun s => A.sub r s) (A.next p b) = true := hc
  cases hs : A.next p b with
  | none => rw [hs] at h; cases (show false = true from h)
  | some s =>
    rw [hs] at h
    exact accepts_cons.mpr ⟨s, hs, accepts_mono hb hi post r s hr h ha⟩

theorem cov3_accepts (hb : A.Bounded) (hi : ∀ x, x < A.size → A.inclNode x = true) {p b x y r : Nat} {post : List Nat}
    (hc : A.cov3 p b x y r = true) (hr : r < A.size) (ha : A.accepts r post = true) :
    A.accepts p (b :: x :: y :: post) = true := by
  have h : Option.rec (motive := fun _ => Bool) false (fun s1 =>
    Option.rec (motive := fun _ => Bool) false (fun s2 =>
      Option.rec (motive := fun _ => Bool) false (fun s3 => A.sub r s3) (A.next s2 y)) (A.next s1 x)) (A.next p b) = true := hc
  cases h1 : A.next p b with
  | none => rw [h1] at h; cases (show false = true from h)
  | some s1 =>
    rw [h1] at h
    have h' : Option.rec (motive := fun _ => Bool) false (fun s2 =>
      Option.rec (motive := fun _ => Bool) false (fun s3 => A.sub r s3) (A.next s2 y)) (A.next s1 x) = true := h
    cases h2 : A.next s1 x with
    | none => rw [h2] at h'; cases (show false = true from h')
    | some s2 =>
      rw [h2] at h'
      have h'' : Option.rec (motive := fun _ => Bool) false (fun s3 => A.sub r s3) (A.next s2 y) = true := h'
      cases h3 : A.next s2 y with
      | none => rw [h3] at h''; cases (show false = true from h'')
      | some s3 =>
        rw [h3] at h''
        exact accepts_cons.mpr ⟨s1, h1, accepts_cons.mpr ⟨s2, h2, accepts_cons.mpr ⟨s3, h3, accepts_mono hb hi post r s3 hr h'' ha⟩⟩⟩

theorem closed (hb : A.Bounded) (hi : ∀ x, x < A.size → A.inclNode x = true) (hn : ∀ x, x < A.size → A.nodeOK x = true) {root : Nat} (hroot : root < A.size)
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
    have h00 := hn p hplt
    have h0 : allB (A.step1 p) (A.edges p) = true := h00
    have h1' := all_mem h0 hcm
    have h1 : allB (A.step2 p a) (A.edges c) = true := h1'
    have h2' := all_mem h1 hrm
    have h2 : Bool.rec (motive := fun _ => Bool) false (allB (A.leap3 p a b) (A.edges r))
      (Bool.rec (motive := fun _ => Bool) true (A.cov1 p b r) (same a b)) = true := h2'
    have h3 : A.cov1 p b r = true := bImp_of (bAnd_left h2) (same_of hm)
    exact accepts_of_run (cov1_accepts hb hi h3 hrlt hp) pre root hrun
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
    have h00 := hn p hplt
    have h0 : allB (A.step1 p) (A.edges p) = true := h00
    have h1' := all_mem h0 hcm
    have h1 : allB (A.step2 p a) (A.edges c) = true := h1'
    have h2' := all_mem h1 hc1m
    have h2 : Bool.rec (motive := fun _ => Bool) false (allB (A.leap3 p a x) (A.edges c1))
      (Bool.rec (motive := fun _ => Bool) true (A.cov1 p x c1) (same a x)) = true := h2'
    have h3 : allB (A.leap3 p a x) (A.edges c1) = true := bAnd_right h2
    have h4' := all_mem h3 hc2m
    have h4 : allB (A.leap4 p a x y) (A.edges c2) = true := h4'
    have h5' := all_mem h4 hrm
    have h5 : Bool.rec (motive := fun _ => Bool) true (A.cov3 p b x y r) (same a b) = true := h5'
    have h6 : A.cov3 p b x y r = true := bImp_of h5 (same_of hm)
    exact accepts_of_run (cov3_accepts hb hi h6 hrlt hp) pre root hrun

/-- The theorem every lost seed uses, with the node checks given in as many pieces as suit the kernel's memory. -/
theorem lost_of_parts {root : Nat} {d : List Card} (hr : Nat.ble (root + 1) A.size = true)
    (hbn : ∀ x, x < A.size → A.boundNode x = true) (hi : ∀ x, x < A.size → A.inclNode x = true) (hn : ∀ x, x < A.size → A.nodeOK x = true)
    (h1 : allB A.notAcc (A.edges root) = true) (h0 : A.accepts root d = true) : Lost d := by
  have hroot : root < A.size := Nat.le_of_ble_eq_true hr
  have hb : A.Bounded := by
    intro q hq e he
    have h2 := hbn q hq
    have h2' : allB A.boundEdge (A.edges q) = true := h2
    have h3 := all_mem h2' he
    exact Nat.le_of_ble_eq_true h3
  refine lost_of_invariant (fun w => A.accepts root w = true) h0
    (fun _ _ hw hs => closed hb hi hn hroot hw hs) ?_
  intro c ha
  obtain ⟨q, hq, ha'⟩ := accepts_cons.mp ha
  have h := all_mem h1 (next_mem hq)
  have hnot : (!(A.acc q)) = true := h
  have hacc : A.acc q = true := ha'
  rw [hacc] at hnot
  cases hnot

theorem lost_of_check {root : Nat} {d : List Card} (hc : A.check root = true) (h0 : A.accepts root d = true) :
    Lost d := by
  have h123 := and_left hc
  have h12 := and_left h123
  have hin : allB A.inclNode (List.range A.size) = true := and_right h12
  have hno : allB A.nodeOK (List.range A.size) = true := and_right h123
  have hbd := and_left h12
  exact lost_of_parts (and_left hbd) (fun x hx => all_range (and_right hbd) hx) (fun x hx => all_range hin hx) (fun x hx => all_range hno hx) (and_right hc) h0

end Aut

/-! ## Part 5. A table for the kernel

`Aut` takes any functions, so this is only about speed: if the table says something else, the check fails or proves the
loss for the automaton that the table does describe. Nodes are the leaves of a binary trie on the bits of their number. -/

structure NodeData where
  edges : List (Nat × Nat)
  acc   : Bool
  sup   : NatSet

inductive Tree where
  | leaf (data : NodeData)
  | node (bit : Nat) (l r : Tree)

def Tree.get (t : Tree) (i : Nat) : NodeData :=
  Tree.rec (motive := fun _ => NodeData) (fun d => d)
    (fun bit _ _ l r => Bool.rec (motive := fun _ => NodeData) l r (Nat.beq (Nat.land (Nat.shiftRight i bit) 1) 1)) t

def Aut.ofTree (size : Nat) (t : Tree) : Aut :=
  ⟨size, fun q => (t.get q).edges, fun q => (t.get q).acc, fun q => (t.get q).sup⟩

end

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
