"""Window barrier proofs, and a direct no-crossing check. Uses regions.py.

Window barrier at cut k: enumerate only W_L = cards[k-wL:k] and W_R = cards[k:k+wR], each in a RELAXED game
that over-approximates everything outside. If no crossing move is possible between any W_L state and any
W_R state, then no pile ever crosses k in the real game, so the row never gets below 2 piles: lost.
"""
import json, sys, time, os
from regions import M, load, NAME, PRIO_OK


def enum_left(win, outside, cap, deadline):
    """Window just left of the cut. Normal moves; a pile in slot j<3 may also leave to the left (over-approx)."""
    layer = {bytes(win)}
    states = moves = 0
    tail = [None, set(), set(), set()]
    ln = len(win)
    fail = False
    while layer:
        states += len(layer)
        if states > cap or time.time() > deadline:
            return None
        if ln == 0:  # every window pile left: the tail is outside the window, so this window proves nothing
            fail = True
            break
        if outside > 0 and ln < 3:
            fail = True
        tail[1].update(s[-1] for s in layer)
        if ln >= 2:
            tail[2].update(s[-2] for s in layer)
        if ln >= 3:
            tail[3].update(s[-3] for s in layer)
        nxt = set()
        add = nxt.add
        for s in layer:
            for i in range(1, ln):
                r = M[s[i]]
                if r[s[i - 1]]:
                    add(s[:i - 1] + s[i:])
                    moves += 1
                if i >= 3 and r[s[i - 3]]:
                    add(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
                    moves += 1
            if outside > 0:
                for j in range(min(3, ln)):
                    if j == 0 or outside >= 3 - j:
                        add(s[:j] + s[j + 1:])
                        moves += 1
        layer = nxt
        ln -= 1
    return dict(states=states, moves=moves, tail=tail, fail=fail)


def enum_right(win, out_cards, cap, deadline):
    """Window just right of the cut. Normal moves; outside cards may land on the last 3 slots (over-approx)."""
    start = bytes(win)
    seen = {start}
    stack = [start]
    moves = 0
    head = [set(), set(), set()]
    fail = False
    nout = len(out_cards)
    tick = 0
    while stack:
        tick += 1
        if len(seen) > cap or (tick % 256 == 0 and time.time() > deadline):
            return None
        s = stack.pop()
        ln = len(s)
        if nout and ln < 3:
            fail = True
        head[0].add(s[0])
        if ln >= 2:
            head[1].add(s[1])
        if ln >= 3:
            head[2].add(s[2])
        kids = []
        for i in range(1, ln):
            r = M[s[i]]
            if r[s[i - 1]]:
                kids.append(s[:i - 1] + s[i:])
            if i >= 3 and r[s[i - 3]]:
                kids.append(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
        if nout:
            for idx in {ln - 1, ln - 3} | ({ln - 2} if nout >= 2 else set()):
                if idx < 0:
                    continue
                for c in out_cards:
                    if M[c][s[idx]] and c not in s:
                        kids.append(s[:idx] + bytes((c,)) + s[idx + 1:])
        moves += len(kids)
        for t in kids:
            if t not in seen:
                seen.add(t)
                stack.append(t)
    return dict(states=len(seen), moves=moves, head=head, fail=fail)


def clear(L, R):
    """No crossing possible between any left-window state and any right-window state?"""
    if L["fail"] or R["fail"]:
        return False
    h, t = R["head"], L["tail"]
    for j, p in ((0, 1), (0, 3), (1, 2), (2, 1)):
        for m in h[j]:
            for x in t[p]:
                if M[m][x]:
                    return False
    return True


def window_proof(cards, k, wmax, cap, deadline):
    """First (wL, wR) by total width that proves a barrier at k. Returns dict or None."""
    n = len(cards)
    Lc, Rc = {}, {}

    def getL(w):
        w = min(w, k)
        if w not in Lc:
            Lc[w] = enum_left(cards[k - w:k], k - w, cap, deadline)
        return Lc[w], w

    def getR(w):
        w = min(w, n - k)
        if w not in Rc:
            Rc[w] = enum_right(cards[k:k + w], list(cards[k + w:]), cap, deadline)
        return Rc[w], w

    for total in range(6, 2 * wmax + 1):
        for wl in range(3, min(total - 3, wmax) + 1):
            wr = total - wl
            if wr < 3 or wr > wmax:
                continue
            L, a = getL(wl)
            R, b = getR(wr)
            if L is None or R is None:
                continue
            if clear(L, R):
                return dict(k=k, wL=a, wR=b, left_states=L["states"], right_states=R["states"], moves=L["moves"] + R["moves"])
        if time.time() > deadline:
            return None
    return None


def main():
    res_path, out_path = sys.argv[1], sys.argv[2]
    wmax = int(sys.argv[3]) if len(sys.argv) > 3 else 12
    cap = int(float(sys.argv[4])) if len(sys.argv) > 4 else 30000
    tlimit = float(sys.argv[5]) if len(sys.argv) > 5 else 30
    res = {r["seed"]: r for r in json.load(open(res_path))}
    print("idle:", PRIO_OK, flush=True)
    out = []
    for d in load():
        r = res[d["seed"]]
        known = None
        if r["status"] == "closed":
            known = sorted(a for a, b in r["intervals"] if a > 0)
            cuts = known
        else:
            cuts = list(range(3, 50))  # no decomposition: try every cut
        dl = time.time() + tlimit
        found = []
        for k in cuts:
            if time.time() > dl:
                break
            p = window_proof(d["cards"], k, wmax, cap, dl)
            if p:
                found.append(p)
                # keep going to find the smallest, but within the time limit
        best = min(found, key=lambda p: p["left_states"] + p["right_states"]) if found else None
        bad = [p["k"] for p in found if known is not None and p["k"] not in known]
        rec = dict(seed=d["seed"], status=r["status"], n_cuts_tried=len(cuts), n_window_proofs=len(found), best=best, false_barriers=bad)
        out.append(rec)
        print(json.dumps(rec), flush=True)
        json.dump(out, open(out_path, "w"))


if __name__ == "__main__":
    main()
