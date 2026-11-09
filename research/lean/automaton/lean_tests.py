# Writes Tests.lean, which must compile: a real win replayed under the rules, and bad certificates the kernel must refuse.
#
#   python research/lean/automaton/lean_tests.py out.lean     (from the repository root)
import sys

from check_certificates import build, deal_from_seed
from lean_certificates import superset_claims, table
from make_golden import solve

LOST = 3417058778


def main():
    seed = next(s for s in range(1000) if solve(deal_from_seed(s)) is not None)
    line = solve(deal_from_seed(seed))
    moves = ",".join(f"({i - d},{'true' if d == 3 else 'false'})" for i, d in line)

    edges, acc = build(deal_from_seed(LOST))
    sup = superset_claims(edges, acc)
    n = len(edges)
    good = table(edges, acc, sup)

    branching = next(q for q, out in enumerate(edges) if len(out) > 1)
    card = next(iter(edges[branching]))
    target = edges[branching].pop(card)
    missing_edge = table(edges, acc, sup)
    edges[branching][card] = target

    sup[1].add(5)
    false_claim = table(edges, acc, sup)

    won_edges, won_acc = build((0, 1))
    won = table(won_edges, won_acc, superset_claims(won_edges, won_acc))

    def refused(name, size=n):
        return f"(Aut.ofTree {size} {name})"

    text = f"""import Certificate
open Accordion

-- The rules allow wins: {len(line)} moves from seed {seed} leave one pile.
theorem won_{seed} : ¬ DealLost (deal {seed}) :=
  not_lost_of_wonBy (line := [{moves}]) (by decide +kernel)

certificate_table good "{good}"
certificate_table missingEdge "{missing_edge}"
certificate_table falseClaim "{false_claim}"
certificate_table wonRow "{won}"

-- The good table passes, and is for this deal only.
example : {refused("good")}.check 0 = true := by decide +kernel
example : {refused("good")}.accepts 0 (deal {LOST}) = true := by decide +kernel
example : {refused("good")}.accepts 0 (deal {LOST + 1}) = false := by decide +kernel

-- Bad certificates fail: an edge dropped, a false inclusion claim, a row that can be won, and a table with no nodes.
example : ({refused("missingEdge")}.check 0 && {refused("missingEdge")}.accepts 0 (deal {LOST})) = false := by decide +kernel
example : {refused("falseClaim")}.check 0 = false := by decide +kernel
example : {refused("wonRow", len(won_edges))}.check 0 = false := by decide +kernel
example : (Aut.mk 0 (fun _ => [(1, 5)]) (fun _ => false) (fun _ => .none)).check 0 = false := by decide +kernel

#print axioms won_{seed}
"""
    with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("win from seed", seed, "in", len(line), "moves")


if __name__ == "__main__":
    main()
