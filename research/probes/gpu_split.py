# How much of the escalation ladder's work is GPU-friendly?
# Deep rungs (small residuals, tiny DFS, no big cache needed) vs shallow rungs (big DFS + TT).
import json, statistics, sys
data = [{int(k): v for k, v in d.items()} for d in json.load(open(sys.argv[1] if len(sys.argv) > 1 else "schedule_data.json"))]
LADDER = [(k, 16) for k in range(38, 23, -2)]
for cut in (36, 34, 32, 30):
    deep_cost = shallow_cost = solved_deep = 0.0
    for d in data:
        reach = 1.0
        for j, (k, m) in enumerate(LADDER):
            att = d[k]; p = sum(w for w, _ in att) / len(att); c = statistics.mean(n for _, n in att)
            last = j == len(LADDER) - 1
            if last:
                cost = c / p
                fail = 0.0
            else:
                fail = (1 - p) ** m
                cost = c * (1 - fail) / p if p > 0 else c * m
            if k >= cut: deep_cost += reach * cost
            else: shallow_cost += reach * cost
            if k >= cut and k - 2 < cut: solved_deep += 1 - reach * fail
            reach *= fail
    n = len(data); tot = deep_cost + shallow_cost
    print(f"rungs k>={cut} on GPU: {solved_deep/n:5.1%} of deals solved there, "
          f"{deep_cost/tot:5.1%} of all nodes spent there; CPU keeps {shallow_cost/tot:5.1%} of nodes for {1-solved_deep/n:5.1%} of deals")

# If GPU attempts are much cheaper, the schedule should buy more deep tries.
print("\nmore tries on the deep rungs (k = 38..32), then CPU ladder 30..24 with 16 tries:")
for m_deep in (16, 64, 256):
    left = []
    for d in data:
        un = 1.0
        for k in (38, 36, 34, 32):
            p = sum(w for w, _ in d[k]) / len(d[k]); un *= (1 - p) ** m_deep
        left.append(un)
    deep_nodes = 0.0
    for d in data:
        reach = 1.0
        for k in (38, 36, 34, 32):
            att = d[k]; p = sum(w for w, _ in att) / len(att); c = statistics.mean(n for _, n in att)
            fail = (1 - p) ** m_deep
            deep_nodes += reach * (c * (1 - fail) / p if p > 0 else c * m_deep); reach *= fail
    print(f"  {m_deep:>3} tries/deep rung: {1-statistics.mean(left):5.1%} of deals solved on deep rungs, "
          f"mean deep-rung nodes/deal {deep_nodes/len(data):,.0f}")
