"""Independent check of a claimed decomposition against the whole game, with piles tracked by their bottom card.

Every legal move of every reachable position is tested: source pile and target pile must lie in the same
interval. For small deals the reachable set is also compared with the literal product of the parts' sets.
"""
import json, sys, time, itertools
from regions import M, load, PRIO_OK


def full_tracked(cards, ivs, cap):
    region = [0] * 52
    for idx, (a, b) in enumerate(ivs):
        for x in range(a, b):
            region[x] = idx
    n = len(cards)
    layer = {(bytes(range(n)), bytes(cards))}
    crossing = 0
    tracked = moves = 0
    tops_seen = set()
    ln = n
    while layer:
        tracked += len(layer)
        if tracked > cap:
            return None
        nxt = set()
        for bots, s in layer:
            tops_seen.add(s)
            for i in range(1, ln):
                r = M[s[i]]
                for d in (1, 3):
                    if i >= d and r[s[i - d]]:
                        moves += 1
                        if region[bots[i]] != region[bots[i - d]]:
                            crossing += 1
                        # the target pile keeps its bottom; the mover's top goes on it
                        t = s[:i - d] + s[i:i + 1] + s[i - d + 1:i] + s[i + 1:]
                        b = bots[:i] + bots[i + 1:]
                        nxt.add((b, t))
        layer = nxt
        ln -= 1
    return dict(tracked_states=tracked, tops_states=len(tops_seen), moves=moves, crossings=crossing, tops=tops_seen)


def region_set(cards):
    layer = {bytes(cards)}
    out = set(layer)
    ln = len(cards)
    while layer:
        nxt = set()
        for s in layer:
            for i in range(1, ln):
                r = M[s[i]]
                if r[s[i - 1]]:
                    nxt.add(s[:i - 1] + s[i:])
                if i >= 3 and r[s[i - 3]]:
                    nxt.add(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
        out |= nxt
        layer = nxt
        ln -= 1
    return out


def main():
    res = {r["seed"]: r for r in json.load(open(sys.argv[1]))}
    cap = int(float(sys.argv[2]))
    prodcap = int(float(sys.argv[3]))
    print("idle:", PRIO_OK, flush=True)
    for d in load():
        r = res[d["seed"]]
        if r["status"] != "closed" or r["n_parts"] < 2:
            continue
        t0 = time.time()
        f = full_tracked(d["cards"], [tuple(x) for x in r["intervals"]], cap)
        if f is None:
            print(json.dumps(dict(seed=d["seed"], skipped="over cap")), flush=True)
            continue
        rec = dict(seed=d["seed"], parts=r["n_parts"], tracked_states=f["tracked_states"], tops_states=f["tops_states"],
                   moves=f["moves"], crossing_moves=f["crossings"], product=r["product"], rec=d["positions"])
        if f["tops_states"] <= prodcap:
            sets = [region_set(d["cards"][a:b]) for a, b in r["intervals"]]
            literal = set(b"".join(p) for p in itertools.product(*sets))
            rec["literal_product_equal"] = literal == f["tops"]
        rec["secs"] = round(time.time() - t0, 1)
        print(json.dumps(rec), flush=True)


if __name__ == "__main__":
    main()
