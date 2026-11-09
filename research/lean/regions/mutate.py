"""Does the test have power? Break the crossing rule on purpose and see whether the checks catch it.
Then decide the one undecided deal, and print factorisations.
"""
import json, os, time, sys
import regions, prune
from regions import M, load, decompose, prod, NAME, PRIO_OK

HERE = os.path.dirname(os.path.abspath(__file__))
RES = {r["seed"]: r for r in json.load(open(os.path.join(HERE, "res1.json")))}


def targets_A(regs, i, p):
    """Mutation A: a leap cannot pass over a region shorter than the distance."""
    r = i - 1
    return [(r, regs[r].tail[p])] if r >= 0 and regs[r].tail[p] else []


def make_crossings(heads):
    def crossings(regs, i):
        best, wit = i, None
        for j, d_list in heads:
            movers = regs[i].head[j]
            if not movers:
                continue
            for d in d_list:
                p = d - j
                if p < 1:
                    continue
                for q, tg in regions.targets(regs, i, p):
                    hit = next(((m, t) for m in movers for t in tg if M[m][t]), None)
                    if hit and q < best:
                        best, wit = q, (j, d, q, NAME[hit[0]], NAME[hit[1]])
        return best, wit
    return crossings


FULL = ((0, (1, 3)), (1, (3,)), (2, (3,)))
orig_t = regions.targets
MUT = {
    "A_no_leap_over_short_region": (targets_A, make_crossings(FULL)),
    "B_only_first_pile_leaps": (orig_t, make_crossings(((0, (1, 3)),))),
    "C_slides_only": (orig_t, make_crossings(((0, (1,)),))),
    "control_correct": (orig_t, make_crossings(FULL)),
}


def main():
    print("idle:", PRIO_OK, flush=True)
    deals = [d for d in load() if d["positions"] <= 300000]
    for name, (t, c) in MUT.items():
        regions.targets = t
        regions.crossings = c
        prune.crossings = c
        wrong = fake = checked = 0
        for d in deals:
            ivs, regs, status, _, _ = decompose(d["cards"], 400000, 30)
            if status != "closed":
                continue
            checked += 1
            if prod(r.states for r in regs) != d["positions"]:
                wrong += 1
            r0 = RES[d["seed"]]
            if len(regs) != r0["n_parts"]:
                fake += 1
        fal = prune.falsify(20000, 45)
        print(json.dumps(dict(mutation=name, deals_checked=checked, product_mismatches=wrong, wrong_part_counts=fake, win_lines=fal)), flush=True)
    regions.targets = orig_t
    regions.crossings = MUT["control_correct"][1]

    # the one deal left undecided by the 2M limit
    d = [x for x in load() if x["seed"] == 1005616798][0]
    t0 = time.time()
    ivs, regs, status, rounds, work = decompose(d["cards"], 4_500_000, 280)
    out = dict(seed=d["seed"], status=status, intervals=ivs, rounds=rounds, work=work, secs=round(time.time() - t0, 1))
    if regs:
        out.update(parts=[r.states for r in regs], product=prod(r.states for r in regs), recorded=d["positions"], minpiles=sum(r.minlen for r in regs))
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
