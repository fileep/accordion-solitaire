# How much do suit/rank symmetries buy for a shared endgame cache?
# A position's future depends only on which positions match which (same suit or same rank);
# the actual suits and ranks are arbitrary labels. Compare cache keys for positions with <= K piles:
#   none     : no shared cache (per-residual dead-state set only; baseline)
#   raw      : exact cards, shared across residuals and deals
#   relabel  : suits and ranks renamed in order of first appearance
#   matrix   : ordered match matrix (upper triangle) - the coarsest exact key
# Cache values: DEAD, or WIN + the winning move as (position, distance), which is
# label-free and valid for every position sharing the key.
import random, sys, time
from collections import Counter
from multiprocessing import Pool
from probe import moves, connected, SUIT, RANK, MATCH
from deadends import TESTS

FNS = list(TESTS.values())
K = 14
DEALS, PER_DEAL, PREFIX_K, BUDGET = 60, 4, 30, 300_000

def key_raw(s): return s
def key_relabel(s):
    sm, rm, out = {}, {}, []
    for c in s:
        out.append(sm.setdefault(SUIT[c], len(sm)) * 16 + rm.setdefault(RANK[c], len(rm)))
    return bytes(out)
def key_matrix(s):
    n = len(s); bits = 0; b = 0
    for i in range(n):
        row = MATCH[s[i]]
        for j in range(i + 1, n):
            if row[s[j]]: bits |= 1 << b
            b += 1
    return (n, bits)
KEYS = {"raw": key_raw, "relabel": key_relabel, "matrix": key_matrix}

def indexed_moves(s):
    n = len(s)
    for i in range(1, n):
        c = s[i]
        if MATCH[c][s[i - 1]]:
            yield i, 1, s[:i - 1] + (c,) + s[i + 1:]
        if i >= 3 and MATCH[c][s[i - 3]]:
            yield i, 3, s[:i - 3] + (c,) + s[i - 2:i] + s[i + 1:]

def apply(s, i, d):
    c = s[i]
    return s[:i - 1] + (c,) + s[i + 1:] if d == 1 else s[:i - 3] + (c,) + s[i - 2:i] + s[i + 1:]

def residuals():
    out = []
    for di in range(DEALS):
        rd = random.Random(7000 + di); deal = list(range(52)); rd.shuffle(deal)
        r = random.Random(di * 31337)
        for a in range(PER_DEAL):
            s = tuple(deal)
            for _ in range(PREFIX_K):
                ms = [t for t in moves(s) if connected(t)]
                if not ms: break
                s = r.choice(ms)
            out.append((di, a, s))
    return out

def run_mode(mode):
    keyf = KEYS.get(mode); cache = {}; st = Counter()
    for di, a, s0 in residuals():
        local = set(); nodes = 0
        def go(s):
            nonlocal nodes
            nodes += 1
            if nodes > BUDGET: raise RuntimeError
            n = len(s)
            if n == 1: return True
            shared = keyf is not None and n <= K
            if shared:
                kk = keyf(s); st["lookups"] += 1
                v = cache.get(kk)
                if v is not None:
                    st["hits"] += 1
                    st["hits from other deals" if v[1] != di else "hits from same deal"] += 1
                    if v[0] == "dead": return False
                    return go(apply(s, *v[2]))   # follow the cached winning move
            elif s in local:
                return False
            if any(f(s) for f in FNS):
                if shared: cache[kk] = ("dead", di, None)
                else: local.add(s)
                return False
            for i, d, t in indexed_moves(s):
                if go(t):
                    if shared: cache[kk] = ("win", di, (i, d))
                    return True
            if shared: cache[kk] = ("dead", di, None)
            else: local.add(s)
            return False
        try: go(s0)
        except RuntimeError: st["budget-outs"] += 1
        st["nodes"] += nodes
    st["entries"] = len(cache)
    return mode, st

if __name__ == "__main__":
    t0 = time.time()
    with Pool(4) as pool:
        res = dict(pool.map(run_mode, ["none", "raw", "relabel", "matrix"]))
    base = res["none"]["nodes"]
    print(f"{DEALS} deals x {PER_DEAL} residuals of {52-PREFIX_K} piles, shared cache for positions with <= {K} piles ({time.time()-t0:.0f}s)")
    print(f"{'key':<9} {'total nodes':>12} {'vs none':>8} {'cache entries':>14} {'hit rate':>9} {'hits from other deals':>22} {'budget-outs':>12}")
    for m in ("none", "raw", "relabel", "matrix"):
        st = res[m]; lk = st["lookups"] or 1
        print(f"{m:<9} {st['nodes']:>12,} {st['nodes']/base:>8.2f} {st['entries']:>14,} {st['hits']/lk:>9.1%} "
              f"{st['hits from other deals']/max(1, st['hits']):>22.1%} {st['budget-outs']:>12}")
