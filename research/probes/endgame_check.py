"""Independent check of the endgame tables (PLAN.md Phase 5 track E): enumerate every suit/rank pattern of k cards
(labels in order of first appearance) and decide each by plain search with its own rules code, then print how many
are winnable. `acc endgame build` must print the same counts.

Usage: python endgame_check.py 7
"""
import sys
from functools import lru_cache

K = int(sys.argv[1]) if len(sys.argv) > 1 else 7


def patterns(k):
    """Every canonical pattern of length k as a tuple of (suit, rank) pairs."""
    out = []

    def extend(seq, s, r):
        if len(seq) == k:
            out.append(tuple(seq))
            return
        used = set(seq)
        options = [(cs, cr) for cs in range(s) for cr in range(r) if (cs, cr) not in used]
        if r < 13:
            options += [(cs, r) for cs in range(s)]
        if s < 4:
            options += [(s, cr) for cr in range(r)]
        if s < 4 and r < 13:
            options.append((s, r))
        for card in options:
            seq.append(card)
            extend(seq, max(s, card[0] + 1), max(r, card[1] + 1))
            seq.pop()

    extend([], 0, 0)
    return out


def match(a, b):
    return a[0] == b[0] or a[1] == b[1]


@lru_cache(maxsize=None)
def winnable(row):
    """Top cards left to right; a pile moves onto the pile 1 or 3 to its left when the top cards match."""
    if len(row) == 1:
        return True
    for i in range(1, len(row)):
        for d in (1, 3):
            if i - d >= 0 and match(row[i], row[i - d]):
                nxt = list(row)
                nxt[i - d] = row[i]
                del nxt[i]
                if winnable(tuple(nxt)):
                    return True
    return False


for k in range(1, K + 1):
    ps = patterns(k)
    wins = sum(winnable(p) for p in ps)
    print(f"{k:>3} piles  {len(ps):>10} patterns  {wins:>10} winnable ({100 * wins / len(ps):5.2f}%)")
