# When can each dead-end test first fire, and where does it fire in practice?
#   1. hard lower bounds on the number of moves (= covered cards) before a test can be true
#   2. uniform random play from the full deal (no filtering): first move at which each test is true
#   3. inside the brute force: nodes, firing rate and cheap-trigger rate by pile count
import random, sys
from collections import Counter, defaultdict
from multiprocessing import Pool
from probe import moves, connected, SUIT, RANK, MATCH
from deadends import orphan, disconnected, left_leaf_conflict

TESTS = (("orphan", orphan), ("disconnected", disconnected), ("left-leaf pair", left_leaf_conflict))

def lower_bounds():
    # Disconnection = suits split S1|S2 and ranks R1|R2 with every card of S1xR2 and S2xR1 covered.
    cross = [(a * (13 - b) + (4 - a) * b, a, b) for a in range(1, 4) for b in range(1, 13)]
    disc = min(c for c, _, _ in cross)
    disc_no_orphan = min(c for c, a, b in cross if a * b >= 2 and (4 - a) * (13 - b) >= 2)
    return {
        "orphan (12 suit-mates + 3 rank-mates covered)": 15,
        "disconnected (any)": disc,
        "disconnected, both parts >= 2 cards": disc_no_orphan,
        "a card with exactly one match (degree 1)": 14,
        # two left leaves cannot match each other: 15 + 15 matches, 2 shared, each keeps one
        "left-leaf pair": 15 + 15 - 2 - 2,
    }

def playout(seed):
    r = random.Random(seed); deal = list(range(52)); r.shuffle(deal); s = tuple(deal)
    first = {}
    for t in range(1, 52):
        ms = moves(s)
        if not ms:
            first["stuck (no moves)"] = t; break
        s = r.choice(ms)
        for name, f in TESTS:
            if name not in first and f(s): first[name] = t
        if "any test" not in first and any(name in first for name, _ in TESTS): first["any test"] = t
        if len(s) == 1: first["won"] = t; break
    return first

def suit_rows(s):
    rows = [0, 0, 0, 0]
    for c in s: rows[SUIT[c]] |= 1 << RANK[c]
    return rows

def triggers(s):
    # cheap incremental preconditions (what the Rust engine would track per move)
    rows = suit_rows(s); counts = [bin(x).count("1") for x in rows]
    present = [i for i in range(4) if rows[i]]
    t_orphan = any(counts[i] == 1 for i in present)                      # some suit down to its last card
    t_disc = any(rows[a] & rows[b] == 0 for a in present for b in present if a < b)  # two suits share no rank
    rc = Counter(RANK[c] for c in s)
    deg1 = sum(1 for c in s if counts[SUIT[c]] + rc[RANK[c]] == 3)
    t_leaf = deg1 >= 2                                                    # at least two degree-1 cards
    return t_orphan, t_disc, t_leaf

BUCKETS = [(26, 28), (23, 25), (20, 22), (17, 19), (14, 16), (11, 13), (8, 10), (2, 7)]
def bucket(n):
    for lo, hi in BUCKETS:
        if lo <= n <= hi: return (lo, hi)
    return None

def search_profile(args):
    di, k = args
    rd = random.Random(9000 + di); deal = list(range(52)); rd.shuffle(deal)
    r = random.Random(di * 104729 + k)
    stats = defaultdict(Counter)
    for _ in range(6):
        s0 = tuple(deal)
        for _ in range(k):
            ms = [t for t in moves(s0) if connected(t)]
            if not ms: break
            s0 = r.choice(ms)
        dead = set(); nodes = 0
        def go(s):
            nonlocal nodes
            nodes += 1
            if nodes > 400_000: raise RuntimeError
            b = bucket(len(s)); st = stats[b]
            st["nodes"] += 1
            if len(s) == 1: return True
            if s in dead: st["tt"] += 1; return False
            to, td, tl = triggers(s)
            st["trig orphan"] += to; st["trig disc"] += td; st["trig leaf"] += tl
            for name, f in TESTS:
                if f(s):
                    st[name] += 1; dead.add(s); return False
            for t in moves(s):
                if go(t): return True
            dead.add(s); return False
        try: go(s0)
        except RuntimeError: pass
    return k, stats

if __name__ == "__main__":
    print("1. Lower bounds (moves before a test can possibly be true, from a full 52-card deal):")
    for name, v in lower_bounds().items():
        print(f"   {name:<50} {v:>3} moves  (<= {52 - v} piles)")

    N = 4000
    with Pool(12) as pool:
        firsts = pool.map(playout, range(N))
        print(f"\n2. Uniform random play from the full deal, no filtering ({N} playouts):")
        keys = [name for name, _ in TESTS] + ["any test", "stuck (no moves)", "won"]
        print("   share of playouts where it has happened by move t:")
        print("   " + f"{'':<18}" + "".join(f"{'t=' + str(t):>7}" for t in (10, 15, 20, 25, 30, 35, 40, 45, 51)))
        for key in keys:
            row = "".join(f"{sum(1 for f in firsts if f.get(key, 99) <= t) / N:>7.1%}" for t in (10, 15, 20, 25, 30, 35, 40, 45, 51))
            print(f"   {key:<18}{row}")
        for key in ("orphan", "disconnected", "left-leaf pair", "any test"):
            ts = sorted(f[key] for f in firsts if key in f)
            if ts: print(f"   first '{key}': earliest move {ts[0]}, median {ts[len(ts)//2]}")

        print("\n3. Inside the brute force (cheap pruning + TT), by piles left; rates are per node:")
        res = pool.map(search_profile, [(d, k) for k in (26, 30, 34) for d in range(30)])
    for k in (26, 30, 34):
        agg = defaultdict(Counter)
        for kk, st in res:
            if kk != k: continue
            for b, c in st.items(): agg[b].update(c)
        tot = sum(c["nodes"] for c in agg.values())
        print(f"   k={k} (brute force starts at {52-k} piles)")
        print(f"   {'piles':>7} {'nodes':>7} | {'fires: orphan':>13} {'disconn':>8} {'leaf pair':>9} | {'triggers: orphan':>16} {'disconn':>8} {'leaf':>6}")
        for b in BUCKETS:
            c = agg.get(b)
            if not c or not c["nodes"]: continue
            n = c["nodes"]
            print(f"   {b[0]:>3}-{b[1]:<3} {n/tot:>7.1%} | {c['orphan']/n:>13.2%} {c['disconnected']/n:>8.2%} {c['left-leaf pair']/n:>9.2%} |"
                  f" {c['trig orphan']/n:>16.1%} {c['trig disc']/n:>8.1%} {c['trig leaf']/n:>6.1%}")
