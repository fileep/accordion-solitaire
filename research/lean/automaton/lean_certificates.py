# Writes a Lean file proving named seeds lost, from their automaton certificates (check_certificates.py builds them).
# Nothing in the output is trusted: a wrong table makes `decide +kernel` fail.
#
#   python research/lean/automaton/lean_certificates.py out.lean seed     (from the repository root)
import sys

from check_certificates import build, deal_from_seed


def superset_claims(edges, acc):
    """For every node, the nodes it must be included in: the pairs the moves ask for, and what those need in turn."""
    def read(q, path):
        for card in path:
            q = edges[q].get(card)
            if q is None:
                return None
        return q

    wanted = []
    for p, out in enumerate(edges):
        for a, c in out.items():
            for x, c1 in edges[c].items():
                if a // 13 == x // 13 or a % 13 == x % 13:
                    wanted.append((c1, read(p, [x])))
                for y, c2 in edges[c1].items():
                    for b, r in edges[c2].items():
                        if a // 13 == b // 13 or a % 13 == b % 13:
                            wanted.append((r, read(p, [b, x, y])))
    sup = [set() for _ in edges]
    while wanted:
        r, s = wanted.pop()
        if r == s or s in sup[r]:
            continue
        sup[r].add(s)
        wanted.extend((r1, edges[s][a]) for a, r1 in edges[r].items())
    return sup


def table(edges, acc, sup):
    """The text that `certificate_table` in Certificate.lean reads."""
    return ";".join(",".join(f"{a},{t}" for a, t in out.items()) + f"|{int(a)}|" + ",".join(map(str, sorted(s)))
                    for out, a, s in zip(edges, acc, sup))


NEWLINE = chr(10)
HEADER = "import Certificate\nopen Accordion\n\n"


MAX_RUN = 1000
NODE_WORK, INCL_WORK = 30000, 20000  # moves checked per theorem: the kernel frees its caches after each one


def chunks(work, limit):
    """Consecutive (first, count) runs of nodes whose work adds up to at most about `limit`. The kernel recurses once per node of a
    run, so a run is also kept short enough for its stack."""
    out, first, total = [], 0, 0
    for q, w in enumerate(work):
        if total and (total + w > limit or q - first >= MAX_RUN):
            out.append((first, q - first))
            first, total = q, 0
        total += w
    out.append((first, len(work) - first))
    return out


def cover(seed, kind, predicate, parts, lines):
    """Writes one theorem per run of nodes, and returns the proof that together they cover every node."""
    proof = "cover_zero"
    for i, (first, count) in enumerate(parts):
        lines += ["set_option maxHeartbeats 0 in",
                  f"theorem {kind}{seed}_{i} : countB a{seed}.{predicate} {first} {count} = true := by decide +kernel", ""]
        proof = f"(cover_more (hi := {first + count}) {proof} {kind}{seed}_{i} rfl)"
    return proof


def source(seed):
    """The Lean text proving `seed` lost, for a file that starts with HEADER, and the size of its certificate as (nodes, edges, claims)."""
    edges, acc = build(deal_from_seed(seed))
    sup = superset_claims(edges, acc)
    node_work = [1 + sum(1 + sum(1 + sum(1 + len(edges[c2]) for c2 in edges[c1].values()) for c1 in edges[c].values()) for c in out.values())
                 for out in edges]
    incl_work = [1 + len(s) * (1 + len(out)) for s, out in zip(sup, edges)]
    lines = [f'certificate_table t{seed} "{table(edges, acc, sup)}"', "", f"noncomputable def a{seed} : Aut := Aut.ofTree {len(edges)} t{seed}", ""]
    bounds = cover(seed, "b", "boundNode", chunks([1 + len(out) for out in edges], INCL_WORK), lines)
    incl = cover(seed, "i", "inclNode", chunks(incl_work, INCL_WORK), lines)
    nodes = cover(seed, "n", "nodeOK", chunks(node_work, NODE_WORK), lines)
    lines += ["set_option maxHeartbeats 0 in",
              f"theorem lost_{seed} : DealLost (deal {seed}) :=",
              f"  dealLost_of_lost (Aut.lost_of_parts (A := a{seed}) (root := 0) (by decide +kernel) {bounds} {incl} {nodes} (by decide +kernel) (by decide +kernel))", "",
              f"#print axioms lost_{seed}", "", ""]
    return NEWLINE.join(lines), (len(edges), sum(map(len, edges)), sum(map(len, sup)))


def main():
    text, size = source(int(sys.argv[2]))
    print("nodes, edges, claims:", size)
    with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as f:
        f.write(HEADER + text)


if __name__ == "__main__":
    main()
