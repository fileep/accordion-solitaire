# Event-driven dead-end checks: how often does a move create a *new* trigger?
#   pair -> 0 : the move removed the last rank two suits shared (only then can the position disconnect;
#               this also covers every orphan). At most 6 such events on any line of play.
#   leaf >= 2 : the move left >= 2 cards with exactly one match (only then can a left-leaf pair exist).
# Rates are per brute-force node, by piles left, using the same residuals as deadend_timing.py.
import random
from collections import Counter, defaultdict
from multiprocessing import Pool
from probe import moves, connected, SUIT, RANK
from deadends import TESTS
from deadend_timing import BUCKETS, bucket

FNS = list(TESTS.values())

def rows_of(s):
    rows = [0, 0, 0, 0]
    for c in s: rows[SUIT[c]] |= 1 << RANK[c]
    return rows

def zero_pairs(rows):
    return {(a, b) for a in range(4) for b in range(a + 1, 4) if rows[a] and rows[b] and rows[a] & rows[b] == 0}

def deg1(s, rows):
    sc = [bin(x).count("1") for x in rows]; rc = Counter(RANK[c] for c in s)
    return sum(1 for c in s if sc[SUIT[c]] + rc[RANK[c]] == 3)

def work(args):
    di, k = args
    rd = random.Random(9000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 104729 + k); stats = defaultdict(Counter)
    for _ in range(6):
        s0 = tuple(deal)
        for _ in range(k):
            ms = [t for t in moves(s0) if connected(t)]
            if not ms: break
            s0 = r.choice(ms)
        dead = set(); nodes = 0
        def go(s, parent_zero, parent_deg1):
            nonlocal nodes
            nodes += 1
            if nodes > 400_000: raise RuntimeError
            st = stats[bucket(len(s))]; st["nodes"] += 1
            if len(s) == 1: return True
            if s in dead: return False
            rows = rows_of(s); z = zero_pairs(rows); d1 = deg1(s, rows)
            new_pair = bool(z - parent_zero); new_leaf = d1 >= 2 and parent_deg1 < 2
            st["event pair->0"] += new_pair; st["event leaf>=2"] += new_leaf
            st["leaf>=2 state"] += d1 >= 2
            if any(f(s) for f in FNS): dead.add(s); return False
            for t in moves(s):
                if go(t, z, d1): return True
            dead.add(s); return False
        try:
            rows = rows_of(s0); go(s0, zero_pairs(rows), deg1(s0, rows))
        except RuntimeError: pass
    return stats

if __name__ == "__main__":
    with Pool(12) as pool:
        res = pool.map(work, [(d, 30) for d in range(30)])
    agg = defaultdict(Counter)
    for st in res:
        for b, c in st.items(): agg[b].update(c)
    print("k=30 (brute force from 22 piles): share of nodes where the move into the node creates a trigger event")
    print(f"{'piles':>7} {'pair->0 event':>14} {'leaf>=2 event':>14} {'leaf>=2 state':>14}")
    for b in BUCKETS:
        c = agg.get(b)
        if not c or not c["nodes"]: continue
        n = c["nodes"]
        print(f"{b[0]:>3}-{b[1]:<3} {c['event pair->0']/n:>14.1%} {c['event leaf>=2']/n:>14.1%} {c['leaf>=2 state']/n:>14.1%}")
