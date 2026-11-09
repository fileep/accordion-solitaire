"""Region decomposition of Accordion losses. Python only, single process, idle priority.

State = bytes of top cards, left to right. A move removes one pile, so layers by length.
A partition of the row into intervals is CLOSED if no move crosses a boundary from any product state.
"""
import json, glob, os, sys, time, ctypes

try:
    k = ctypes.windll.kernel32
    k.GetCurrentProcess.restype = ctypes.c_void_p
    k.SetPriorityClass.argtypes = (ctypes.c_void_p, ctypes.c_uint)
    PRIO_OK = bool(k.SetPriorityClass(k.GetCurrentProcess(), 0x40))  # IDLE_PRIORITY_CLASS
except Exception:
    PRIO_OK = False

from pathlib import Path
PROOFS = str(Path(__file__).resolve().parents[3] / "hard-deals/proofs")
RANKS = "A23456789TJQK"
SUITS = "CDHS"


def card(tok):
    return RANKS.index(tok[0]) * 4 + SUITS.index(tok[1])


M = [bytes(1 if a != b and (a // 4 == b // 4 or a % 4 == b % 4) else 0 for b in range(52)) for a in range(52)]
NAME = [RANKS[c // 4] + SUITS[c % 4] for c in range(52)]


def load():
    out = []
    for f in glob.glob(PROOFS + "/*.json"):
        d = json.load(open(f))
        out.append(dict(
            seed=d["deal"]["seed"],
            cards=tuple(card(t) for t in d["deal"]["cards"].split()),
            positions=d["stats"]["proof"]["plain"]["dead_positions"],
            moves=d["stats"]["proof"]["plain"]["positions"],
        ))
    out.sort(key=lambda x: x["positions"])
    return out


class Reach:
    __slots__ = ("cards", "states", "moves", "head", "tail", "lens", "minlen", "peak", "secs")


def succ_bytes(s, ln):
    """All children of s, as a list of bytes."""
    out = []
    for i in range(1, ln):
        c = s[i]
        r = M[c]
        if r[s[i - 1]]:
            out.append(s[:i - 1] + s[i:i + 1] + s[i + 1:])
        if i >= 3 and r[s[i - 3]]:
            out.append(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
    return out


def enum_reach(cards, cap=None, deadline=None):
    """Plain layer enumeration of a row. Returns Reach, or None if over cap / deadline."""
    t0 = time.time()
    n = len(cards)
    layer = {bytes(cards)}
    R = Reach()
    R.cards = cards
    R.states = R.moves = R.peak = 0
    R.head = [set(), set(), set()]
    R.tail = [None, set(), set(), set()]
    R.lens = set()
    R.minlen = n
    ln = n
    while layer:
        R.states += len(layer)
        R.peak = max(R.peak, len(layer))
        R.minlen = ln
        R.lens.add(ln)
        if cap is not None and R.states > cap:
            return None
        if deadline is not None and time.time() > deadline:
            return None
        h0, h1, h2 = R.head
        t1, t2, t3 = R.tail[1], R.tail[2], R.tail[3]
        h0.update(s[0] for s in layer)
        t1.update(s[-1] for s in layer)
        if ln >= 2:
            h1.update(s[1] for s in layer)
            t2.update(s[-2] for s in layer)
        if ln >= 3:
            h2.update(s[2] for s in layer)
            t3.update(s[-3] for s in layer)
        nxt = set()
        add = nxt.add
        mv = 0
        for s in layer:
            for i in range(1, ln):
                r = M[s[i]]
                if r[s[i - 1]]:
                    add(s[:i - 1] + s[i:])
                    mv += 1
                if i >= 3 and r[s[i - 3]]:
                    add(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
                    mv += 1
        R.moves += mv
        layer = nxt
        ln -= 1
    R.secs = time.time() - t0
    return R


def targets(regs, i, p):
    """(region index, cards) that can sit p slots left of region i's left edge, in the product game."""
    out = []
    r = i - 1
    need = [(r, p)]
    seen = set()
    while need:
        r, q = need.pop()
        if r < 0 or (r, q) in seen:
            continue
        seen.add((r, q))
        if regs[r].tail[q]:
            out.append((r, regs[r].tail[q]))
        for L in regs[r].lens:
            if L < q:
                need.append((r - 1, q - L))
    return out


def crossings(regs, i):
    """Leftmost region a pile of region i can land on in the product game; i if none.
    Also returns one witness."""
    best, wit = i, None
    for j, d_list in ((0, (1, 3)), (1, (3,)), (2, (3,))):
        movers = regs[i].head[j]
        if not movers:
            continue
        for d in d_list:
            p = d - j
            if p < 1:
                continue
            for q, tg in targets(regs, i, p):
                hit = next(((m, t) for m in movers for t in tg if M[m][t]), None)
                if hit and q < best:
                    best, wit = q, (j, d, q, NAME[hit[0]], NAME[hit[1]])
    return best, wit


def decompose(cards, cap, tlimit):
    """Fixpoint: start from singletons, merge intervals joined by a possible crossing.
    Returns (intervals, Reach list, status, rounds, work)."""
    t0 = time.time()
    deadline = t0 + tlimit
    ivs = [(i, i + 1) for i in range(len(cards))]
    cache = {}
    rounds = work = 0
    while True:
        rounds += 1
        regs = []
        for a, b in ivs:
            R = cache.get((a, b))
            if R is None:
                R = enum_reach(cards[a:b], cap, deadline)
                if R is None:
                    return ivs, None, "capped", rounds, work
                cache[(a, b)] = R
                work += R.states
            regs.append(R)
        spans = []
        for i in range(1, len(ivs)):
            q, _ = crossings(regs, i)
            if q < i:
                spans.append((q, i))
        if not spans:
            return ivs, regs, "closed", rounds, work
        # union of overlapping region-index spans
        merged = [False] * len(ivs)  # merged[i]: boundary between i-1 and i dissolves
        for q, i in spans:
            for x in range(q + 1, i + 1):
                merged[x] = True
        new, a = [], ivs[0][0]
        for i in range(1, len(ivs)):
            if not merged[i]:
                new.append((a, ivs[i][0]))
                a = ivs[i][0]
        new.append((a, ivs[-1][1]))
        ivs = new


def exact(cards, cap, tlimit):
    return enum_reach(cards, cap, time.time() + tlimit)


def prod(xs):
    p = 1
    for x in xs:
        p *= x
    return p


def main():
    cap = int(float(sys.argv[1])) if len(sys.argv) > 1 else 2_000_000
    tlimit = float(sys.argv[2]) if len(sys.argv) > 2 else 120
    only = int(sys.argv[3]) if len(sys.argv) > 3 else 99
    out_path = sys.argv[4] if len(sys.argv) > 4 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "res1.json")
    print("idle priority set:", PRIO_OK, flush=True)
    deals = load()[:only]
    res = []
    for d in deals:
        t0 = time.time()
        rec = dict(seed=d["seed"], rec_positions=d["positions"], rec_moves=d["moves"])
        ivs, regs, status, rounds, work = decompose(d["cards"], cap, tlimit)
        rec.update(status=status, rounds=rounds, decomp_work=work, intervals=ivs)
        if regs:
            parts = [r.states for r in regs]
            rec.update(parts=parts, n_parts=len(parts), product=prod(parts), sum=sum(parts),
                       part_moves=sum(r.moves for r in regs), minpiles=sum(r.minlen for r in regs))
        ex = None
        if d["positions"] <= cap:
            ex = exact(d["cards"], cap * 2, tlimit * 2)
        if ex:
            rec.update(exact_positions=ex.states, exact_moves=ex.moves, exact_minpiles=ex.minlen, exact_secs=round(ex.secs, 2))
        rec["secs"] = round(time.time() - t0, 2)
        res.append(rec)
        print(json.dumps(rec), flush=True)
        json.dump(res, open(out_path, "w"))


if __name__ == "__main__":
    main()
