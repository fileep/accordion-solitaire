"""Barrier-pruned proof for deals whose start has no barrier, and a soundness test on won deals.

prove: expand positions layer by layer; at each position, if the region fixpoint gives 2+ closed parts,
the position is lost and is a leaf. Otherwise expand it. Proof size = expanded positions + the distinct
regions used at leaves.

falsify: replay verified winning lines and test every position on them. A correct theorem must never
find 2+ closed parts on a position from which a win exists.
"""
import json, sys, time, glob, re
from regions import M, load, card, crossings, enum_reach, PRIO_OK

from pathlib import Path
GALLERY = str(Path(__file__).resolve().parents[3] / "hard-deals/gallery")


def split(s, cap, cache):
    """Finest closed partition of position s. Returns list of Reach, or None if under 2 parts or capped."""
    n = len(s)
    ivs = [(i, i + 1) for i in range(n)]
    while True:
        regs = []
        for a, b in ivs:
            key = s[a:b]
            R = cache.get(key)
            if R is None:
                R = enum_reach(tuple(key), cap)
                if R is None:
                    return None
                cache[key] = R
            regs.append(R)
        merged = [False] * len(ivs)
        any_merge = False
        for i in range(1, len(ivs)):
            q, _ = crossings(regs, i)
            if q < i:
                any_merge = True
                for x in range(q + 1, i + 1):
                    merged[x] = True
        if not any_merge:
            return regs if len(regs) >= 2 else None
        new, a = [], ivs[0][0]
        for i in range(1, len(ivs)):
            if not merged[i]:
                new.append((a, ivs[i][0]))
                a = ivs[i][0]
        new.append((a, ivs[-1][1]))
        if len(new) < 2:
            return None
        ivs = new


def prove(cards, cap, tlimit):
    t0 = time.time()
    cache = {}
    used = {}
    layer = {bytes(cards)}
    ln = len(cards)
    expanded = leaves = moves = 0
    peak = 0
    status = "done"
    tick = 0
    while layer and status == "done":
        peak = max(peak, len(layer))
        nxt = set()
        for s in layer:
            tick += 1
            if tick % 128 == 0:
                if time.time() - t0 > tlimit:
                    status = "timeout"
                    break
                if len(cache) > 300000:
                    cache.clear()
            regs = split(s, cap, cache)
            if regs is not None:
                leaves += 1
                for R in regs:
                    used[bytes(R.cards)] = (R.states, R.moves)
                continue
            expanded += 1
            for i in range(1, ln):
                r = M[s[i]]
                if r[s[i - 1]]:
                    nxt.add(s[:i - 1] + s[i:])
                    moves += 1
                if i >= 3 and r[s[i - 3]]:
                    nxt.add(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
                    moves += 1
        layer = nxt
        ln -= 1
    rs = sum(x for x, _ in used.values())
    rm = sum(y for _, y in used.values())
    return dict(status=status, reached_len=ln, expanded=expanded, leaves=leaves, moves_expanded=moves, distinct_regions=len(used),
                region_states=rs, region_moves=rm, proof_states=expanded + rs, proof_moves=moves + rm,
                peak_layer=peak, secs=round(time.time() - t0, 1), cache=len(cache))


def completes(s, cap, cache):
    """Did the fixpoint run to the end (single part) rather than give up at the cap?"""
    return enum_reach(tuple(s), cap) is not None


def falsify(cap, tlimit):
    t0 = time.time()
    files = sorted(glob.glob(GALLERY + "/*.json"))
    tested = capped = splits = lines = 0
    bad = []
    for f in files:
        d = json.load(open(f))
        if d.get("result") not in ("win", "won", None) or "moves" not in d:
            continue
        s = bytes(card(t) for t in d["deal"]["cards"].split())
        lines += 1
        cache = {}
        for mv in d["moves"] + [None]:
            if time.time() - t0 > tlimit:
                return dict(lines=lines, tested=tested, capped=capped, splits=splits, bad=bad, stop="time")
            tested += 1
            regs = split(s, cap, cache)
            if regs is not None:
                splits += 1
                bad.append((f.split("seed-")[-1], len(s)))
            elif not completes(s, cap, cache):
                capped += 1  # no claim possible: the fixpoint gave up, so this position is not a real test
            if mv is None:
                break
            a, b = mv.split(">")
            i, j = s.index(card(a)), s.index(card(b))
            assert i - j in (1, 3) and M[s[i]][s[j]], (mv, i, j)
            s = s[:j] + s[i:i + 1] + s[j + 1:i] + s[i + 1:]
        assert len(s) == 1
    return dict(lines=lines, tested=tested, capped=capped, splits=splits, bad=bad, secs=round(time.time() - t0, 1))


def main():
    print("idle:", PRIO_OK, flush=True)
    what = sys.argv[1]
    if what == "falsify":
        print(json.dumps(falsify(int(float(sys.argv[2])), float(sys.argv[3]))), flush=True)
        return
    res = {r["seed"]: r for r in json.load(open(sys.argv[2]))}
    caps = [int(float(x)) for x in sys.argv[3].split(",")]
    tlimit = float(sys.argv[4])
    maxexact = float(sys.argv[5]) if len(sys.argv) > 5 else 1e12
    for d in load():
        r = res[d["seed"]]
        if (r["status"] == "closed" and r["n_parts"] >= 2) or d["positions"] > maxexact:
            continue
        for cap in caps:
            p = prove(d["cards"], cap, tlimit)
            p.update(seed=d["seed"], cap=cap, exact=d["positions"], exact_moves=d["moves"],
                     shrink_states=round(d["positions"] / max(1, p["proof_states"]), 2),
                     shrink_moves=round(d["moves"] / max(1, p["proof_moves"]), 2))
            print(json.dumps(p), flush=True)


if __name__ == "__main__":
    main()
