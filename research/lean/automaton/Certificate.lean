/-
  Loads a certificate table without Lean's term elaborator, which is far too slow for a table of tens of thousands of
  edges. `certificate_table t "..."` builds the `Tree` term directly and gives it to the kernel, which type-checks it.
  Nothing here needs trusting: a table that is wrong fails `check`, or describes an automaton that proves the same thing.

  The text has one record per node, separated by `;`: `letter,target,letter,target|acc|sup,sup`, with acc 0 or 1.
-/
import Lean
import Accordion

namespace Accordion

open Lean Elab Command

private def natTy : Expr := mkConst ``Nat

private def pairTy : Expr := mkApp2 (mkConst ``Prod [.zero, .zero]) natTy natTy

private def listE (ty : Expr) (xs : List Expr) : Expr :=
  xs.foldr (fun x acc => mkApp3 (mkConst ``List.cons [.zero]) ty x acc) (mkApp (mkConst ``List.nil [.zero]) ty)

private def numbers (s : String) : List Nat :=
  (s.splitOn ",").filterMap fun t => if t.isEmpty then none else some t.toNat!

private def pairsE : List Nat → List Expr
  | a :: t :: rest =>
    mkApp4 (mkConst ``Prod.mk [.zero, .zero]) natTy natTy (mkRawNatLit a) (mkRawNatLit t) :: pairsE rest
  | _ => []

private partial def firstSet (xs : Array Nat) (bit i hi : Nat) : Nat :=
  if i < hi && !(xs[i]!).testBit bit then firstSet xs bit (i + 1) hi else i

/-- `NatSet` of the sorted numbers `xs[lo]` to `xs[hi - 1]`, split on the highest bit where they differ. -/
private partial def setE (xs : Array Nat) (lo hi : Nat) : Expr :=
  if lo ≥ hi then mkConst ``NatSet.none
  else if lo + 1 == hi then mkApp (mkConst ``NatSet.leaf) (mkRawNatLit xs[lo]!)
  else
    let bit := Nat.log2 (xs[lo]! ^^^ xs[hi - 1]!)
    let mid := firstSet xs bit lo hi
    mkApp3 (mkConst ``NatSet.node) (mkRawNatLit bit) (setE xs lo mid) (setE xs mid hi)

private def nodeE (record : String) : Expr :=
  match record.splitOn "|" with
  | [es, a, ss] =>
    mkApp3 (mkConst ``NodeData.mk) (listE pairTy (pairsE (numbers es)))
      (mkConst (if a == "1" then ``Bool.true else ``Bool.false))
      (setE (numbers ss).toArray 0 (numbers ss).length)
  | _ => panic! "bad certificate record"

private def emptyLeaf : Expr :=
  mkApp (mkConst ``Tree.leaf) (mkApp3 (mkConst ``NodeData.mk) (listE pairTy []) (mkConst ``Bool.false) (mkConst ``NatSet.none))

/-- The binary trie that `Tree.get` reads: this subtree holds the numbers `lo` to `lo + 2 ^ k - 1`. -/
private partial def treeE (nodes : Array Expr) (lo k : Nat) : Expr :=
  if k == 0 then
    if h : lo < nodes.size then mkApp (mkConst ``Tree.leaf) nodes[lo] else emptyLeaf
  else if lo ≥ nodes.size then emptyLeaf
  else mkApp3 (mkConst ``Tree.node) (mkRawNatLit (k - 1)) (treeE nodes lo (k - 1)) (treeE nodes (lo + 2 ^ (k - 1)) (k - 1))

elab "certificate_table " id:ident s:str : command => do
  let nodes := ((s.getString.splitOn ";").map nodeE).toArray
  liftCoreM <| addDecl <| .defnDecl {
    name := id.getId, levelParams := [], type := mkConst ``Tree,
    value := treeE nodes 0 (if nodes.size ≤ 1 then 0 else Nat.log2 (nodes.size - 1) + 1),
    hints := .abbrev, safety := .safe }

end Accordion
