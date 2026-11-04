# Count canonical Accordion patterns of length k (sequences of k distinct cards
# up to relabeling of suits and ranks, since only equality of suit/rank matters).
# DP over (k, s used suits, r used ranks).
from collections import defaultdict
import math

g = {(0, 0): 1}
print(f"{'k':>3} {'canonical patterns':>22} {'bits (MB) for 1-bit table':>28} {'raw sequences 52!/(52-k)!':>28}")
for k in range(0, 26):
    if k > 0:
        n = sum(g.values())
        raw = math.perm(52, k)
        print(f"{k:>3} {n:>22,} {n/8/1e6:>28,.1f} {raw:>28.3e}")
    ng = defaultdict(int)
    for (s, r), c in g.items():
        if s * r - k > 0:
            ng[(s, r)] += c * (s * r - k)
        if r < 13 and s > 0:
            ng[(s, r + 1)] += c * s
        if s < 4 and r > 0:
            ng[(s + 1, r)] += c * r
        if s < 4 and r < 13:
            ng[(s + 1, r + 1)] += c
    g = ng
