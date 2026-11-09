"""Near-win losses in detail, by exact enumeration: where lines die, what the last positions are, how much the
candidate abstractions merge, and whether each one is sound.  Single process, idle priority.

usage: anatomy.py <seed> [<seed> ...]
"""
import sys, time, json
from collections import Counter
from dawg import load, M, RANKS, SUITS

NAME = [RANKS[c // 4] + SUITS[c % 4] for c in range(52)]


def children(s):
    ln = len(s)
    out = []
    for i in range(1, ln):
        r = M[s[i]]
        if r[s[i - 1]]:
            out.append(s[:i - 1] + s[i:])
        if i >= 3 and r[s[i - 3]]:
            out.append(s[:i - 3] + s[i:i + 1] + s[i - 2:i] + s[i + 1:])
    return out


def has_orphan(s):
    """Some card shares neither suit nor rank with any other visible card."""
    su, ra = [0] * 4, [0] * 13
    for c in s:
        su[c % 4] += 1
        ra[c // 4] += 1
    return any(su[c % 4] == 1 and ra[c // 4] == 1 for c in s)


def erase(s):
    """Replace a rank (suit) that no other visible card shares by a blank: it can never match again."""
    su, ra = [0] * 4, [0] * 13
    for c in s:
        su[c % 4] += 1
        ra[c // 4] += 1
    return bytes((c % 4 if su[c % 4] > 1 else 4) * 14 + (c // 4 if ra[c // 4] > 1 else 13) for c in s)


def rename(e):
    """Rename suits and ranks by first appearance (blanks stay blank)."""
    sm, rm, out = {4: 4}, {13: 13}, []
    for x in e:
        a, b = x // 14, x % 14
        if a not in sm:
            sm[a] = len(sm) - 1
        if b not in rm:
            rm[b] = len(rm) - 1
        out.append(sm[a] * 14 + rm[b])
    return bytes(out)


def adj(s):
    """Only the pattern of matches between slots."""
    v, n = 0, len(s)
    for i in range(n):
        r = M[s[i]]
        for j in range(i + 1, n):
            v = (v << 1) | r[s[j]]
    return v


def pname(s):
    return " ".join(NAME[c] for c in s)


def study(d):
    t0 = time.time()
    layers = {52: {bytes(d["cards"])}}
    k = 52
    while layers[k]:
        nxt = set()
        for s in layers[k]:
            nxt.update(children(s))
        k -= 1
        layers[k] = nxt
    del layers[k]
    minp = k + 1
    total = sum(len(v) for v in layers.values())
    # v = fewest piles reachable from each position, deepest layer first
    v = {}
    dead = Counter()
    for k in sorted(layers):
        for s in layers[k]:
            ch = children(s)
            if ch:
                v[s] = min(v[c] for c in ch)
            else:
                v[s] = k
                dead[k] += 1
    profile = {k: len(layers[k]) for k in sorted(layers, reverse=True)}
    peak = max(profile, key=profile.get)
    bands = Counter()
    for k, n in profile.items():
        bands["k>=40" if k >= 40 else "30-39" if k >= 30 else "20-29" if k >= 20 else "10-19" if k >= 10 else "<10"] += n
    funnel = Counter(v.values())
    cum = {}
    acc = 0
    for x in sorted(funnel):
        acc += funnel[x]
        cum[x] = acc
    lastset = sorted(layers[minp])[:12]
    last_cards = Counter()
    for s in layers[minp]:
        last_cards.update(s)
    # cards that are on top in every position of a deep layer
    never = {}
    for K in (minp, minp + 1, minp + 2, 5, 8, 12, 16):
        if K in layers and layers[K]:
            base = set(next(iter(layers[K])))
            for s in layers[K]:
                base &= set(s)
            never[K] = [NAME[c] for c in sorted(base)]
    # distinct visible-card sets per deep layer
    sets = {K: (len(layers[K]), len({frozenset(s) for s in layers[K]})) for K in layers if K <= 12}
    # first pile in deep positions
    slot0 = {K: Counter(NAME[s[0]] for s in layers[K]).most_common(5) for K in layers if K in (minp, 6, 10, 14)}
    best2 = sorted(v.values())[0]
    hourglass = {k: (len(layers[k]), sum(1 for s in layers[k] if v[s] == minp), sum(1 for s in layers[k] if v[s] <= minp + 1)) for k in sorted(layers) if k % 3 == 0 or k <= 12}
    if total > 300000:
        out = dict(seed=d["seed"], total=total, minp=minp, peak=peak, peak_n=profile[peak], secs=round(time.time() - t0, 1), bands=dict(bands),
                   dead_total=sum(dead.values()), funnel_cum={k: cum[k] for k in sorted(cum) if k <= 8 or k % 5 == 0}, last_n=len(layers[minp]), last=[pname(s) for s in lastset],
                   last_cards=[(NAME[c], n) for c, n in last_cards.most_common(14)], always=never,
                   deep_sets={str(K): sets[K] for K in sorted(sets)}, slot0={str(K): x for K, x in slot0.items()}, hourglass=hourglass)
        print(json.dumps(out), flush=True)
        return
    # abstractions: classes per layer, and whether all members of a class have the same fewest-piles value and move count
    res = {}
    for name, fn in (("erase", erase), ("erase+rename", lambda s: rename(erase(s)))):
        cl = {}
        bad = 0
        for k in layers:
            for s in layers[k]:
                key = (k, fn(s))
                val = (v[s], sum(1 for _ in children(s)))
                got = cl.setdefault(key, val)
                if got != val:
                    bad += 1
        res[name] = (len(cl), bad)
        del cl
    if total <= 200000:
        cl = {}
        bad = 0
        for k in layers:
            for s in layers[k]:
                key = (k, adj(s))
                val = (v[s], sum(1 for _ in children(s)))
                got = cl.setdefault(key, val)
                if got != val:
                    bad += 1
        res["match pattern"] = (len(cl), bad)
        del cl
    # orphan pruning (a sound lemma): stop expanding a position with an orphan card
    seen = {bytes(d["cards"])}
    layer = set(seen)
    pos = mv = cut = 0
    k = 52
    while layer:
        nxt = set()
        for s in layer:
            pos += 1
            if k >= 2 and has_orphan(s):
                cut += 1
                continue
            ch = children(s)
            mv += len(ch)
            nxt.update(ch)
        layer = nxt
        k -= 1
    orphan = dict(positions=pos, moves=mv, cut=cut)
    out = dict(seed=d["seed"], total=total, minp=minp, peak=peak, peak_n=profile[peak], secs=round(time.time() - t0, 1),
               bands=dict(bands), dead_by_k={k: dead[k] for k in sorted(dead)}, dead_total=sum(dead.values()),
               funnel_cum=cum, last_n=len(layers[minp]), last=[pname(s) for s in lastset],
               last_cards=[(NAME[c], n) for c, n in last_cards.most_common(14)], always=never,
               deep_sets={str(K): sets[K] for K in sorted(sets)}, slot0={str(K): x for K, x in slot0.items()},
               abstractions=res, orphan=orphan, hourglass=hourglass)
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    want = [int(x) for x in sys.argv[1:]]
    by = {d["seed"]: d for d in load()}
    for s in want:
        study(by[s])
