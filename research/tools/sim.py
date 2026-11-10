#!/usr/bin/env python3
"""sim.py: the one simulator to use. Wraps `mine` built from ov/learn/sim, which plays what attempts_split plays at the base's settings and holds every policy
of stats/mine and look/mine plus learning within a deal. It runs through machine.py s (E-cores, idle priority).
  python sim.py --deals 20000 base=base x=lk=4,lsh=2 y=rungs... [--threads 8] [--from 800000000000]
  python sim.py --keys          the policy keys and the fields they set
A variant is NAME=SPEC[@own=710,shared=555]: SPEC is `base` or key=value,key=value (mine's parse_pol), @ gives machine instructions a move, if the policy changes them.

Modelled time a deal (reports/ideas.md sections 2 and 5; bit-plane build; prices are [M]/[C], the result is a model, not a measurement):
  P = (shared x I_shared / 32 + own x I_own / 31.8) / 48 + attempts x 1.2;  S = attempts x 3.3 + pos x 0.982 x 0.212;  T = max(P + S, 2P) + 3 x misses/100k
I_shared 555 and I_own 393 for the base. Use the speed column. Absolute numbers run low: deals play one at a time here, so no attempt is wasted on a deal already
won: the base simulates at about 20.7 attempts and 1,073 positions, 455 ns, where the kernel runs 23.4 attempts, 1,110 tried, 497 ns measured.
Misses need many deals: at 20,000 deals one miss is 5 per 100k, and the table shows the count.
"""
import argparse, os, re, statistics, subprocess, sys, time

# The private harness this runs in (machine.py, ov/): not published, so this must be named (README.md beside this file).
S = os.environ.get("ACC_SCRATCH") or sys.exit("sim.py: set ACC_SCRATCH to the folder holding machine.py and ov/ (see README.md beside this file)")
HERE = os.path.dirname(os.path.abspath(__file__))
SOURCE = S + "/ov/learn/sim/src/main.rs"
MINE = HERE + "/mine.exe"
LINE = re.compile(r"^(?P<name>\S+)\s+att\s+(?P<att>[\d.]+) shared\s+(?P<shared>[\d.]+) own\s+(?P<own>[\d.]+) srch\s+(?P<srch>[\d.]+) pos\s+(?P<pos>[\d.]+) miss/100k\s+(?P<miss>[\d.]+)")


def model(r, own, shared):
    p = (r["shared"] * shared / 32 + r["own"] * own / 31.8) / 48 + r["att"] * 1.2
    s = r["att"] * 3.3 + r["pos"] * 0.982 * 0.212
    return max(p + s, 2 * p) + 3 * r["miss"], p, s


def keys():
    text = open(SOURCE, encoding="utf-8").read()
    start = text.index("fn parse_pol")
    end = text.index('what is {k}?', start)
    arms = re.findall(r'"([a-z0-9_]+)" => (?:\{\s*)?([^\n]*)', text[start:end])
    for k, body in arms:
        print(f"{k:<10} {body.strip()[:90]}")
    print("casc       name@from-to joined by +: a cascade of features (F_NAMES in the source)")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("variants", nargs="*")
    ap.add_argument("--deals", type=int, default=20000)
    ap.add_argument("--from", dest="first", type=int, default=800_000_000_000)
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--own", type=float, default=393, help="machine instructions an own move")
    ap.add_argument("--shared", type=float, default=555, help="machine instructions a shared move")
    ap.add_argument("--keys", action="store_true")
    a = ap.parse_args()
    if a.keys:
        return keys()
    if not a.variants:
        ap.error("name at least one variant, e.g. base=base")
    if not os.environ.get("SIM_BG"):
        env = dict(os.environ, SIM_BG="1")
        return subprocess.call([sys.executable, S + "/machine.py", "s", "--", sys.executable, os.path.abspath(__file__), *sys.argv[1:]], env=env)
    specs, prices = [], {}
    for v in a.variants:
        name, _, rest = v.partition("=")
        if not rest:
            ap.error(f"{v}: expected NAME=SPEC")
        spec, _, cost = rest.partition("@")
        c = dict(kv.split("=") for kv in cost.split(",")) if cost else {}
        prices[name] = (float(c.get("own", a.own)), float(c.get("shared", a.shared)))
        specs.append(f"{name}:{spec}")
    t = time.time()
    p = subprocess.run([MINE, "run", str(a.first), str(a.deals), str(a.threads), *specs], capture_output=True, text=True, env=dict(os.environ))
    if p.returncode:
        sys.exit(f"mine failed: {p.stderr[-1500:]}")
    print(f"{a.deals:,} deals from {a.first}, {a.threads} threads, {time.time() - t:.0f} s. SIMULATED; the time is a model.")
    print(f"{'name':<14} {'att':>7} {'shared':>7} {'own':>7} {'srch':>6} {'pos':>7} {'miss/100k':>9} {'misses':>6} {'P ns':>6} {'S ns':>6} {'T ns':>6} {'speed':>6}")
    base = None
    seen = 0
    for line in p.stdout.splitlines():
        m = LINE.match(line)
        if not m:
            continue
        r = {k: float(m[k]) for k in ("att", "shared", "own", "srch", "pos", "miss")}
        own, shared = prices[m["name"]]
        ns, pp, ss = model(r, own, shared)
        base = base or ns
        seen += 1
        print(f"{m['name']:<14} {r['att']:>7.2f} {r['shared']:>7.1f} {r['own']:>7.1f} {r['srch']:>6.2f} {r['pos']:>7.1f} {r['miss']:>9.2f} {r['miss'] * a.deals / 1e5:>6.0f} {pp:>6.0f} {ss:>6.0f} {ns:>6.0f} {base / ns:>6.3f}")
    if seen != len(specs):
        print(f"WARNING: {len(specs)} variants asked, {seen} lines read from mine:\n{p.stdout[-800:]}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
