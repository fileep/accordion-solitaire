# Collect per-deal attempt data for several random-prefix lengths k:
# play k random (connectivity-preserving) moves, then randomized first-win DFS on the
# residual with a node budget. Writes schedule_data.json; evaluate schedules with
#   python schedule_eval.py schedule_data.json
import random, time, json
from multiprocessing import Pool
from probe import moves, connected, solve

KS = [24, 26, 28, 30, 32, 34, 36, 38]
DEALS, ATTEMPTS, BUDGET = 40, 30, 150000

def run(args):
    di, k = args
    rd = random.Random(5000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 7919 + k * 31)
    out = []
    for a in range(ATTEMPTS):
        s = tuple(deal); n0 = 0; dead = False
        for _ in range(k):
            ms = [t for t in moves(s) if connected(t)]; n0 += 1
            if not ms: dead = True; break
            s = r.choice(ms)
        if dead: out.append((0, n0)); continue
        ok, n, _ = solve(list(s), BUDGET, True, lambda ms: r.sample(ms, len(ms)))
        out.append((1 if ok else 0, n + n0))
    return di, k, out

if __name__ == "__main__":
    t0 = time.time()
    with Pool(14) as pool:
        res = pool.map(run, [(d, k) for k in KS for d in range(DEALS)])
    data = [dict() for _ in range(DEALS)]
    for d, k, o in res: data[d][k] = o
    json.dump([{str(k): v for k, v in dd.items()} for dd in data], open("schedule_data.json", "w"))
    print(f"collected {DEALS} deals x {len(KS)} k x {ATTEMPTS} attempts in {time.time()-t0:.0f}s")
    print("per-deal P(win) spread by k:")
    for k in KS:
        ps = sorted(sum(ok for ok, _ in data[d][k]) / ATTEMPTS for d in range(DEALS))
        zero = sum(1 for p in ps if p == 0)
        print(f"  k={k} resid={52-k}: min={ps[0]:.2f} median={ps[len(ps)//2]:.2f} max={ps[-1]:.2f} deals with p=0 in {ATTEMPTS} tries: {zero}")
