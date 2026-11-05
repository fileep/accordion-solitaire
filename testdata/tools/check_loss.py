# Re-checks a proven loss (`acc prove`) with the independent Python rules in make_golden.py: an exhaustive search over
# every position reachable from the deal, which must find no win and count exactly the dead positions of acc prove's
# plain run. It also prints what the search saw, to show why the deal is lost: dead ends by piles left, which top
# cards can ever stand in the first few slots, and every position with three piles or fewer.
#
#   python testdata/tools/check_loss.py 202346720 112249515        (from the repository root)
#
# Positions are kept as bytes, so tens of millions fit in a few GB; 19.4M take about 4 minutes.
import sys
import time
from collections import Counter, defaultdict

from make_golden import deal_from_seed, moves_of, name, play

SLOTS = 6


def check(seed):
    deal = tuple(deal_from_seed(seed))
    print(f"seed {seed}: {' '.join(name(c) for c in deal)}")
    start = time.time()
    seen, stack = {bytes(deal)}, [deal]
    tops = defaultdict(set)  # slot -> every card ever on top there
    ends = Counter()  # piles left -> positions with no move
    few = defaultdict(set)  # piles left (3 or fewer) -> positions
    while stack:
        s = stack.pop()
        for j in range(min(SLOTS, len(s))):
            tops[j].add(s[j])
        if len(s) <= 3:
            few[len(s)].add(s)
        stuck = True
        for i, d in moves_of(s):
            stuck = False
            t = play(s, i, d)
            if bytes(t) not in seen:
                seen.add(bytes(t))
                stack.append(t)
        if stuck:
            ends[len(s)] += 1
    print(f"  reachable positions {len(seen)}, won {ends[1] > 0}, fewest piles {min(ends)}"
          f" ({time.time() - start:.0f} s)")
    print("  dead ends by piles left:", ", ".join(f"{n}: {k}" for n, k in sorted(ends.items())[:10]))
    for j in range(SLOTS):
        print(f"  slot {j} top is only ever:", " ".join(sorted(name(c) for c in tops[j])))
    for n in (1, 2, 3):
        print(f"  positions with {n} piles: {len(few[n])}")
        for p in sorted(few[n]):
            print("    " + " ".join(name(c) for c in p) + ("" if any(moves_of(p)) else "  (no move)"))
    return ends[1] == 0


if __name__ == "__main__":
    lost = [check(int(seed)) for seed in sys.argv[1:]]
    sys.exit(0 if all(lost) else 1)
