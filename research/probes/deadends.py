# Dead-end tests: soundness check and usefulness on real residual positions.
#
# Residuals come from the Masten-style attempt: k random (connectivity-preserving)
# moves from a random deal. Each residual is labelled winnable/unwinnable by DFS
# without any pruning (so labels don't depend on the tests under study), then:
#   1. soundness: no test may ever fire on a winnable state (checked at the root
#      and on every state along each winning line found),
#   2. root detection: share of unwinnable residuals each test flags immediately,
#   3. in-search value: DFS nodes with each pruning set, and how often each test fires.
import random, sys, time
from collections import Counter
from multiprocessing import Pool
from probe import moves, connected, SUIT, RANK, MATCH

def orphan(s):
    # a top card sharing neither suit nor rank with any other top card
    if len(s) < 2: return False
    sc = Counter(SUIT[c] for c in s); rc = Counter(RANK[c] for c in s)
    return any(sc[SUIT[c]] == 1 and rc[RANK[c]] == 1 for c in s)

def disconnected(s):
    return len(s) >= 2 and not connected(s)

def left_leaf_conflict(s):
    # A "left leaf" is a card with exactly one match, and that match is to its left.
    # It can only ever move by covering that match, which orphans it, so that must be
    # the game's final move: a left leaf has to be the final card. Two of them -> dead.
    if len(s) <= 2: return False
    sc = Counter(SUIT[c] for c in s); rc = Counter(RANK[c] for c in s)
    leaves = 0
    for i, c in enumerate(s):
        if sc[SUIT[c]] + rc[RANK[c]] == 3:  # degree exactly 1
            j = next(j for j, d in enumerate(s) if d != c and MATCH[c][d])
            if j < i:
                leaves += 1
                if leaves >= 2: return True
    return False

TESTS = {"orphan": orphan, "disconnected": disconnected, "left-leaf pair": left_leaf_conflict}
PRUNE_SETS = {
    "none": [],
    "orphan": ["orphan"],
    "orphan+connectivity": ["orphan", "disconnected"],
    "orphan+connectivity+left-leaf": ["orphan", "disconnected", "left-leaf pair"],
}

class Budget(Exception): pass

def dfs_solve(s0, tests, budget, record_path=False):
    dead = set(); nodes = 0; fired = Counter(); path = []
    fns = [(t, TESTS[t]) for t in tests]
    def go(s):
        nonlocal nodes
        nodes += 1
        if nodes > budget: raise Budget
        if len(s) == 1: return True
        if s in dead: return False
        for name, f in fns:
            if f(s):
                fired[name] += 1; dead.add(s); return False
        for t in moves(s):
            if go(t):
                if record_path: path.append(t)
                return True
        dead.add(s); return False
    try:
        ok = go(s0)
    except Budget:
        ok = None
    path.reverse()
    return ok, nodes, fired, [s0] + path

K_VALUES = [26, 30, 34]
DEALS, PER_DEAL, BUDGET = 30, 8, 400_000

def work(args):
    di, k = args
    rd = random.Random(9000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 104729 + k)
    out = []
    for _ in range(PER_DEAL):
        s = tuple(deal); ok_prefix = True
        for _ in range(k):
            ms = [t for t in moves(s) if connected(t)]
            if not ms: ok_prefix = False; break
            s = r.choice(ms)
        if not ok_prefix: continue
        # unbiased label: plain DFS, no pruning
        label, n_plain, _, path = dfs_solve(s, [], BUDGET, record_path=True)
        if label is None: continue
        rec = {"k": k, "n": len(s), "win": label,
               "root": {t: f(s) for t, f in TESTS.items()},
               "unsound": [t for t, f in TESTS.items() for x in (path if label else []) if f(x)]}
        rec["nodes"] = {}; rec["fired"] = {}
        for name, ps in PRUNE_SETS.items():
            ok, n, fired, _ = dfs_solve(s, ps, BUDGET)
            assert ok == label, (name, ok, label)  # pruning must never change the answer
            rec["nodes"][name] = n; rec["fired"][name] = dict(fired)
        out.append(rec)
    return out

if __name__ == "__main__":
    t0 = time.time()
    with Pool(14) as pool:
        recs = [x for chunk in pool.map(work, [(d, k) for k in K_VALUES for d in range(DEALS)]) for x in chunk]
    print(f"{len(recs)} labelled residuals in {time.time()-t0:.0f}s")
    bad = [r for r in recs if r["unsound"]]
    print(f"SOUNDNESS: tests fired on a winnable state in {len(bad)} residuals {sorted({t for r in bad for t in r['unsound']})}")
    for k in K_VALUES:
        rs = [r for r in recs if r["k"] == k]
        lost = [r for r in rs if not r["win"]]; won = [r for r in rs if r["win"]]
        print(f"\nk={k} (residual {52-k} piles): {len(won)} winnable, {len(lost)} unwinnable")
        if lost:
            for t in TESTS:
                print(f"  root flags {t:<15}: {sum(r['root'][t] for r in lost)/len(lost):6.1%} of unwinnable")
            anyf = sum(any(r["root"].values()) for r in lost) / len(lost)
            print(f"  root flags any test      : {anyf:6.1%} of unwinnable")
        for label, group in (("unwinnable (refute)", lost), ("winnable (first win)", won)):
            if not group: continue
            base = sum(r["nodes"]["none"] for r in group)
            parts = ", ".join(f"{name}={sum(r['nodes'][name] for r in group)/base:.2f}x" for name in PRUNE_SETS)
            print(f"  DFS nodes vs no pruning, {label}: {parts}")
        full = "orphan+connectivity+left-leaf"
        fired = Counter()
        for r in lost: fired.update(r["fired"][full])
        tot = sum(r["nodes"][full] for r in lost)
        if tot:
            print(f"  share of refutation nodes cut by each test (full set): " +
                  ", ".join(f"{t}={fired[t]/tot:.1%}" for t in TESTS))
