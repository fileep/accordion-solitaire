# Random-prefix + brute-force trade-off probe.
# For each prefix length k: play k random (connectivity-preserving) moves, then run
# DFS on the residual (52-k piles) until first win, refutation, or node budget.
import random, sys, time
from multiprocessing import Pool
from probe import moves, connected, solve

KS = [0, 10, 20, 26, 30, 34, 38, 42]
DEALS, ATTEMPTS, BUDGET = 24, 20, 200000

def run(args):
    di, k = args
    rd = random.Random(1000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 7919 + k)
    out = []
    for a in range(ATTEMPTS):
        s = tuple(deal); dead = False
        for _ in range(k):
            ms = [t for t in moves(s) if connected(t)]
            if not ms: dead = True; break
            s = r.choice(ms)
        if dead: out.append(("dead", k)); continue
        ok, n, _ = solve(list(s), BUDGET, True, lambda ms: r.sample(ms, len(ms)))
        out.append(("win" if ok else "loss" if ok is False else "budget", n + k))
    return di, k, out

if __name__ == "__main__":
    t0 = time.time()
    with Pool(14) as pool:
        res = pool.map(run, [(d, k) for k in KS for d in range(DEALS)])
    print(f"{DEALS} deals x {ATTEMPTS} attempts per k, budget {BUDGET:,} nodes/attempt ({time.time()-t0:.0f}s)")
    print(f"{'k':>3} {'resid':>5} {'P(win)':>7} {'P(refuted)':>10} {'P(budget)':>9} {'mean nodes/attempt':>18} {'nodes per win':>14} {'deals with >=1 win':>18}")
    for k in KS:
        rows = [x for (d, kk, o) in res if kk == k for x in o]
        perdeal = [o for (d, kk, o) in res if kk == k]
        w = sum(1 for t, _ in rows if t == "win")
        l = sum(1 for t, _ in rows if t in ("loss", "dead"))
        b = sum(1 for t, _ in rows if t == "budget")
        nodes = sum(n for _, n in rows)
        dw = sum(1 for o in perdeal if any(t == "win" for t, _ in o))
        npw = f"{nodes / w:,.0f}" if w else "inf"
        print(f"{k:>3} {52-k:>5} {w/len(rows):>7.2f} {l/len(rows):>10.2f} {b/len(rows):>9.2f} {nodes/len(rows):>18,.0f} {npw:>14} {dw:>12}/{DEALS}")
