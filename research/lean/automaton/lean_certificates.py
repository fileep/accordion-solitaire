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


HEADER = "import Certificate\nopen Accordion\n\n"


def source(seed):
    """The Lean text proving `seed` lost, for a file that starts with HEADER, and the size of its certificate as (nodes, edges, claims)."""
    edges, acc = build(deal_from_seed(seed))
    sup = superset_claims(edges, acc)
    lines = [f'certificate_table t{seed} "{table(edges, acc, sup)}"', "",
             "set_option maxHeartbeats 0 in",
             f"theorem lost_{seed} : DealLost (deal {seed}) :=",
             f"  dealLost_of_lost (Aut.lost_of_check (A := Aut.ofTree {len(edges)} t{seed}) (root := 0) (by decide +kernel) (by decide +kernel))", "",
             f"#print axioms lost_{seed}", "", ""]
    return "\n".join(lines), (len(edges), sum(map(len, edges)), sum(map(len, sup)))


def main():
    text, size = source(int(sys.argv[2]))
    print("nodes, edges, claims:", size)
    with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as f:
        f.write(HEADER + text)


if __name__ == "__main__":
    main()
