/-
  Accordion solitaire: rules, generator, and two ways to prove a deal lost.

  STATUS: DRAFT, NEVER COMPILED. Lean was not available when this was written. Expect syntax and name errors.
  `sorry` marks proofs that are only outlined. No Mathlib; core Lean 4 only.

  Part 1  rules on piles, and the reduction to top cards
  Part 2  the deal generator
  Part 3  lost-by-invariant: the only theorem a loss really needs
  Part 4  the automaton certificate: the checker for lost deals (recommended)
  Part 5  the splitting theorem, including the leap over a short block (optional, see the report)
-/

namespace Accordion

/-! ## Part 1. Rules -/

/-- A card is its index 0..51: suit * 13 + rank, as `Card::index` in crates/core/src/card.rs. -/
abbrev Card := Nat

/-- Two cards match if they share a suit or a rank. -/
def matches (a b : Card) : Bool :=
  Nat.beq (a / 13) (b / 13) || Nat.beq (a % 13) (b % 13)

/-- A pile: its top card, then the rest top-down. Never empty by construction. -/
structure Pile where
  top   : Card
  below : List Card

/-- One legal move on piles: a whole pile goes onto the pile 1 or 3 places to its left, if the two top cards match.
    Moving pile `p` onto `t` gives a pile whose top is p's top; what lay in `t` stays under it. -/
inductive PileStep : List Pile → List Pile → Prop
  | slide (pre : List Pile) (t p : Pile) (post : List Pile) (h : matches t.top p.top = true) :
      PileStep (pre ++ t :: p :: post) (pre ++ ⟨p.top, p.below ++ t.top :: t.below⟩ :: post)
  | leap (pre : List Pile) (t x y p : Pile) (post : List Pile) (h : matches t.top p.top = true) :
      PileStep (pre ++ t :: x :: y :: p :: post)
               (pre ++ ⟨p.top, p.below ++ t.top :: t.below⟩ :: x :: y :: post)

/-- The same moves on the row of top cards alone. -/
inductive Step : List Card → List Card → Prop
  | slide (pre : List Card) (a b : Card) (post : List Card) (h : matches a b = true) :
      Step (pre ++ a :: b :: post) (pre ++ b :: post)
  | leap (pre : List Card) (a x y b : Card) (post : List Card) (h : matches a b = true) :
      Step (pre ++ a :: x :: y :: b :: post) (pre ++ b :: x :: y :: post)

/-- What is under a pile never affects the moves, so a game of piles projects onto a game of top cards. -/
theorem tops_step {s t : List Pile} (h : PileStep s t) :
    Step (s.map Pile.top) (t.map Pile.top) := by
  sorry -- cases h; simp [List.map_append]; exact Step.slide .. / Step.leap ..

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

theorem dealLost_of_lost {deal : List Card} (h : Lost deal) : DealLost deal := by
  sorry -- induction on PileReach with tops_step gives Reach ((start deal).map Pile.top) (map top of the end),
        -- and (start deal).map Pile.top = deal by List.map_map and List.map_id.

/-! ## Part 2. The generator (crates/core/src/rng.rs, deal.rs)

Everything is plain `Nat` modulo 2^64, because the kernel has fast big-number `Nat.add`, `Nat.mul`, `Nat.mod`,
`Nat.land`, `Nat.lor`, `Nat.xor`, `Nat.shiftLeft`, `Nat.shiftRight`. Test: the 23 seed/card pairs in `hard-deals/proofs`. -/

def wrap (x : Nat) : Nat := x % 18446744073709551616
def rotl (x k : Nat) : Nat := wrap (Nat.lor (Nat.shiftLeft x k) (Nat.shiftRight x (64 - k)))

/-- SplitMix64: returns the new state and one output. -/
def splitmix (s : Nat) : Nat × Nat :=
  let s' := wrap (s + 0x9E3779B97F4A7C15)
  let z := wrap (Nat.xor s' (Nat.shiftRight s' 30) * 0xBF58476D1CE4E5B9)
  let z := wrap (Nat.xor z (Nat.shiftRight z 27) * 0x94D049BB133111EB)
  (s', Nat.xor z (Nat.shiftRight z 31))

structure Xo where (s0 s1 s2 s3 : Nat)

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

/-- Lemire's rejection, with fuel: a redraw has chance about 2^-58, so fuel is never used up on a real seed.
    `deal seed` is checked against the recorded cards, so running out of fuel could not go unnoticed. -/
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

/-! ## Part 3. Lost by invariant: the only theorem a loss needs -/

theorem reach_inv {S : List Card → Prop} (hc : ∀ w w', S w → Step w w' → S w')
    {u v : List Card} (hu : S u) (h : Reach u v) : S v := by
  induction h with
  | refl => exact hu
  | tail _ hs ih => exact hc _ _ ih hs

theorem lost_of_invariant {d : List Card} (S : List Card → Prop)
    (h0 : S d) (hc : ∀ w w', S w → Step w w' → S w') (hw : ∀ c, ¬ S [c]) : Lost d :=
  fun c hr => hw c (reach_inv hc h0 hr)

/-! ## Part 4. The automaton certificate (recommended)

`S` is the language of a small deterministic automaton over cards, given as an untrusted table that comes from
outside Lean. The checker needs one pass over the nodes (the automaton is deterministic, so the pairs dawg.py's
`verify` walks from (root, root) are always (p, p), and no pair-of-nodes search is needed) plus one pass over a table
of inclusion pairs.  Nothing is enumerated. Letters are card numbers as in `matches`, suit * 13 + rank; dawg.py numbers
them rank * 4 + suit, so check_certificates.py converts them. -/

structure Aut where
  size  : Nat
  edges : Nat → List (Nat × Nat)    -- (letter, target); in the real checker, a lookup in a balanced tree of packed `Nat`s
  acc   : Nat → Bool                -- the end-of-word flag

def Aut.next (A : Aut) (q a : Nat) : Option Nat :=
  ((A.edges q).find? (fun e => Nat.beq e.1 a)).map Prod.snd

/-- Follow a path of letters from `q`. -/
def Aut.run (A : Aut) : Nat → List Nat → Option Nat
  | q, [] => some q
  | q, a :: w => match A.next q a with
    | some q' => A.run q' w
    | none => none

def Aut.accepts (A : Aut) (q : Nat) (w : List Nat) : Bool :=
  match A.run q w with
  | some p => A.acc p
  | none => false

/-- `I` is a table of pairs (r, s), each claiming language(r) is inside language(s). Check every pair locally.
    `I.contains` is written for reading: the kernel version looks the pair up in a sorted tree, and the soundness proof
    only uses "if the lookup says yes, the pair is in the table that was checked", so that swap is cheap. -/
def inclOK (A : Aut) (I : List (Nat × Nat)) : Bool :=
  I.all fun (r, s) =>
    (!(A.acc r) || A.acc s) &&
    (A.edges r).all fun (a, r') =>
      match A.next s a with
      | some s' => I.contains (r', s')
      | none => false

/-- Is (r, the node reached from p along `path`) in the table? -/
def covered (A : Aut) (I : List (Nat × Nat)) (p : Nat) (path : List Nat) (r : Nat) : Bool :=
  match A.run p path with
  | some s => I.contains (r, s)
  | none => false

/-- The local rule at node `p`. Whatever a slide or a leap with target letter `a` produces is back in the language of p. -/
def nodeOK (A : Aut) (I : List (Nat × Nat)) (p : Nat) : Bool :=
  (A.edges p).all fun (a, c) =>
    ((A.edges c).all fun (b, r) => !(matches a b) || covered A I p [b] r) &&
    ((A.edges c).all fun (x, c1) =>
      (A.edges c1).all fun (y, c2) =>
        (A.edges c2).all fun (b, r) => !(matches a b) || covered A I p [b, x, y] r)

/-- Reading a word can't leave the nodes that `check` looks at. Without this, a table with `size = 0` passes `check`
    whatever its edges are, and would prove any deal lost. -/
def inBounds (A : Aut) (root : Nat) : Bool :=
  Nat.blt root A.size &&
  (List.range A.size).all fun q => (A.edges q).all fun (_, c) => Nat.blt c A.size

def check (A : Aut) (I : List (Nat × Nat)) (root : Nat) : Bool :=
  inBounds A root &&
  inclOK A I &&
  (List.range A.size).all (nodeOK A I) &&
  (A.edges root).all fun (_, c) => !(A.acc c)     -- no word of one letter: no win

theorem accepts_mono {A I} (h : inclOK A I = true) :
    ∀ {r s w}, (r, s) ∈ I → A.accepts r w = true → A.accepts s w = true := by
  sorry -- induction on w, generalising r and s; both cases are one unfolding of `inclOK`.

/-- `pre` is read to some node `p`, and the rest of the word is then read from `p`. -/
theorem accepts_append {A : Aut} {q} (pre rest : List Nat) (h : A.accepts q (pre ++ rest) = true) :
    ∃ p, A.run q pre = some p ∧ A.accepts p rest = true := by
  sorry -- induction on pre, generalising q.

theorem check_closed {A I root} (h : check A I root = true) :
    ∀ w w', A.accepts root w = true → Step w w' → A.accepts root w' = true := by
  /- Outline, slide case, w = pre ++ a :: b :: post:
     1. accepts_append gives p = run root pre, and `a :: b :: post` accepted from p; p < A.size by inBounds and induction on pre.
     2. So p has edge (a, c), c has edge (b, r), and post is accepted from r.
     3. nodeOK at p with (a, c) and (b, r) gives (r, s) ∈ I where s = next p b.
     4. accepts_mono: post is accepted from s, so `b :: post` from p, so pre ++ b :: post from root.
     Leap case, w = pre ++ a :: x :: y :: b :: post: the same with the path [b, x, y].  -/
  sorry

/-- The theorem each lost seed uses. `by decide +kernel` proves the two hypotheses, no `native_decide`. -/
theorem lost_of_check {A I root d} (hc : check A I root = true) (h0 : A.accepts root d = true) : Lost d := by
  sorry -- lost_of_invariant with S := fun w => A.accepts root w = true.
        -- A word [c] would need an edge from root to an accepting node, excluded by the last clause of `check`.

/-! ## Part 5. The splitting theorem (optional: the certificate above covers these deals too)

Blocks are consecutive runs of the row. The hypothesis quantifies over the row made by joining one reachable
position per block, so it says in one line what the hand argument took many: no legal move of the joined row
crosses a boundary. -/

/-- `qs` is `ps` with exactly one block taking one step inside itself. -/
def InternalStep (ps qs : List (List Card)) : Prop :=
  ∃ pre b b' post, ps = pre ++ b :: post ∧ qs = pre ++ b' :: post ∧ Step b b'

/-- One reachable position for each block, in order. -/
def Choice : List (List Card) → List (List Card) → Prop
  | [], [] => True
  | b :: bs, p :: ps => Reach b p ∧ Choice bs ps
  | _, _ => False

/-- Closed: every legal move of any joined row is inside one block. -/
def Closed (blocks : List (List Card)) : Prop :=
  ∀ ps, Choice blocks ps → ∀ w', Step ps.flatten w' →
    ∃ qs, w' = qs.flatten ∧ InternalStep ps qs

theorem step_ne_nil {w w'} (h : Step w w') (hne : w ≠ []) : w' ≠ [] := by
  sorry -- each step's result has a letter b.

/-- A deal that splits into two or more closed blocks is lost. -/
theorem lost_of_closed_split {blocks : List (List Card)}
    (hclosed : Closed blocks) (two : 2 ≤ blocks.length) (ne : ∀ b ∈ blocks, b ≠ []) :
    Lost blocks.flatten := by
  /- S w := ∃ ps, Choice blocks ps ∧ w = ps.flatten.
     Closed turns any step of ps.flatten into one block's step, which extends that block's Reach by Reach.tail.
     No win: each part is non-empty (step_ne_nil, along Reach), and there are at least 2 parts, so the length is at least 2. -/
  sorry

/-! ### The finite test, and the trap

A crossing move starts in a block's first 3 piles. A mover at slot i (0, 1 or 2) of a block, moving d = 1 or d = 3 left
with i < d, lands p = d - i places left of the block's left edge.

THE TRAP. "p places left" is not always in the block next door. When the left neighbour currently has fewer than p
piles, the target is in the block beyond it, and so on. `canSit` must follow that recursion; skipping it produced 818
false splits out of 1,040 positions in the mutation test of the earlier study.

The data for a block: its cards `head i` at slot i, `tail p` at p-th slot from the right, and `lens` the pile counts it
can have, all taken from the block's own reachable positions. -/

structure BlockInfo where
  head : Nat → List Card     -- cards that can be at slot 0, 1, 2
  tail : Nat → List Card     -- cards that can be p-th from the right, p = 1, 2, 3
  lens : List Nat            -- pile counts the block can have

/-- Cards that can sit `p` slots to the left of the boundary. `left` lists the blocks to the left, nearest first.
    The second line is the leap over a short block: nearest block has length l < p, so the target is p - l further. -/
def canSit : List BlockInfo → Nat → List Card
  | [], _ => []
  | B :: rest, p =>
    B.tail p ++ (B.lens.filter (· < p)).flatMap fun l => canSit rest (p - l)

/-- No mover in the first 3 piles of this block can land on anything to its left. -/
def noCrossing (left : List BlockInfo) (B : BlockInfo) : Bool :=
  [(0, 1), (0, 3), (1, 3), (2, 3)].all fun (i, d) =>
    (B.head i).all fun m => (canSit left (d - i)).all fun t => !(matches m t)

/-- The finite check implies `Closed`. Outline: take a Step of the joined row. If mover and target are in the same
    block, it is an InternalStep (the two are contiguous inside that block). Otherwise the mover is in a block's first 3
    piles and its target is d - i slots left of the block's edge. Induct over the blocks to its left: that is exactly `canSit`,
    so `noCrossing` contradicts the match. Least-sure step: this induction, where the part boundaries in `ps.flatten` have to
    be matched with slot arithmetic. -/
theorem closed_of_noCrossing (blocks : List (List Card)) (infos : List BlockInfo)
    -- infos describe the blocks soundly, and `noCrossing` holds at every block
    : True := trivial -- statement to be completed with the block soundness hypotheses; left open on purpose

end Accordion
