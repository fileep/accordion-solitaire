# GPU feasibility: does brute force on small residuals need a transposition table?
# Compare DFS nodes with and without the dead-state cache (cheap pruning on in both).
import random, sys
from multiprocessing import Pool
from probe import moves, connected
from deadends import TESTS

FULL = list(TESTS.values())
DEALS, PER_DEAL, BUDGET = 30, 6, 300_000

def dfs(s0, use_tt):
    dead = set(); nodes = 0
    def go(s):
        nonlocal nodes
        nodes += 1
        if nodes > BUDGET: raise RuntimeError
        if len(s) == 1: return True
        if use_tt and s in dead: return False
        if any(f(s) for f in FULL):
            if use_tt: dead.add(s)
            return False
        for t in moves(s):
            if go(t): return True
        if use_tt: dead.add(s)
        return False
    try: return go(s0), nodes
    except RuntimeError: return None, nodes

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
        a, na = dfs(s, True); b, nb = dfs(s, False)
        out.append((a, na, b, nb))
    return out

if __name__ == "__main__":
    ks = [int(x) for x in sys.argv[1:]] or [38, 36, 34, 32]
    with Pool(14) as pool:
        for k in ks:
            rs = [x for c in pool.map(work, [(d, k) for d in range(DEALS)]) for x in c]
            both = [r for r in rs if r[0] is not None and r[2] is not None]
            cap = sum(1 for r in rs if r[2] is None)
            tt = sum(r[1] for r in both) / len(both); nott = sum(r[3] for r in both) / len(both)
            nott_sorted = sorted(r[3] for r in rs)
            p99 = nott_sorted[min(len(rs) - 1, int(0.99 * len(rs)))]
            print(f"k={k} (residual {52-k}): mean nodes with TT {tt:>8,.0f} | without TT {nott:>9,.0f} ({nott/tt:.1f}x) "
                  f"| no-TT p99 {p99:,} | no-TT hit {BUDGET:,} cap: {cap}/{len(rs)}", flush=True)
