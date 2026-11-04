# Where to run dead-end tests during the random prefix?
#   filter: test every candidate child at every prefix step and only pick live ones (what earlier probes did)
#   end:    uniform random prefix with no tests; run the tests once at the end, abandon the attempt if dead
# Both then run the same brute force (all three tests at every node + dead-state cache).
import random, sys, json, statistics
from multiprocessing import Pool
from probe import moves
from deadends import TESTS

FNS = list(TESTS.values())
def is_dead(s): return any(f(s) for f in FNS)

KS = [26, 28, 30, 32, 34, 36, 38]
DEALS, ATTEMPTS, BUDGET = 40, 30, 150_000

def dfs(s0):
    dead = set(); nodes = 0
    def go(s):
        nonlocal nodes
        nodes += 1
        if nodes > BUDGET: raise RuntimeError
        if len(s) == 1: return True
        if s in dead: return False
        if is_dead(s): dead.add(s); return False
        for t in moves(s):
            if go(t): return True
        dead.add(s); return False
    try: return go(s0), nodes
    except RuntimeError: return None, nodes

def run(args):
    di, k, mode = args
    rd = random.Random(5000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 7919 + k * 31 + (1 if mode == "end" else 0))
    out = []
    for _ in range(ATTEMPTS):
        s = tuple(deal); steps = 0; evals = 0; failed = None
        for _ in range(k):
            ms = moves(s); steps += 1
            if mode == "filter":
                evals += len(ms)
                ms = [t for t in ms if not is_dead(t)]
            if not ms: failed = "stuck"; break
            s = r.choice(ms)
        if failed is None and mode == "end":
            evals += 1
            if is_dead(s): failed = "dead at end"
        if failed:
            out.append([0, steps, evals, failed]); continue
        ok, n = dfs(s)
        out.append([1 if ok else 0, steps + n, evals, "win" if ok else "refuted" if ok is False else "budget"])
    return di, k, mode, out

def expected(att_by_k, sched):
    tot, reach = 0.0, 1.0
    for j, (k, m) in enumerate(sched):
        att = att_by_k[k]
        p = sum(a[0] for a in att) / len(att); c = statistics.mean(a[1] for a in att)
        if j == len(sched) - 1:
            return float("inf") if p == 0 else tot + reach * c / p
        fail = (1 - p) ** m
        tot += reach * (c * (1 - fail) / p if p > 0 else c * m); reach *= fail
    return tot

if __name__ == "__main__":
    with Pool(10) as pool:
        res = pool.map(run, [(d, k, m) for m in ("filter", "end") for k in KS for d in range(DEALS)])
    data = {m: [dict() for _ in range(DEALS)] for m in ("filter", "end")}
    for d, k, m, o in res: data[m][d][k] = o
    json.dump(data, open("prefix_check_data.json", "w"))
    print(f"{DEALS} deals x {ATTEMPTS} attempts per k; DFS identical in both modes")
    print(f"{'k':>3} {'mode':<7} {'P(win)':>7} {'dead at end':>11} {'stuck':>6} {'nodes/attempt':>13} {'prefix tests/attempt':>20}")
    for k in KS:
        for m in ("filter", "end"):
            rows = [a for d in range(DEALS) for a in data[m][d][k]]
            n = len(rows)
            print(f"{k:>3} {m:<7} {sum(a[0] for a in rows)/n:>7.2f} {sum(a[3]=='dead at end' for a in rows)/n:>11.1%} "
                  f"{sum(a[3]=='stuck' for a in rows)/n:>6.1%} {statistics.mean(a[1] for a in rows):>13,.0f} {statistics.mean(a[2] for a in rows):>20,.1f}")
    print("\nexpected nodes to first win per deal (mean / worst), escalation ladders k = start..26:")
    for start, m_tries, step in ((38, 16, 2), (36, 16, 4), (36, 8, 4), (38, 8, 2)):
        sched = [(k, m_tries) for k in range(start, 25, -step)]
        line = f"  ladder {start}->26 step {step}, {m_tries} tries/rung:"
        for m in ("filter", "end"):
            v = [expected(data[m][d], sched) for d in range(DEALS)]
            line += f"   {m}: {statistics.mean(v):>8,.0f} / {max(v):>9,.0f}"
        print(line)
