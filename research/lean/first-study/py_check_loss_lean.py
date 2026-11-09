# Re-checks a proven loss with the independent Python engine's rules (testdata/tools/make_golden.py: moves_of, play,
# deal_from_seed): the same exhaustive search as its reachable(), but holding each seen position as bytes and no
# parent links, so tens of millions of positions fit in memory.
# Usage: python py_check_loss_lean.py <repo root> <seed> <expected cards>
import sys
import time

sys.path.insert(0, sys.argv[1] + "/testdata/tools")
import make_golden as g

seed, cards = int(sys.argv[2]), sys.argv[3]
deal = tuple(g.deal_from_seed(seed))
print("deal matches the dossier:", " ".join(g.name(c) for c in deal) == cards, flush=True)
t = time.time()
seen = {bytes(deal)}
stack, win, fewest = [deal], False, len(deal)
while stack:
    s = stack.pop()
    if len(s) == 1:
        win = True
    fewest = min(fewest, len(s))
    for i, d in g.moves_of(s):
        n = g.play(s, i, d)
        k = bytes(n)
        if k not in seen:
            seen.add(k)
            stack.append(n)
    if len(seen) % 1_000_000 < 6:
        print(f"  {len(seen)} positions, {time.time() - t:.0f} s", flush=True)
print(f"reachable positions {len(seen)}, winnable {win}, fewest piles {fewest}, {time.time() - t:.1f} s")
