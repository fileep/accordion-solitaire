# Compare move orderings and randomized restarts on the same deals.
import random, sys, time
from probe import solve, moves
N, BUDGET = 50, 200000
rng = random.Random(12345)
deals = []
for _ in range(N):
    d = list(range(52)); rng.shuffle(d); deals.append(d)

def luby(i):
    k = 1
    while (1 << k) - 1 < i: k += 1
    while True:
        if i == (1 << k) - 1: return 1 << (k - 1)
        if (1 << (k - 1)) <= i < (1 << k) - 1:
            i = i - (1 << (k - 1)) + 1; k = 1
            while (1 << k) - 1 < i: k += 1
        else: k -= 1

def report(name, results, t0):
    wins = [n for ok, n in results if ok]
    unk = sum(1 for ok, _ in results if ok is None)
    wins.sort(); med = wins[len(wins)//2] if wins else None
    tot = sum(n for _, n in results)
    print(f"{name:<28} wins={len(wins)}/{N} budget_out={unk} median_nodes={med} total_nodes={tot:,} ({time.time()-t0:.0f}s)", flush=True)

for name, order in [("left->right (L1,L3)", None), ("right->left", lambda ms: ms[::-1])]:
    t0 = time.time(); res = []
    for d in deals:
        ok, n, _ = solve(d, BUDGET, True, order); res.append((ok, n))
    report(name, res, t0)

# randomized DFS with Luby restarts (unit 500 nodes), same total budget per deal
t0 = time.time(); res = []
for di, d in enumerate(deals):
    r = random.Random(di); used = 0; ok = None; i = 1
    while used < BUDGET:
        b = min(500 * luby(i), BUDGET - used); i += 1
        o, n, _ = solve(d, b, True, lambda ms: r.sample(ms, len(ms)))
        used += n
        if o is not None: ok = o; break
    res.append((ok, used))
report("random order + Luby restarts", res, t0)
