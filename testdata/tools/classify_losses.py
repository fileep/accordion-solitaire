# Explains proven losses: for every `loss` record in a directory, an exhaustive search with the independent Python rules
# in make_golden.py over every position reachable from the deal. It must find no win and count exactly the dead positions
# of the record's plain run, so this is also a full re-check of the record. It writes one row per deal.
#
#   python testdata/tools/classify_losses.py hard-deals/proofs research/runs/losses.csv [jobs]     (from the repository root)
#   python testdata/tools/classify_losses.py hard-deals-chacha/proofs research/runs/losses-chacha.csv [jobs] --chacha
#
# --chacha is for the second generator's losses: their cards are not the main generator's deal of the seed, so that check is
# skipped. Lean checks them against ChaCha20 instead (research/lean/automaton/ChaCha.lean).
# Each row records which cards ever interact, taken over every move of every reachable position. A move joins two cards,
# the mover and the card it covers. Cards never joined form separate groups, which explain the loss but are no proof apart
# from the search that found them: a pile only ever holds cards of one group, so at least one pile per group is always left.
# Two cards that are never covered explain it as well, since a win covers every card but one, and so does a first card that
# is never covered, since the first card never moves and the last card standing is one that moved.
import csv
import functools
import json
import multiprocessing
import sys
import time
from collections import Counter
from pathlib import Path

from make_golden import deal_from_seed, moves_of, name, play

NUMBER = {name(i): i for i in range(52)}
FIELDS = ["seed", "positions", "fewest_piles", "kind", "groups", "group_sizes", "never_covered", "first_covered",
          "never_moved", "frozen", "two_pile_positions", "seconds", "cards"]


def analyse(path, chacha=False):
    record = json.loads(Path(path).read_text(encoding="utf-8"))
    deal = tuple(NUMBER[token] for token in record["deal"]["cards"].split())
    seed = record["deal"].get("seed")
    if seed is not None and not chacha and list(deal) != deal_from_seed(seed):
        raise ValueError(f"{path}: the cards are not the deal of seed {seed}")

    start = time.time()
    seen, stack = {bytes(deal)}, [deal]
    joined = bytearray(52 * 52)  # mover * 52 + covered: this move has been seen
    group = list(range(52))
    ends, piles = Counter(), Counter()

    def find(c):
        while group[c] != c:
            group[c] = group[group[c]]
            c = group[c]
        return c

    while stack:
        s = stack.pop()
        piles[len(s)] += 1
        stuck = True
        for i, d in moves_of(s):
            stuck = False
            mover, covered = s[i], s[i - d]
            if not joined[mover * 52 + covered]:
                joined[mover * 52 + covered] = 1
                group[find(mover)] = find(covered)
            t = play(s, i, d)
            if bytes(t) not in seen:
                seen.add(bytes(t))
                stack.append(t)
        if stuck:
            ends[len(s)] += 1
    if ends[1]:
        raise ValueError(f"{path}: the search found a win")

    expected = record.get("stats", {}).get("proof", {}).get("plain", {}).get("dead_positions")
    if expected is not None and expected != len(seen):
        raise ValueError(f"{path}: {len(seen)} reachable positions, but the plain run has {expected} dead")

    moved = [any(joined[c * 52 + o] for o in range(52)) for c in range(52)]
    covered = [any(joined[o * 52 + c] for o in range(52)) for c in range(52)]
    sizes = sorted(Counter(find(c) for c in range(52)).values(), reverse=True)
    never_covered = [c for c in deal if not covered[c]]
    first_covered = covered[deal[0]]
    if len(sizes) > 1:
        kind = "split"
    elif len(never_covered) > 1 or not first_covered:
        kind = "uncovered"
    else:
        kind = "endgame"
    return {
        "seed": seed,
        "positions": len(seen),
        "fewest_piles": min(ends),
        "kind": kind,
        "groups": len(sizes),
        "group_sizes": " ".join(map(str, sizes)),
        "never_covered": " ".join(name(c) for c in never_covered),
        "first_covered": first_covered,
        "never_moved": sum(not moved[c] for c in deal),
        "frozen": sum(not moved[c] and not covered[c] for c in deal),
        "two_pile_positions": piles[2],
        "seconds": round(time.time() - start, 1),
        "cards": record["deal"]["cards"],
    }


def main():
    chacha = "--chacha" in sys.argv
    args = [a for a in sys.argv if a != "--chacha"]
    directory, out = Path(args[1]), Path(args[2])
    jobs = int(args[3]) if len(args) > 3 else multiprocessing.cpu_count() // 2
    paths = sorted(directory.glob("*-loss.json"), key=lambda p: json.loads(p.read_text(encoding="utf-8"))["deal"].get("seed", 0))
    with multiprocessing.Pool(jobs) as pool:
        rows = pool.map(functools.partial(analyse, chacha=chacha), paths, chunksize=1)
    with out.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)

    print(f"{len(rows)} losses checked: no wins, and every position count matches its record")
    print(f"{sum(r['positions'] for r in rows)} positions in all")
    for kind, n in Counter(r["kind"] for r in rows).most_common():
        fewest = sorted(r["fewest_piles"] for r in rows if r["kind"] == kind)
        print(f"  {kind}: {n} deals, fewest piles reached {fewest[0]}..{fewest[-1]} (median {fewest[len(fewest) // 2]})")


if __name__ == "__main__":
    main()
