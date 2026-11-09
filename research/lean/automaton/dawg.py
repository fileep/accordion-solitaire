"""Symbolic reachability for Accordion: sets of positions as minimal automata (DAWGs) over the row of top cards.

A position is the word of its top cards, left to right, plus an end letter 52.  A set of positions is a hash-consed
deterministic acyclic automaton.  A move is a local rewrite of the word, so "all children of a set" is a short
memoised recursion over the automaton, and never looks at the positions one by one.

usage: dawg.py [limit_positions] [node_cap] [seed ...]
"""
import json, glob, sys, time, ctypes

try:
    k = ctypes.windll.kernel32
    k.GetCurrentProcess.restype = ctypes.c_void_p
    k.SetPriorityClass.argtypes = (ctypes.c_void_p, ctypes.c_uint)
    PRIO = bool(k.SetPriorityClass(k.GetCurrentProcess(), 0x40))
except Exception:
    PRIO = False

PROOFS = r"C:/Users/Filip/Documents/Accordion Solitaire/hard-deals/proofs"
RANKS = "A23456789TJQK"
SUITS = "CDHS"
END = 52
M = [bytes(1 if a != b and a < 52 and b < 52 and (a // 4 == b // 4 or a % 4 == b % 4) else 0 for b in range(53)) for a in range(53)]


def card(t):
    return RANKS.index(t[0]) * 4 + SUITS.index(t[1])


def load():
    out = []
    for f in glob.glob(PROOFS + "/*.json"):
        d = json.load(open(f))
        out.append(dict(seed=d["deal"]["seed"], cards=[card(t) for t in d["deal"]["cards"].split()],
                        positions=d["stats"]["proof"]["plain"]["dead_positions"],
                        moves=d["stats"]["proof"]["plain"]["positions"],
                        best=None))
    out.sort(key=lambda x: x["positions"])
    return out


class Too(Exception):
    pass


class DD:
    def __init__(self, cap):
        self.T = [None, ()]          # id 0 = empty set, id 1 = "end of word reached"
        self.tab = {(): 1}
        self.cap = cap
        self.um = {}
        self.sm = {}
        self.lm = {}
        self.cm = {0: 0, 1: 1}
        self.mm = {0: 0, 1: 0}
        self.ops = dict(union=0, succ=0, leap_paths=0)

    def intern(self, t):
        if not t:
            return 0
        r = self.tab.get(t)
        if r is None:
            r = len(self.T)
            self.T.append(t)
            self.tab[t] = r
            if r > self.cap:
                raise Too()
        return r

    def mk(self, d):
        out = []
        for key in sorted(d):
            out.append(key)
            out.append(d[key])
        return self.intern(tuple(out))

    def union(self, a, b):
        if a == b or b == 0:
            return a
        if a == 0:
            return b
        if a > b:
            a, b = b, a
        key = (a << 32) | b
        r = self.um.get(key)
        if r is not None:
            return r
        self.ops["union"] += 1
        ta, tb = self.T[a], self.T[b]
        i = j = 0
        la, lb = len(ta), len(tb)
        out = []
        ap = out.append
        U = self.union
        while i < la and j < lb:
            ca, cb = ta[i], tb[j]
            if ca < cb:
                ap(ca); ap(ta[i + 1]); i += 2
            elif ca > cb:
                ap(cb); ap(tb[j + 1]); j += 2
            else:
                ap(ca); ap(U(ta[i + 1], tb[j + 1])); i += 2; j += 2
        out.extend(ta[i:])
        out.extend(tb[j:])
        r = self.intern(tuple(out))
        self.um[key] = r
        return r

    def leapmap(self, c):
        r = self.lm.get(c)
        if r is not None:
            return r
        acc = {}
        T = self.T
        tc = T[c]
        for i in range(0, len(tc), 2):
            x, c1 = tc[i], tc[i + 1]
            t1 = T[c1]
            for j in range(0, len(t1), 2):
                y, c2 = t1[j], t1[j + 1]
                t2 = T[c2]
                for h in range(0, len(t2), 2):
                    b = t2[h]
                    if b == END:
                        continue
                    self.ops["leap_paths"] += 1
                    acc.setdefault(b, {}).setdefault(x, {})[y] = t2[h + 1]
        r = {}
        for b, xs in acc.items():
            r[b] = self.mk({x: self.mk(ys) for x, ys in xs.items()})
        self.lm[c] = r
        return r

    def succ(self, n):
        """Every word got from a word of n by exactly one legal move."""
        if n <= 1:
            return 0
        r = self.sm.get(n)
        if r is not None:
            return r
        self.ops["succ"] += 1
        acc = {}
        U = self.union
        T = self.T
        t = T[n]
        for i in range(0, len(t), 2):
            a, c = t[i], t[i + 1]
            if a == END:
                continue
            s = self.succ(c)
            if s:
                acc[a] = U(acc.get(a, 0), s)
            ma = M[a]
            tc = T[c]
            for j in range(0, len(tc), 2):
                b = tc[j]
                if ma[b]:
                    acc[b] = U(acc.get(b, 0), tc[j + 1])
            for b, l in self.leapmap(c).items():
                if ma[b]:
                    acc[b] = U(acc.get(b, 0), l)
        r = self.mk(acc)
        self.sm[n] = r
        return r

    def cnt(self, n):
        r = self.cm.get(n)
        if r is not None:
            return r
        t = self.T[n]
        r = sum(self.cnt(t[i + 1]) for i in range(0, len(t), 2))
        self.cm[n] = r
        return r

    def nmoves(self, n):
        r = self.mm.get(n)
        if r is not None:
            return r
        T, cnt = self.T, self.cnt
        t = T[n]
        r = 0
        for i in range(0, len(t), 2):
            a, c = t[i], t[i + 1]
            if a == END:
                continue
            r += self.nmoves(c)
            ma = M[a]
            tc = T[c]
            for j in range(0, len(tc), 2):
                b = tc[j]
                if ma[b]:
                    r += cnt(tc[j + 1])
            for b, l in self.leapmap(c).items():
                if ma[b]:
                    r += cnt(l)
        self.mm[n] = r
        return r

    def size(self, n):
        seen = set()
        edges = 0
        stack = [n]
        T = self.T
        while stack:
            x = stack.pop()
            if x in seen or x <= 1:
                continue
            seen.add(x)
            t = T[x]
            edges += len(t) // 2
            stack.extend(t[1::2])
        return len(seen), edges

    def compact(self, keep):
        """Rebuild the node table with only what the given roots reach; returns the new ids of the roots."""
        T = self.T
        mp = {0: 0, 1: 1}
        nT = [None, ()]
        ntab = {(): 1}
        for root in keep:
            st = [root]
            while st:
                x = st[-1]
                if x in mp:
                    st.pop(); continue
                t = T[x]
                pend = [c for c in t[1::2] if c not in mp]
                if pend:
                    st.extend(pend); continue
                nt = tuple(mp[v] if i & 1 else v for i, v in enumerate(t))
                mp[x] = len(nT)
                nT.append(nt)
                ntab[nt] = mp[x]
                st.pop()
        self.T = nT
        self.tab = ntab
        self.cm = {0: 0, 1: 1}
        self.mm = {0: 0, 1: 0}
        return [mp[r] for r in keep]

    def clear(self):
        self.um.clear()
        self.sm.clear()
        self.lm.clear()
        self.mm.clear()


def chain(dd, word):
    n = 1
    for c in reversed(word):
        n = dd.intern((c, n))
    return n


def child(T, q, a):
    t = T[q]
    lo, hi = 0, len(t) // 2
    while lo < hi:
        mid = (lo + hi) // 2
        v = t[2 * mid]
        if v < a:
            lo = mid + 1
        elif v > a:
            hi = mid
        else:
            return t[2 * mid + 1]
    return 0


def verify(dd, q0, op_cap):
    """Check, by local rules only, that the automaton q0 is closed under moves (the proof a kernel would run).
    R(p,q): every word got from a word of p by one move is in q.  q = 0 means p has no move.
    I(r,s): every word of r is in s.
    Returns the counts of pair checks and lookups."""
    T = dd.T
    R, I = set(), set()
    Rs = [(q0, q0)]
    Is = []
    leaf = look = 0
    while Rs or Is:
        if look > op_cap:
            return dict(ok=None, R=len(R), I=len(I), leaf=leaf, look=look)
        if Is:
            r, s = Is.pop()
            if (r, s) in I:
                continue
            I.add((r, s))
            t = T[r]
            for i in range(0, len(t), 2):
                look += 1
                sa = child(T, s, t[i])
                if not sa:
                    return dict(ok=False, why="incl")
                Is.append((t[i + 1], sa))
            continue
        p, q = Rs.pop()
        if (p, q) in R:
            continue
        R.add((p, q))
        t = T[p]
        for i in range(0, len(t), 2):
            a, c = t[i], t[i + 1]
            if a == END:
                continue
            ma = M[a]
            look += 1
            qa = child(T, q, a) if q else 0
            Rs.append((c, qa))
            tc = T[c]
            for j in range(0, len(tc), 2):
                b, r1 = tc[j], tc[j + 1]
                if b == END:
                    continue
                if ma[b]:
                    leaf += 1
                    look += 1
                    qb = child(T, q, b) if q else 0
                    if not qb:
                        return dict(ok=False, why="slide")
                    Is.append((r1, qb))
            for j in range(0, len(tc), 2):
                x, c1 = tc[j], tc[j + 1]
                t1 = T[c1]
                for h in range(0, len(t1), 2):
                    y, c2 = t1[h], t1[h + 1]
                    t2 = T[c2]
                    for g in range(0, len(t2), 2):
                        b, r2 = t2[g], t2[g + 1]
                        if b == END or not ma[b]:
                            continue
                        leaf += 1
                        look += 3
                        s1 = child(T, q, b) if q else 0
                        s2 = child(T, s1, x) if s1 else 0
                        s3 = child(T, s2, y) if s2 else 0
                        if not s3:
                            return dict(ok=False, why="leap")
                        Is.append((r2, s3))
    return dict(ok=True, R=len(R), I=len(I), leaf=leaf, look=look)


def run(d, cap, verify_cap, layers_out=None):
    t0 = time.time()
    dd = DD(cap)
    root = chain(dd, d["cards"] + [END])
    layers = []
    S = 0
    n = 52
    tot_c = tot_m = 0
    peak_nodes = 0
    while root:
        c = dd.cnt(root)
        m = dd.nmoves(root)
        nn, ee = dd.size(root)
        layers.append((n, c, m, nn, ee))
        S = dd.union(S, root)
        tot_c += c
        tot_m += m
        peak_nodes = max(peak_nodes, nn)
        root = dd.succ(root)
        dd.clear()
        n -= 1
        if len(dd.T) > cap // 2:
            root, S = dd.compact([root, S])
    t1 = time.time()
    Sn, Se = dd.size(S)
    win_words = [a for a, c in zip(dd.T[S][0::2], dd.T[S][1::2]) if child(dd.T, c, END)]
    v = verify(dd, S, verify_cap)
    t2 = time.time()
    return dict(seed=d["seed"], rec_pos=d["positions"], rec_mov=d["moves"], pos=tot_c, mov=tot_m,
                match=(tot_c == d["positions"] and tot_m == d["moves"]),
                minpiles=n + 1, layer_nodes=sum(l[3] for l in layers), layer_edges=sum(l[4] for l in layers),
                peak_layer_nodes=peak_nodes, created=len(dd.T), S_nodes=Sn, S_edges=Se,
                wins=len(win_words), verify=v, ops=dd.ops, secs_reach=round(t1 - t0, 1), secs_verify=round(t2 - t1, 1),
                layers=layers)


def main():
    limit = int(float(sys.argv[1])) if len(sys.argv) > 1 else 1000000
    cap = int(float(sys.argv[2])) if len(sys.argv) > 2 else 6000000
    seeds = set(int(x) for x in sys.argv[3:])
    print("idle", PRIO, flush=True)
    res = []
    for d in load():
        if seeds and d["seed"] not in seeds:
            continue
        if not seeds and d["positions"] > limit:
            continue
        try:
            r = run(d, cap, 2e8)
        except Too:
            print(json.dumps(dict(seed=d["seed"], status="node cap", positions=d["positions"])), flush=True)
            continue
        v = r["verify"]
        print("%12d pos=%9d mov=%10d match=%s minp=%2d  layernodes=%8d edges=%9d | S nodes=%8d edges=%9d wins=%d | R=%s I=%s look=%s ok=%s | %.1fs+%.1fs" % (
            r["seed"], r["pos"], r["mov"], r["match"], r["minpiles"], r["layer_nodes"], r["layer_edges"], r["S_nodes"], r["S_edges"],
            r["wins"], v.get("R"), v.get("I"), v.get("look"), v.get("ok"), r["secs_reach"], r["secs_verify"]), flush=True)
        res.append(r)
        json.dump(res, open(sys.argv[0].replace("dawg.py", "dawg_res_%s.json" % (seeds and "sel" or "all")), "w"))


if __name__ == "__main__":
    main()
