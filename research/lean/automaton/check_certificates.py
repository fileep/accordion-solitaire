# Builds the automaton certificate of every proven loss with dawg.py and checks it with code that shares nothing with
# dawg.py: the rules and the generator are make_golden.py's, and the check is the one `check` states in Accordion.lean.
# It takes only the seeds, from the names of the loss records, so the records can change under a running batch.
#
#   python research/lean/automaton/check_certificates.py hard-deals/proofs research/lean/automaton/certificates.csv     (from the repository root)
#
# Before the losses it tests the check itself: random short rows against an explicit search, and damaged certificates.
import csv
import json
import random
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "testdata" / "tools"))

import dawg  # also drops this process to idle priority
from make_golden import deal_from_seed, matches, moves_of, play

FIELDS = ["seed", "positions", "record_positions", "nodes", "edges", "pairs", "lookups", "ok", "build_seconds", "check_seconds"]


def build(row):
    """The automaton of every position reachable from `row`, as (edges, acc): node 0 is the root, edges[q] maps a card to
    the next node, acc[q] says a word can end at q. Cards are numbered as in the rules, suit * 13 + rank."""
    dd = dawg.DD(6_000_000)
    layer = dawg.chain(dd, [c % 13 * 4 + c // 13 for c in row] + [dawg.END])
    root = 0
    while layer:
        root = dd.union(root, layer)
        layer = dd.succ(layer)
        dd.clear()
        if len(dd.T) > dd.cap // 2:
            layer, root = dd.compact([layer, root])

    number, order = {root: 0}, [root]
    for q in order:
        for letter, target in zip(dd.T[q][0::2], dd.T[q][1::2]):
            if letter != dawg.END and target not in number:
                number[target] = len(order)
                order.append(target)
    edges = [{l % 4 * 13 + l // 4: number[t] for l, t in zip(dd.T[q][0::2], dd.T[q][1::2]) if l != dawg.END} for q in order]
    return edges, [dawg.END in dd.T[q][0::2] for q in order]


def count(edges, acc):
    memo = {}

    def words(q):
        if q not in memo:
            memo[q] = acc[q] + sum(words(t) for t in edges[q].values())
        return memo[q]

    return words(0)


def check(edges, acc, row):
    """True if the three checks hold: `row` is accepted, no one-card word is, and every move from a word stays in the set."""
    lookups = 0
    wanted = []  # pairs (r, s): every word from r must be a word from s

    def read(q, path):
        nonlocal lookups
        for card in path:
            lookups += 1
            q = edges[q].get(card) if q is not None else None
        return q

    def fail():
        return False, 0, lookups

    end = read(0, row)
    if end is None or not acc[end] or any(acc[t] for t in edges[0].values()):
        return fail()
    for p, out in enumerate(edges):
        for a, c in out.items():
            for x, c1 in edges[c].items():
                if matches(a, x):
                    s = read(p, [x])
                    if s is None:
                        return fail()
                    wanted.append((c1, s))
                for y, c2 in edges[c1].items():
                    for b, r in edges[c2].items():
                        if matches(a, b):
                            s = read(p, [b, x, y])
                            if s is None:
                                return fail()
                            wanted.append((r, s))
    pairs = set()
    while wanted:
        r, s = wanted.pop()
        if (r, s) in pairs:
            continue
        pairs.add((r, s))
        if acc[r] and not acc[s]:
            return fail()
        for a, r1 in edges[r].items():
            s1 = read(s, [a])
            if s1 is None:
                return fail()
            wanted.append((r1, s1))
    return True, len(pairs), lookups


def has_win(row):
    seen, stack = {row}, [row]
    while stack:
        s = stack.pop()
        if len(s) == 1:
            return True
        for i, d in moves_of(s):
            t = play(s, i, d)
            if t not in seen:
                seen.add(t)
                stack.append(t)
    return False


def test_check(rng, seeds):
    rows = wins = 0
    for _ in range(400):
        row = tuple(rng.sample(range(52), rng.randint(8, 16)))
        edges, acc = build(row)
        win = has_win(row)
        if check(edges, acc, row)[0] == win or count(edges, acc) == 0:
            raise SystemExit(f"the check disagrees with the search on {row}")
        rows += 1
        wins += win

    damaged = 0
    for seed in seeds:
        row = deal_from_seed(seed)
        edges, acc = build(row)
        if not check(edges, acc, row)[0]:
            raise SystemExit(f"the certificate of seed {seed} fails before any damage")
        for _ in range(25):
            q = rng.randrange(len(edges))
            if edges[q] and rng.random() < 0.8:
                card = rng.choice(list(edges[q]))
                target = edges[q].pop(card)
                passed = check(edges, acc, row)[0]
                edges[q][card] = target
            elif acc[q]:
                acc[q] = False
                passed = check(edges, acc, row)[0]
                acc[q] = True
            else:
                continue
            if passed:
                raise SystemExit(f"a damaged certificate of seed {seed} passed (node {q})")
            damaged += 1
    print(f"check agrees with explicit search on {rows} random rows ({wins} with a win); rejected all {damaged} damaged certificates", flush=True)


def main():
    directory, out = Path(sys.argv[1]), Path(sys.argv[2])
    seeds = sorted(int(p.name.split("-")[1]) for p in directory.glob("seed-*-loss.json"))
    test_check(random.Random(20261109), seeds[:8])

    rows = []
    for seed in seeds:
        row = deal_from_seed(seed)
        start = time.time()
        edges, acc = build(row)
        built = time.time()
        ok, pairs, lookups = check(edges, acc, row)
        try:
            record = json.loads((directory / f"seed-{seed}-loss.json").read_text(encoding="utf-8"))["stats"]["proof"]["plain"]["dead_positions"]
        except (ValueError, KeyError):
            record = ""
        rows.append({"seed": seed, "positions": count(edges, acc), "record_positions": record, "nodes": len(edges),
                     "edges": sum(map(len, edges)), "pairs": pairs, "lookups": lookups, "ok": ok,
                     "build_seconds": round(built - start, 2), "check_seconds": round(time.time() - built, 2)})
    with out.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)

    mismatches = [r["seed"] for r in rows if r["record_positions"] != "" and r["record_positions"] != r["positions"]]
    print(f"{len(rows)} losses, {sum(r['ok'] for r in rows)} certificates pass; position counts differ from the record on {mismatches}")
    print(f"nodes in all {sum(r['nodes'] for r in rows)}, largest {max(r['nodes'] for r in rows)}; lookups in all {sum(r['lookups'] for r in rows)}, largest {max(r['lookups'] for r in rows)}")
    print(f"positions in all {sum(r['positions'] for r in rows)}, largest {max(r['positions'] for r in rows)}")


if __name__ == "__main__":
    main()
