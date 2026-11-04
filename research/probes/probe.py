# Quick feasibility probe for open Accordion (52 cards dealt face up, moves L1/L3).
# Card = suit*13 + rank. State = tuple of top cards, left to right.
import random, sys, time
sys.setrecursionlimit(10000)

SUIT = [c // 13 for c in range(52)]
RANK = [c % 13 for c in range(52)]
MATCH = [[SUIT[a] == SUIT[b] or RANK[a] == RANK[b] for b in range(52)] for a in range(52)]

def moves(s):
    n = len(s)
    out = []
    for i in range(1, n):
        c = s[i]
        if MATCH[c][s[i - 1]]:
            out.append(s[:i - 1] + (c,) + s[i + 1:])
        if i >= 3 and MATCH[c][s[i - 3]]:
            out.append(s[:i - 3] + (c,) + s[i - 2:i] + s[i + 1:])
    return out

def connected(s):
    # match graph over top cards connected <=> suit/rank bipartite graph connected
    suits = {SUIT[s[0]]}; ranks = {RANK[s[0]]}
    changed = True
    while changed:
        changed = False
        for c in s:
            su, ra = SUIT[c], RANK[c]
            if (su in suits) != (ra in ranks):
                suits.add(su); ranks.add(ra); changed = True
    return all(SUIT[c] in suits for c in s)

class Budget(Exception): pass

def solve(deal, budget, prune=True, order=None):
    dead = set()
    nodes = 0
    def dfs(s):
        nonlocal nodes
        nodes += 1
        if nodes > budget: raise Budget
        if len(s) == 1: return True
        if s in dead: return False
        if prune and not connected(s):
            dead.add(s); return False
        ms = moves(s)
        if order: ms = order(ms)
        for t in ms:
            if dfs(t): return True
        dead.add(s)
        return False
    try:
        return dfs(tuple(deal)), nodes, len(dead)
    except Budget:
        return None, nodes, len(dead)

def count_reachable(s):
    # exhaustive distinct-state count of everything reachable from s
    seen = {s}; frontier = [s]; per_len = {}
    while frontier:
        nxt = []
        for x in frontier:
            for y in moves(x):
                if y not in seen:
                    seen.add(y); nxt.append(y)
        frontier = nxt
    return len(seen)

if __name__ == "__main__":
    mode = sys.argv[1]
    rng = random.Random(12345)
    if mode == "solve":
        n_deals, budget = int(sys.argv[2]), int(sys.argv[3])
        prune = sys.argv[4] == "1"
        res = []
        t0 = time.time()
        for d in range(n_deals):
            deck = list(range(52)); rng.shuffle(deck)
            ok, nodes, dead = solve(deck, budget, prune)
            res.append((ok, nodes, dead))
        el = time.time() - t0
        wins = sum(1 for r in res if r[0]); unk = sum(1 for r in res if r[0] is None)
        ns = sorted(r[1] for r in res if r[0])
        tot = sum(r[1] for r in res)
        print(f"deals={n_deals} prune={prune} wins={wins} losses={n_deals-wins-unk} budget_out={unk} time={el:.1f}s nodes/s={tot/el:,.0f}")
        if ns:
            q = lambda p: ns[min(len(ns)-1, int(p*len(ns)))]
            print(f"nodes-to-win: min={ns[0]} median={q(.5)} p90={q(.9)} p99={q(.99)} max={ns[-1]}")
    elif mode == "reach":
        for k in [int(x) for x in sys.argv[2:]]:
            counts = []
            t0 = time.time()
            for _ in range(5):
                deck = list(range(52)); rng.shuffle(deck)
                counts.append(count_reachable(tuple(deck[:k])))
            print(f"k={k}: reachable states from random {k}-card rows: {sorted(counts)}  ({time.time()-t0:.1f}s)")
    elif mode == "width":
        # number of distinct states at each depth from a full 52-card deal (BFS)
        deck = list(range(52)); rng.shuffle(deck)
        frontier = {tuple(deck)}
        for depth in range(1, int(sys.argv[2]) + 1):
            nxt = set()
            for x in frontier:
                nxt.update(moves(x))
            frontier = nxt
            print(f"depth {depth}: {len(frontier):,} distinct states", flush=True)
