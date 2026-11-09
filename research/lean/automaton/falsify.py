"""Try to break the automaton proof.  Single process, idle priority.

1. Brute force: on the deals up to 200k positions, read every word back out of the automaton, compare with an
   independent explicit enumeration (set equality), and play every legal move on every word, testing membership by
   walking the automaton.  Nothing here uses the local rules that the Lean checker would use.
2. On every near-win (the 19.4M deal included): sample random words uniformly and test all their moves the same way.
3. Mutation: delete one random word from the automaton (not the deal itself); the local checker must reject it.
   Also drop one random edge.  A checker that accepts any of these is broken.
4. Wins must be seen: exhaustive small sub-rows (20-26 cards of random deals) compared with explicit search, including
   whether a one-pile word exists.  And full winnable deals, up to a node cap.
"""
import json, random, sys, time, glob
from dawg import DD, Too, chain, child, verify, load, M, END, card


def build(cards, cap=6000000):
    """Exact automaton of the reachable set."""
    dd = DD(cap)
    root = chain(dd, list(cards) + [END])
    S = 0
    n = len(cards)
    while root:
        S = dd.union(S, root)
        root = dd.succ(root)
        dd.clear()
        n -= 1
    return dd, S, n + 1


def words(T, n, prefix=()):
    t = T[n]
    for i in range(0, len(t), 2):
        a, c = t[i], t[i + 1]
        if a == END:
            yield bytes(prefix)
        else:
            yield from words(T, c, prefix + (a,))


def member(T, q0, w):
    q = q0
    for a in w:
        q = child(T, q, a)
        if not q:
            return False
    return child(T, q, END) != 0


def moves(s):
    ln = len(s)
    for i in range(1, ln):
        r = M[s[i]]
        if r[s[i - 1]]:
            yield s[:i - 1] + s[i:]
        if i >= 3 and r[s[i - 3]]:
            yield s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:]


def explicit(cards):
    layer = {bytes(cards)}
    out = set()
    while layer:
        out |= layer
        nxt = set()
        for s in layer:
            nxt.update(moves(s))
        layer = nxt
    return out


def sample(dd, q0, rng):
    T = dd.T
    q, w = q0, []
    while True:
        t = T[q]
        x = rng.randrange(dd.cnt(q))
        for i in range(0, len(t), 2):
            c = dd.cnt(t[i + 1])
            if x < c:
                break
            x -= c
        a, q = t[i], t[i + 1]
        if a == END:
            return bytes(w)
        w.append(a)


def remove(dd, n, w, i=0):
    """The automaton n without word w (w includes END)."""
    t = dd.T[n]
    out = []
    for j in range(0, len(t), 2):
        a, c = t[j], t[j + 1]
        if a == w[i]:
            if i + 1 < len(w):
                c2 = remove(dd, c, w, i + 1)
                if c2:
                    out += [a, c2]
        else:
            out += [a, c]
    return dd.intern(tuple(out))


def drop_edge(dd, q0, rng):
    """Remove one random edge out of a random node reached by a random word."""
    T = dd.T
    path, q = [], q0
    w = sample(dd, q0, rng) + bytes([END])
    cut = rng.randrange(len(w))
    def go(n, i):
        t = T[n]
        out = []
        for j in range(0, len(t), 2):
            a, c = t[j], t[j + 1]
            if a == w[i]:
                if i < cut:
                    x = go(c, i + 1)
                    if x:
                        out += [a, x]
            else:
                out += [a, c]
        return dd.intern(tuple(out))
    return go(q0, 0)


def main():
    rng = random.Random(20261108)
    deals = load()
    near = []
    # 1 brute force
    for d in deals:
        if d["positions"] > 200000 or len(sys.argv) > 1:
            continue
        dd, S, _ = build(d["cards"])
        ex = explicit(d["cards"])
        got = set(words(dd.T, S))
        bad = sum(1 for s in got for w in moves(s) if not member(dd.T, S, w))
        print("brute", d["seed"], "words", len(got), "explicit", len(ex), "equal", got == ex, "illegal-closure", bad, flush=True)
    # 2 sampling, 3 mutation
    for d in near:
        dd, S, mn = build(d["cards"])
        t0 = time.time()
        badm = nm = 0
        for _ in range(20000):
            s = sample(dd, S, rng)
            for w in moves(s):
                nm += 1
                if not member(dd.T, S, w):
                    badm += 1
        rej = acc = 0
        init = bytes(d["cards"])
        for _ in range(30):
            while True:
                s = sample(dd, S, rng)
                if s != init:
                    break
            m = remove(dd, S, s + bytes([END]))
            v = verify(dd, m, 1e9)
            if v["ok"] is False:
                rej += 1
            else:
                acc += 1
        rej2 = acc2 = same = 0
        for _ in range(30):
            m = drop_edge(dd, S, rng)
            if m == S or m == 0 or not member(dd.T, m, init):
                same += 1     # no change, empty, or the deal itself is gone: not a proof of this deal
                continue
            v = verify(dd, m, 1e9)
            if v["ok"] is False:
                rej2 += 1
            else:
                acc2 += 1
        last = sorted(words(dd.T, S), key=len)[:6] if d["positions"] < 0 else None
        print("near", d["seed"], "samples 20000 moves", nm, "not-in-S", badm, "| delete-word: rejected", rej, "accepted", acc,
              "| drop-edge: rejected", rej2, "accepted", acc2, "deal-word-gone-or-unchanged", same, "minp", mn, "secs", round(time.time() - t0, 1), flush=True)
    # 4 sub-rows
    from dawg import RANKS
    agree = disagree = wins = 0
    for trial in range(150):
        cards = list(range(52))
        rng.shuffle(cards)
        cards = cards[:rng.randrange(12, 20)]
        ex = explicit(cards)
        dd, S, mn = build(cards)
        has = any(len(s) == 1 for s in ex)
        win_syms = [a for a, c in zip(dd.T[S][0::2], dd.T[S][1::2]) if child(dd.T, c, END)]
        ok = set(words(dd.T, S)) == ex and has == bool(win_syms) and verify(dd, S, 1e9)["ok"]
        wins += has
        agree += ok
        disagree += not ok
    print("subrows agree", agree, "disagree", disagree, "with-wins", wins, flush=True)
    # 4b full winnable deals, up to a cap
    for f in sorted(glob.glob(r"C:/Users/Filip/Documents/Accordion Solitaire/hard-deals/gallery/*.json"))[:1]:
        d = json.load(open(f))
        cards = [card(t) for t in d["deal"]["cards"].split()]
        t0 = time.time()
        try:
            dd = DD(int(3e6))
            root = chain(dd, cards + [END])
            n = 52
            peak = 0
            total = 0
            win = False
            while root:
                nn, _ = dd.size(root)
                peak = max(peak, nn)
                total += dd.cnt(root)
                if n == 1:
                    win = True
                root = dd.succ(root)
                dd.clear()
                n -= 1
                if len(dd.T) > 1500000:
                    root = dd.compact([root])[0]
                if time.time() - t0 > 90:
                    print("won-deal", f[-22:], "timeout at piles", n, "peak nodes", peak, flush=True)
                    break
            print("won-deal", f[-22:], "win seen", win, "positions", total, "peak layer nodes", peak, "secs", round(time.time() - t0, 1), flush=True)
        except Too:
            print("won-deal", f[-22:], "node cap at piles", n, "peak", peak, "positions so far", total, "secs", round(time.time() - t0, 1), flush=True)


if __name__ == "__main__":
    main()
