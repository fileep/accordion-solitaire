# Where do brute-force nodes sit by pile count? Tells how much an exact endgame
# table for <= K piles would cut. Uses the full cheap pruning set from deadends.py.
import random
from collections import Counter
from multiprocessing import Pool
from probe import moves, connected
from deadends import TESTS

FULL = list(TESTS.values())
DEALS, PER_DEAL, BUDGET = 30, 8, 400_000

def profile(s0):
    dead = set(); by_n = Counter(); nodes = 0
    def go(s):
        nonlocal nodes
        nodes += 1; by_n[len(s)] += 1
        if nodes > BUDGET: raise RuntimeError
        if len(s) == 1: return True
        if s in dead: return False
        if any(f(s) for f in FULL): dead.add(s); return False
        for t in moves(s):
            if go(t): return True
        dead.add(s); return False
    try: ok = go(s0)
    except RuntimeError: return None
    return ok, by_n

def work(args):
    di, k = args
    rd = random.Random(9000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 104729 + k); out = []
    for _ in range(PER_DEAL):
        s = tuple(deal)
        for _ in range(k):
            ms = [t for t in moves(s) if connected(t)]
            if not ms: break
            s = r.choice(ms)
        res = profile(s)
        if res: out.append((k, *res))
    return out

if __name__ == "__main__":
    with Pool(14) as pool:
        recs = [x for c in pool.map(work, [(d, k) for k in (26, 30, 34) for d in range(DEALS)]) for x in c]
    for k in (26, 30, 34):
        for label, want in (("refutations", False), ("first wins", True)):
            tot = Counter()
            for kk, ok, by_n in recs:
                if kk == k and ok == want: tot.update(by_n)
            s = sum(tot.values())
            if not s: continue
            cut = lambda K: sum(v for n, v in tot.items() if n < K) / s  # nodes strictly below K piles
            print(f"k={k} {label:<11}: nodes below 10 piles {cut(10):5.1%}, below 12 {cut(12):5.1%}, below 14 {cut(14):5.1%}")
