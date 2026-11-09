# Independent spot check of lean2.md, written without reading dawg.py.
import json, random, sys, time

from pathlib import Path
PROOFS = str(Path(__file__).resolve().parents[3] / "hard-deals/proofs/seed-%s-loss.json")


def enumerate_words(start, match):
    seen, layer, moves = 0, {start}, 0
    words = []
    while layer:
        seen += len(layer)
        words.extend(layer)
        nxt = set()
        for w in layer:
            for j in range(1, len(w)):
                for d in (1, 3):
                    if j >= d and match[w[j - d]][w[j]]:
                        moves += 1
                        nxt.add(w[:j - d] + w[j:j + 1] + w[j - d + 1:j] + w[j + 1:])
        layer = nxt
    return words, seen, moves


def build(words):
    words = sorted(words)
    table, delta, accept = {}, [], []

    def go(lo, hi, depth):
        acc = len(words[lo]) == depth
        if acc:
            lo += 1
        kids, i = [], lo
        while i < hi:
            c, k = words[i][depth], i + 1
            while k < hi and words[k][depth] == c:
                k += 1
            kids.append((c, go(i, k, depth + 1)))
            i = k
        key = (acc, tuple(kids))
        n = table.get(key)
        if n is None:
            n = table[key] = len(delta)
            delta.append(dict(kids))
            accept.append(acc)
        return n

    return go(0, len(words), 0), delta, accept


def check(root, delta, accept, start, match):
    # deal accepted; no one-card word; every slide and leap stays inside
    q = root
    for c in start:
        q = delta[q].get(c)
        if q is None:
            return False, 0
    if not accept[q]:
        return False, 0
    if any(accept[r] for r in delta[root].values()):
        return False, 0
    memo, lookups = {}, 0

    def inc(x, y):
        if x == y:
            return True
        v = memo.get((x, y))
        if v is None:
            v = (not accept[x] or accept[y]) and all(
                l in delta[y] and inc(n, delta[y][l]) for l, n in delta[x].items())
            memo[(x, y)] = v
        return v

    for q in range(len(delta)):
        for a, r in delta[q].items():
            for b, s in delta[r].items():
                if match[a][b]:
                    lookups += 1
                    t = delta[q].get(b)
                    if t is None or not inc(s, t):
                        return False, lookups
            for c, r2 in delta[r].items():
                for d, r3 in delta[r2].items():
                    for b, s in delta[r3].items():
                        if match[a][b]:
                            lookups += 1
                            t = delta[q].get(b)
                            t = None if t is None else delta[t].get(c)
                            t = None if t is None else delta[t].get(d)
                            if t is None or not inc(s, t):
                                return False, lookups
    return True, lookups


def run(seed, claimed):
    t0 = time.time()
    doc = json.load(open(PROOFS % seed))
    cards = doc["deal"]["cards"].split()
    plain = doc["stats"]["proof"]["plain"]
    match = [[x[0] == y[0] or x[1] == y[1] for y in cards] for x in cards]
    start = bytes(range(52))
    words, seen, moves = enumerate_words(start, match)
    root, delta, accept = build(words)
    ok, lookups = check(root, delta, accept, start, match)
    rng, caught = random.Random(seed), 0
    others = [w for w in words if w != start]
    for _ in range(5):
        cut = rng.choice(others)
        m = build([w for w in words if w != cut])
        if not check(m[0], m[1], m[2], start, match)[0]:
            caught += 1
    print(f"seed {seed}: words {seen} (record {plain['dead_positions']}), moves {moves} (record {plain['positions']}), "
          f"nodes {len(delta)} (claimed {claimed}), shrink {seen / len(delta):.0f}x, check {ok}, lookups {lookups}, "
          f"one-word deletions caught {caught}/5, {time.time() - t0:.1f} s", flush=True)


if __name__ == "__main__":
    sys.setrecursionlimit(10000)
    for seed, claimed in ((12929752010, 2689), (18238151902, 2413), (5895817790, 1734)):
        run(seed, claimed)
