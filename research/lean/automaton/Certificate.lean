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

private def listE (ty : Expr) (xs : List Expr) : Expr :=
  xs.foldr (fun x acc => mkApp3 (mkConst ``List.cons [.zero]) ty x acc) (mkApp (mkConst ``List.nil [.zero]) ty)

private def numbers (s : String) : List Nat :=
  (s.splitOn ",").filterMap fun t => if t.isEmpty then none else some t.toNat!

private def pairsE : List Nat → List Expr
  | a :: t :: rest =>
    mkApp4 (mkConst ``Prod.mk [.zero, .zero]) natTy natTy (mkRawNatLit a) (mkRawNatLit t) :: pairsE rest
  | _ => []

private def nodeE (record : String) : Expr :=
  match record.splitOn "|" with
  | [es, a, ss] =>
    let pairTy := mkApp2 (mkConst ``Prod [.zero, .zero]) natTy natTy
    mkApp3 (mkConst ``NodeData.mk) (listE pairTy (pairsE (numbers es)))
      (mkConst (if a == "1" then ``Bool.true else ``Bool.false))
      (listE natTy ((numbers ss).map mkRawNatLit))
  | _ => panic! "bad certificate record"

/-- The Braun tree that `Tree.get` reads: a subtree holds the node numbers `first + j * step`. -/
private partial def treeE (nodes : Array Expr) (first step : Nat) : Expr :=
  if h : first < nodes.size then
    mkApp3 (mkConst ``Tree.node) nodes[first] (treeE nodes (first + step) (2 * step))
      (treeE nodes (first + 2 * step) (2 * step))
  else mkConst ``Tree.leaf

elab "certificate_table " id:ident s:str : command => do
  let nodes := ((s.getString.splitOn ";").map nodeE).toArray
  liftCoreM <| addDecl <| .defnDecl {
    name := id.getId, levelParams := [], type := mkConst ``Tree, value := treeE nodes 0 1,
    hints := .abbrev, safety := .safe }

end Accordion
