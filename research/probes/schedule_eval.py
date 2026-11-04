# Analytic expected nodes-to-first-win for restart schedules, from schedule_data.json
# (per deal, per prefix length k: list of [won, nodes] attempt records).
import json, statistics, sys
data = [{int(k): v for k, v in d.items()} for d in json.load(open(sys.argv[1] if len(sys.argv) > 1 else "schedule_data.json"))]
KS = sorted(data[0])

def rung_stats(att):
    p = sum(w for w, _ in att) / len(att)
    c = statistics.mean(n for _, n in att)          # mean cost of one attempt
    return p, c

def expected(d, sched):
    # sched: [(k, m)], last rung repeats forever
    tot, reach = 0.0, 1.0
    for j, (k, m) in enumerate(sched):
        p, c = rung_stats(data[d][k])
        if j == len(sched) - 1:
            return float("inf") if p == 0 else tot + reach * c / p
        fail = (1 - p) ** m
        tot += reach * (c * (1 - fail) / p if p > 0 else c * m)
        reach *= fail
    return tot

def summarize(name, sched):
    v = [expected(d, sched) for d in range(len(data))]
    fin = [x for x in v if x != float("inf")]
    stuck = len(v) - len(fin)
    mean = statistics.mean(v) if not stuck else float("inf")
    return name, mean, max(v), stuck

rows = [summarize(f"fixed k={k} (resid {52-k})", [(k, 1)]) for k in KS]
for start in (38, 36, 34, 32):
    for m in (1, 2, 4, 8, 16):
        for step in (2, 4):
            ladder = [(k, m) for k in range(start, KS[0] - 1, -step)]
            rows.append(summarize(f"ladder {start}->{ladder[-1][0]} step {step}, {m} tries/rung", ladder))
rows.sort(key=lambda r: (r[3], r[1]))
print(f"{'schedule':<44} {'mean nodes/deal':>15} {'worst deal':>12} {'deals never won':>16}")
for name, mean, worst, stuck in rows[:14]:
    print(f"{name:<44} {mean:>15,.0f} {worst:>12,.0f} {stuck:>16}")
print("...")
for name, mean, worst, stuck in [r for r in rows if r[0].startswith('fixed')]:
    m = f"{mean:,.0f}" if mean != float('inf') else "inf"
    w = f"{worst:,.0f}" if worst != float('inf') else "inf"
    print(f"{name:<44} {m:>15} {w:>12} {stuck:>16}")
