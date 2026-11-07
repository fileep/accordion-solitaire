#!/usr/bin/env python3
"""ab.py: compare kernels in one lock hold, alternated, with the run conditions logged and suspect runs flagged.

GPU alone (accgpu; timed runs skip verification, --verify-last adds one verified run of each kernel after them):
  python ab.py --passes 3 --count 2000000 base=kernel.cu x=variant.cu y=variant.cu:DEF=1:OTHER=2 --config '{"rungs":[[39,128],[37,1152]]}' --verify-last
Hot acc batch (--hot; a variant is a .cu, compiled by acc --gpu-kernel, or an acc.exe with its built-in kernel):
  python ab.py --hot --passes 2 --count 60000000 base=old/acc.exe x=variant.cu

A variant is NAME=PATH[:DEF=VALUE...][@JSON]; PATH "builtin" is the exe's own kernel, @JSON overrides --config for it. The first is the reference.
Lock: takes it itself by default (machine.py x, or h with --hot), and blocks until it has it. --queue NAME sends the job through the GPU queue
instead, waits up to --wait seconds, and prints the table; if it isn't done, `ab.py --wait-on NAME` goes on waiting. --show FILE reprints a table.
Results go to --out (default ov/tools/results/). Nothing is dropped: every run is in the JSON and flagged runs stay in the medians.
"""
import argparse, csv, ctypes, glob, hashlib, json, os, re, shlex, shutil, statistics, subprocess, sys, tempfile, threading, time

S = os.environ.get("ACC_SCRATCH", "C:/Users/Filip/AppData/Local/Temp/claude/C--Users-Filip-Documents-Accordion-Solitaire/2d8fbaee-89f4-4073-a2f7-1bc93a04a06d/scratchpad")
HERE = os.path.dirname(os.path.abspath(__file__))
TOOLS = os.environ.get("ACC_TOOLS", S + "/ov/tools")
SECONDS_A_LINE = 60e-6  # the reference verification in accgpu, thread time
HW_THROTTLE = 0x08 | 0x20 | 0x40 | 0x80  # hw_slowdown, sw_thermal, hw_thermal, power brake (sw_power_cap, 0x04, is normal at the 30 W cap)


def fail(text):
    sys.exit("ab.py: " + text)


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()[:12]


def parse_variant(text):
    name, sep, rest = text.partition("=")
    if not sep:
        fail(f"{text!r}: expected NAME=PATH[:DEF=VALUE...][@JSON]")
    config = None
    if "@" in rest:
        rest, config = rest.split("@", 1)
        json.loads(config)
    parts = rest.split(":")
    if len(parts) > 1 and len(parts[0]) == 1 and parts[1][:1] in ("/", "\\"):  # a drive letter
        parts[0:2] = [parts[0] + ":" + parts[1]]
    path = parts[0]
    if re.match(r"/[a-zA-Z]/", path):  # Git Bash leaves /c/... alone in an argument with a colon
        path = path[1] + ":" + path[2:]
    if path != "builtin":
        path = os.path.abspath(path)
        if not os.path.exists(path):
            fail(f"variant {name}: no file {path}")
    return {"name": name, "path": path, "defines": parts[1:], "config": config}


class Sampler:
    """GPU and CPU conditions through the hold, each sample stamped on arrival; window() summarises a stretch of time."""

    def __init__(self, ms):
        self.ms, self.gpu, self.cpu, self.load, self.stop_now, self.procs = ms, [], [], [], False, []

    def start(self):
        query = "clocks.gr,power.draw,temperature.gpu,utilization.gpu,clocks_throttle_reasons.active"
        p = subprocess.Popen(["nvidia-smi", f"--query-gpu={query}", "--format=csv,noheader,nounits", "-lms", str(self.ms)],
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.procs.append(p)
        threading.Thread(target=self._read_gpu, args=(p,), daemon=True).start()
        counters = [r"\Processor Information(_Total)\% Processor Performance", r"\Energy Meter(*)\Power"]
        p = subprocess.Popen(["typeperf", *counters, "-si", str(max(1, self.ms // 1000))], stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, text=True)
        self.procs.append(p)
        threading.Thread(target=self._read_cpu, args=(p,), daemon=True).start()
        threading.Thread(target=self._read_load, daemon=True).start()

    def stop(self):
        self.stop_now = True
        for p in self.procs:
            p.terminate()

    def _read_gpu(self, p):
        def num(x):
            try:
                return float(x)
            except ValueError:
                return None
        for line in p.stdout:
            f = [x.strip() for x in line.split(",")]
            if len(f) >= 5:
                try:
                    bits = int(f[4], 16)
                except ValueError:
                    bits = 0
                self.gpu.append((time.time(), num(f[0]), num(f[1]), num(f[2]), num(f[3]), bits))

    def _read_cpu(self, p):
        pkg = None
        for row in csv.reader(p.stdout):
            if len(row) < 2:
                continue
            if row[0].startswith("(PDH"):
                for i, name in enumerate(row):
                    if "Energy Meter" in name and "PKG" in name.upper() and pkg is None:
                        pkg = i
                continue
            try:
                perf = float(row[1])
                watts = float(row[pkg]) / 1000 if pkg is not None else None  # the counter is in milliwatts
            except (ValueError, IndexError):
                continue
            self.cpu.append((time.time(), perf, watts))

    def _read_load(self):
        class FT(ctypes.Structure):
            _fields_ = [("lo", ctypes.c_uint32), ("hi", ctypes.c_uint32)]
        idle, kernel, user = FT(), FT(), FT()
        get = ctypes.windll.kernel32.GetSystemTimes
        val = lambda f: f.hi * 2 ** 32 + f.lo
        while not self.stop_now:
            get(ctypes.byref(idle), ctypes.byref(kernel), ctypes.byref(user))
            self.load.append((time.time(), val(idle), val(kernel), val(user)))
            time.sleep(0.25)

    def window(self, t0, t1):
        g = [s for s in self.gpu if t0 <= s[0] <= t1]
        c = [s for s in self.cpu if t0 <= s[0] <= t1]
        l = [s for s in self.load if t0 <= s[0] <= t1]
        def mean(rows, i):
            v = [r[i] for r in rows if r[i] is not None]
            return statistics.fmean(v) if v else None
        busy = None
        if len(l) >= 2:
            dk, du, di = l[-1][2] - l[0][2], l[-1][3] - l[0][3], l[-1][1] - l[0][1]
            busy = (dk + du - di) / max(1, dk + du) * (os.cpu_count() or 16)
        bits = 0
        for s in g:
            bits |= s[5]
        return {"samples": len(g), "clock_mhz": mean(g, 1), "power_w": mean(g, 2), "temp_c": mean(g, 3), "gpu_busy": mean(g, 4),
                "throttle_bits": bits, "cpu_perf": mean(c, 1), "cpu_pkg_w": mean(c, 2), "cpu_busy_threads": busy}

    def gpu_busy_now(self):
        v = [s[4] for s in self.gpu[-6:] if s[4] is not None]
        return max(v) if v else None


def accgpu_command(a, v, count, verify):
    exe = v["path"] if v["path"].lower().endswith(".exe") else a.exe
    cmd = [exe, "--from", str(a.first), "--count", str(count), "--json", "--endgame-dir", a.endgame]
    config = v["config"] or a.config
    if config:
        cmd += ["--gpu-config", config]
    if v["path"].endswith(".cu"):
        cmd += ["--kernel", v["path"]]
    for d in v["defines"]:
        cmd += ["--define", d]
    return cmd + (["--verify-threads", str(a.verify_threads)] if verify else ["--no-verify"])


def run_alone(a, v, sampler, tag, count, verify=False):
    cmd = accgpu_command(a, v, count, verify)
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, cwd=a.run_dir)
    except OSError as e:
        fail(f"{cmd[0]}: {e}. WinError 4551 is Smart App Control blocking the exe: change a comment and rebuild.")
    try:
        r = json.loads(p.stdout.strip().splitlines()[-1])
    except (IndexError, ValueError):
        return {"kernel": v["name"], "tag": tag, "error": f"exit {p.returncode}: {p.stderr[-600:]}"}
    r.pop("settings", None)
    r.pop("profile", None)
    r["kernel_path"] = r.pop("kernel", None)
    run = {"kernel": v["name"], "tag": tag, "exit": p.returncode, **r, **sampler.window(r["gpu_start_unix"], r["gpu_end_unix"])}
    say_run(run)
    return run


def say_run(r):
    if "error" in r:
        print(f"{r['tag']:>7} {r['kernel']:<12} ERROR {r['error']}", flush=True)
        return
    fmt = lambda x, f="{:.0f}": "-" if x is None else f.format(x)
    print(f"{r['tag']:>7} {r['kernel']:<12} {r['deals_per_s']:>9.0f} deals/s  {r.get('attempts_per_deal', 0):6.2f} att {r.get('positions_per_deal', 0):7.0f} pos  "
          f"{fmt(r.get('clock_mhz'))} MHz {fmt(r.get('power_w'), '{:.1f}')} W {fmt(r.get('temp_c'))} C  cpu {fmt(r.get('cpu_busy_threads'), '{:.1f}')} thr", flush=True)


def order(n_variants, passes):
    """A B C, C B A, A B C: each kernel is as often early as late."""
    out = []
    for p in range(passes):
        out += [(p, i) for i in (range(n_variants) if p % 2 == 0 else reversed(range(n_variants)))]
    return out


def run_hot(a, v, sampler, tag, count):
    d = os.path.join(a.work, tag)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    exe = v["path"] if v["path"].lower().endswith(".exe") else a.acc
    cmd = [exe, "batch", "--from", str(a.first), "--count", str(count), "--gpu", "on", "--endgame-dir", a.endgame,
           "--checkpoint", d + "/ck.json", "--log-dir", d + "/logs", "--dossiers", d + "/dossiers"]
    config = v["config"] or a.config
    cmd += ["--gpu-config", config or "{}"]
    if v["path"].endswith(".cu"):
        cmd += ["--gpu-kernel", v["path"]]
    for x in v["defines"]:
        cmd += ["--gpu-define", x]
    cmd += shlex.split(a.extra or "")
    t0 = time.time()
    with open(d + "/out.txt", "w") as out:
        try:
            rc = subprocess.call(cmd, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, cwd=a.run_dir)
        except OSError as e:
            fail(f"{cmd[0]}: {e}. WinError 4551 is Smart App Control blocking the exe: change a comment and rebuild.")
    text = open(d + "/out.txt", errors="replace").read().replace("\r", "\n")
    run = {"kernel": v["name"], "tag": tag, "exit": rc, "count": count, "wall_s": time.time() - t0}
    logs = glob.glob(d + "/logs/*.csv")
    rows = []
    if logs:
        for line in open(logs[0]):
            f = line.strip().split(",")
            if len(f) >= 8 and f[0][:1].isdigit():
                rows.append([float(x) for x in f[:8]])
    if len(rows) < 8:
        run["error"] = f"exit {rc}, {len(rows)} rows in the speed log: {text[-500:]}"
        print(f"{tag:>7} {v['name']:<12} ERROR {run['error']}", flush=True)
        return run
    half = next(i for i, r in enumerate(rows) if r[2] >= count / 2)
    a0, a1 = rows[1], rows[half]
    z = rows[-1]
    second = (z[2] - a1[2]) / max(1e-9, z[1] - a1[1])
    run["first_half_deals_per_s"] = (a1[2] - a0[2]) / max(1e-9, a1[1] - a0[1])
    run["deals_per_s"] = run["second_half_deals_per_s"] = second
    run["positions_per_deal"] = statistics.fmean(r[6] * 1e6 / max(1.0, r[4]) for r in rows[half:])
    run.update(sampler.window(a1[0], z[0]))
    for line in text.split("\n"):
        w = line.split()
        if line.startswith("failed to verify"):
            run["failed_lines"] = int(w[3])
        elif line.startswith("throughput"):
            run["throughput"] = float(w[1])
        elif line.startswith("won "):
            run["won"] = int(w[1])
        elif line.startswith("GPU ") and " won " in line:
            i = w.index("won")
            run["gpu_won"], run["gpu_given"] = int(w[i + 1]), int(w[i + 4])
            run["misses"] = run["gpu_given"] - run["gpu_won"]
            run["misses_per_100k"] = run["misses"] * 1e5 / max(1, run["gpu_given"])
            for x in range(len(w)):
                if w[x].startswith("attempts"):
                    run["attempts_per_deal"] = float(w[x - 1].rstrip("M")) * 1e6 / count
        elif line.startswith("CPU threads"):
            run["cpu_threads_line"] = line.strip()[:170]
    run["all_won"] = run.get("won") == count
    say_run(run)
    return run


def flag_runs(data):
    """Each run's reasons to doubt it. A flagged run stays in the numbers, so that the reader decides."""
    timed = [r for r in data["runs"] if "error" not in r]
    best = max((r.get("clock_mhz") or 0 for r in timed), default=0)
    pkg = [r["cpu_pkg_w"] for r in timed if r.get("cpu_pkg_w")]
    pkg_mid = statistics.median(pkg) if pkg else None
    med = {}
    for r in timed:
        med.setdefault(r["kernel"], []).append(r["deals_per_s"])
    for r in data["runs"]:
        why = []
        if "error" in r:
            why.append("error")
        else:
            c = r.get("clock_mhz")
            if not c or r.get("samples", 0) < 2:
                why.append("too few GPU samples")
            elif c < data["min_clock"] or c < 0.93 * best:
                why.append(f"low clock {c:.0f}")
            if r.get("throttle_bits", 0) & HW_THROTTLE:
                why.append(f"hw throttle {r['throttle_bits']:#x}")
            if not data["hot"] and (r.get("cpu_busy_threads") or 0) > data["max_cpu"]:
                why.append(f"CPU load {r['cpu_busy_threads']:.1f} threads")
            m = statistics.median(med[r["kernel"]])
            if abs(r["deals_per_s"] / m - 1) > 0.08:
                why.append(f"{100 * (r['deals_per_s'] / m - 1):+.0f}% from its median")
            if data["hot"]:
                if (r.get("gpu_busy") or 100) < 85:
                    why.append(f"GPU busy {r['gpu_busy']:.0f}%")
                if pkg_mid and r.get("cpu_pkg_w") and abs(r["cpu_pkg_w"] / pkg_mid - 1) > 0.2:
                    why.append(f"CPU {r['cpu_pkg_w']:.0f} W, others {pkg_mid:.0f} W: another power state")
                if r["second_half_deals_per_s"] < 0.85 * r["first_half_deals_per_s"]:
                    why.append("fell 15%+ within the run")
                if r.get("failed_lines") or not r.get("all_won"):
                    why.append("FAILED lines or not all won")
        r["suspect"] = why


def summarise(data):
    flag_runs(data)
    names = data["variants"]
    runs = [r for r in data["runs"] if "error" not in r]
    by_pass = {}
    for r in runs:
        by_pass.setdefault(r["pass"], {})[r["kernel"]] = r["deals_per_s"]
    out, warn = [], []
    med = lambda rows, key: statistics.median(v) if (v := [r[key] for r in rows if r.get(key) is not None]) else None
    for name in names:
        rows = [r for r in runs if r["kernel"] == name]
        if not rows:
            out.append({"kernel": name, "runs": 0})
            continue
        speeds = [r["deals_per_s"] for r in rows]
        ratios = [p[name] / p[names[0]] for p in by_pass.values() if name in p and names[0] in p]
        out.append({"kernel": name, "runs": len(rows), "deals_per_s": statistics.median(speeds), "min": min(speeds), "max": max(speeds),
                    "ratio": statistics.median(ratios) if ratios else None, "ratio_min": min(ratios) if ratios else None,
                    "ratio_max": max(ratios) if ratios else None, "attempts_per_deal": med(rows, "attempts_per_deal"),
                    "positions_per_deal": med(rows, "positions_per_deal"), "misses_per_100k": med(rows, "misses_per_100k"),
                    "clock_mhz": med(rows, "clock_mhz"), "power_w": med(rows, "power_w"), "temp_c": med(rows, "temp_c"),
                    "gpu_busy": med(rows, "gpu_busy"), "cpu_perf": med(rows, "cpu_perf"), "cpu_pkg_w": med(rows, "cpu_pkg_w"),
                    "cpu_busy_threads": med(rows, "cpu_busy_threads"), "suspect_runs": sum(1 for r in rows if r["suspect"]),
                    "failed_lines": (sum(r["failed_lines"] for r in rows) if all("failed_lines" in r for r in rows) else "?") if data["hot"] else None})
    if (data.get("gpu_busy_at_start") or 0) > 10:
        warn.append(f"the GPU was {data['gpu_busy_at_start']:.0f}% busy before this hold's first run: something else was on it")
    first = [r for r in runs if r["kernel"] == names[0]]
    if len(first) >= 2 and abs(first[-1]["deals_per_s"] / first[0]["deals_per_s"] - 1) > 0.05:
        warn.append(f"the reference moved {100 * (first[-1]['deals_per_s'] / first[0]['deals_per_s'] - 1):+.1f}% from its first run to its last in this hold")
    for s in out[1:]:
        if s.get("ratio_min") is not None and s["ratio_min"] < 1 < s["ratio_max"]:
            warn.append(f"{s['kernel']}: its pass ratios fall on both sides of 1: no difference shown")
    if len(by_pass) < 2:
        warn.append("one pass only: no alternation, order and heat are not cancelled")
    if any(r["suspect"] for r in data["runs"]):
        warn.append(f"{sum(1 for r in data['runs'] if r['suspect'])} of {len(data['runs'])} runs flagged suspect (kept in the numbers)")
    data["summary"], data["warnings"] = out, warn


def report(data):
    fmt = lambda x, f="{:.0f}": "-" if x is None else f.format(x)
    hot = data["hot"]
    print()
    print(f"{'HOT acc batch, second-half speed' if hot else 'GPU alone'}, {data['count']:,} deals a run, {data['passes']} passes in one hold, seeds from {data['first']}"
          f" ({data['finished'] or 'unfinished'}); hold {data['hold_s']:.0f} s")
    if data.get("warm"):
        print(f"warm-up: {data['warm']}")
    head = f"{'kernel':<12} {'deals/s':>9} {'range':>19} {'ratio':>6} {'ratio range':>12} {'att':>6} {'pos':>6} {'miss/100k':>9} {'MHz':>5} {'W':>5} {'C':>3} "
    head += f"{'GPU%':>4} {'CPU%':>4} {'CPU W':>5} " if hot else f"{'CPUthr':>6} "
    print(head + f"{'lines':<22} suspect")
    verified = {r["kernel"]: r for r in data.get("verify", [])}
    for s in data["summary"]:
        if not s["runs"]:
            print(f"{s['kernel']:<12} no successful run")
            continue
        if hot:
            lines = f"{s['failed_lines']} failed, all verified" if s["failed_lines"] == 0 else f"{s['failed_lines']} FAILED OR UNREAD"
        else:
            v = verified.get(s["kernel"], {}).get("verify")
            if not v:
                lines = "NOT VERIFIED"
            elif v.get("failed_lines") or not v.get("all_won"):
                lines = f"{v.get('failed_lines')} FAILED, all won {v.get('all_won')}"
            else:
                lines = f"0 failed, all won"
        line = (f"{s['kernel']:<12} {s['deals_per_s']:>9.0f} {s['min']:>9.0f}-{s['max']:<9.0f} {fmt(s['ratio'], '{:.3f}'):>6} "
                f"{fmt(s['ratio_min'], '{:.3f}') + '-' + fmt(s['ratio_max'], '{:.3f}'):>12} {fmt(s['attempts_per_deal'], '{:.2f}'):>6} {fmt(s['positions_per_deal']):>6} "
                f"{fmt(s['misses_per_100k'], '{:.2f}'):>9} {fmt(s['clock_mhz']):>5} {fmt(s['power_w'], '{:.1f}'):>5} {fmt(s['temp_c']):>3} ")
        line += f"{fmt(s['gpu_busy']):>4} {fmt(s['cpu_perf']):>4} {fmt(s['cpu_pkg_w'], '{:.1f}'):>5} " if hot else f"{fmt(s['cpu_busy_threads'], '{:.1f}'):>6} "
        print(line + f"{lines:<22} {s['suspect_runs']} of {s['runs']}")
    if not hot:
        if data.get("verify"):
            for r in data["verify"]:
                v = r.get("verify") or {}
                if "error" in r:
                    print(f"verify {r['kernel']}: ERROR {r['error']}")
                else:
                    print(f"verify {r['kernel']}: one extra run, not in the medians: {v.get('gpu_lines_verified')} GPU lines verified on {v.get('threads')} threads in {v.get('seconds', 0):.1f} s, "
                          f"{v.get('failed_lines')} failed, {r.get('misses')} misses, {v.get('cpu_won')} of {v.get('cpu_solved')} misses won on the CPU, all won: {v.get('all_won')}")
        else:
            print("LINES NOT VERIFIED: these runs used --no-verify; a kernel is only a result once every line is verified (--verify-last).")
    for r in data["runs"]:
        if r.get("suspect"):
            print(f"  suspect {r['tag']} {r['kernel']}: {'; '.join(r['suspect'])}")
    for w in data["warnings"]:
        print("  note: " + w)
    print(f"results: {data['out']}", flush=True)


def queue_job(a, argv):
    name = ("hot" if a.hot else "") + a.queue
    q = S + "/ov/queue"
    marker = f"{TOOLS}/results/{name}.done"
    out = a.out or f"{TOOLS}/results/{name}.json"
    os.makedirs(os.path.dirname(out), exist_ok=True)
    for p in (marker, out):
        if os.path.exists(p):
            os.remove(p)
    words = [w for w in argv]
    i = words.index("--queue")
    del words[i:i + 2]
    if "--out" not in words:
        words += ["--out", out]
    script = (f'#!/bin/bash\n# ab.py job\nexport AB_LOCK_HELD=1\ncd {shlex.quote(os.getcwd().replace(chr(92), "/"))}\n'
              f'python {shlex.quote(os.path.abspath(__file__).replace(chr(92), "/"))} {shlex.join(words)}\nrc=$?\necho "$rc" > {shlex.quote(marker)}\nexit $rc\n')
    tmp = f"{q}/{name}.tmp"
    with open(tmp, "w", newline="\n") as f:
        f.write(script)
    os.replace(tmp, f"{q}/gpu/{name}.sh")
    print(f"queued {q}/gpu/{name}.sh; log {q}/done/gpu-{name}.sh.log", flush=True)
    return wait_on(name, out, a.wait)


def wait_on(name, out, seconds):
    marker = f"{TOOLS}/results/{name}.done"
    end = time.time() + seconds
    while time.time() < end:
        if os.path.exists(marker):
            if os.path.exists(out):
                report(json.load(open(out)))
            else:
                print(f"the job ended with no results: see {S}/ov/queue/done/gpu-{name}.sh.log")
            return 0
        time.sleep(3)
    print(f"not yet: python {os.path.abspath(__file__)} --wait-on {name}")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("variants", nargs="*", help="NAME=PATH[:DEF=VALUE...][@JSON]; the first is the reference")
    ap.add_argument("--passes", type=int, default=3)
    ap.add_argument("--count", type=int, help="deals a run: default 2,000,000, with --hot 60,000,000")
    ap.add_argument("--from", dest="first", type=int, help="first seed: default 600000000000, with --hot 700000000000")
    ap.add_argument("--config", help="--gpu-config JSON for every variant without its own")
    ap.add_argument("--warm", type=int, help="deals of the first variant before the timed runs: default 10 x --count; with --hot 240,000,000, past the CPU's power-limit drop at about 2.3 min")
    ap.add_argument("--verify-last", action="store_true", help="after the timed passes, one more run of each kernel with every line verified")
    ap.add_argument("--verify-threads", type=int, default=0, help="threads for it: 0 = all")
    ap.add_argument("--hot", action="store_true", help="acc batch: the laptop to itself, sustained speed from the speed log")
    ap.add_argument("--extra", help="more arguments for acc batch (with --hot)")
    ap.add_argument("--exe", default=TOOLS + "/accgpu.exe")
    ap.add_argument("--acc", default=TOOLS + "/acc.exe")
    ap.add_argument("--endgame", default=TOOLS + "/endgame")
    ap.add_argument("--out", help="results JSON")
    ap.add_argument("--min-clock", type=float, default=1300, help="flag a run below this mean GPU clock, MHz")
    ap.add_argument("--max-cpu", type=float, default=9.5, help="flag a GPU-alone run above this many busy CPU threads")
    ap.add_argument("--max-hold", type=float, default=300, help="refuse a GPU-alone plan estimated longer than this, s")
    ap.add_argument("--dry-run", action="store_true", help="check the variants and print the plan; no GPU run")
    ap.add_argument("--no-lock", action="store_true", help="only when the caller already holds the machine lock")
    ap.add_argument("--queue", metavar="NAME", help="go through the GPU queue")
    ap.add_argument("--wait", type=float, default=590, help="with --queue: seconds to wait for the result")
    ap.add_argument("--wait-on", metavar="NAME", help="wait for a queued job and print its table")
    ap.add_argument("--show", metavar="FILE", help="print the table from a results file")
    a = ap.parse_args()
    if a.show:
        return report(json.load(open(a.show)))
    if a.wait_on:
        return wait_on(a.wait_on, a.out or f"{TOOLS}/results/{a.wait_on}.json", a.wait)
    if not a.variants:
        fail("name at least one variant")
    variants = [parse_variant(t) for t in a.variants]
    names = [v["name"] for v in variants]
    if len(set(names)) != len(names):
        fail("two variants have one name")
    a.count = a.count or (60_000_000 if a.hot else 2_000_000)
    a.first = a.first if a.first is not None else (700_000_000_000 if a.hot else 600_000_000_000)
    a.warm = a.warm if a.warm is not None else (240_000_000 if a.hot else 10 * a.count)
    held = os.environ.get("AB_LOCK_HELD") or a.no_lock
    a.run_dir = TOOLS
    a.endgame = os.path.abspath(a.endgame)

    speed = 1.7e6 if a.hot else 2.0e6
    runs = a.passes * len(variants)
    estimate = (a.warm + runs * a.count) / speed + 3 * (runs + 1)
    if a.verify_last and not a.hot:
        estimate += len(variants) * (a.count / speed + 5 + a.count * SECONDS_A_LINE / max(1, (a.verify_threads or (os.cpu_count() or 16) - 2)))
    if not held or a.dry_run:
        for v in variants:
            if a.hot:
                if v["path"].endswith(".cu") or v["path"] == "builtin" or v["defines"]:
                    if not v["path"].lower().endswith(".exe") and not os.path.exists(a.acc):
                        fail(f"no {a.acc}")
                continue
            if v["path"].lower().endswith(".exe") and "--json" not in subprocess.run([v["path"], "--help"], capture_output=True, text=True, cwd=a.run_dir).stdout:
                fail(f"variant {v['name']}: {v['path']} is older than --json; give the kernel as a .cu with {a.exe}")
            # Fail now, not after waiting for the lock. This compiles only.
            with tempfile.TemporaryDirectory() as d:
                cmd = accgpu_command(a, v, 1, False)
                keep = [cmd[0]] + (["--gpu-config", v["config"] or a.config] if v["config"] or a.config else [])
                keep += (["--kernel", v["path"]] if v["path"].endswith(".cu") else [])
                for x in v["defines"]:
                    keep += ["--define", x]
                p = subprocess.run(keep + ["--ptx", d + "/x.ptx"], capture_output=True, text=True, cwd=a.run_dir)
                if p.returncode:
                    fail(f"variant {v['name']} doesn't compile:\n{p.stderr[-2000:]}")
    print(f"plan: {a.warm:,} warm-up deals, then {runs} runs of {a.count:,} ({', '.join(names[i] for _, i in order(len(names), a.passes))}){', verified pass' if a.verify_last and not a.hot else ''}; hold about {estimate:.0f} s", flush=True)
    if not a.hot and estimate > a.max_hold:
        fail(f"estimated hold {estimate:.0f} s is over the {a.max_hold:.0f} s limit: fewer passes or deals, or --max-hold if you must")
    if a.dry_run:
        return
    if a.queue:
        return queue_job(a, sys.argv[1:])
    if not held:
        env = dict(os.environ, AB_LOCK_HELD="1")
        return subprocess.call([sys.executable, S + "/machine.py", "h" if a.hot else "x", "--", sys.executable, os.path.abspath(__file__), *sys.argv[1:]], env=env)

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = os.path.abspath(a.out or f"{TOOLS}/results/ab-{stamp}.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    a.work = tempfile.mkdtemp(prefix="ab-") if not a.hot else os.path.join(TOOLS, "hot-work", stamp)
    data = {"tool": "ab.py", "argv": sys.argv[1:], "hot": a.hot, "count": a.count, "first": a.first, "passes": a.passes, "started": stamp, "finished": None,
            "min_clock": a.min_clock, "max_cpu": a.max_cpu, "variants": names, "out": out, "lock": "held" if os.environ.get("AB_LOCK_HELD") else "caller says held (--no-lock)",
            "kernels": {v["name"]: {"path": v["path"], "defines": v["defines"], "config": v["config"] or a.config,
                                    "sha": sha(v["path"]) if os.path.exists(v["path"]) else None} for v in variants},
            "exe": {p: sha(p) for p in ([a.acc] if a.hot else [a.exe]) if os.path.exists(p)}, "runs": [], "hold_s": 0}
    sampler = Sampler(2000 if a.hot else 200)
    sampler.start()
    time.sleep(2.5)  # typeperf and nvidia-smi take a moment to start
    t0 = time.time()
    data["gpu_busy_at_start"] = sampler.gpu_busy_now()
    one = (lambda v, tag, count: run_hot(a, v, sampler, tag, count)) if a.hot else (lambda v, tag, count: run_alone(a, v, sampler, tag, count))
    if not a.hot:
        for v in variants:  # a new kernel's first load takes the driver about 15 s of CPU, with the GPU idle: not in a timed pass
            one(v, "jit", 65536)
    if a.warm:
        w = one(variants[0], "warm", a.warm)
        data["warm"] = f"{a.warm:,} deals of {names[0]}" + (f": {w['deals_per_s']:.0f} deals/s, {w.get('clock_mhz') or 0:.0f} MHz, {w.get('temp_c') or 0:.0f} C" if "error" not in w else f": {w['error']}")
    for p, i in order(len(variants), a.passes):
        r = one(variants[i], f"p{p + 1}", a.count)
        r["pass"] = p
        data["runs"].append(r)
        data["hold_s"] = time.time() - t0
        data["finished"] = None
        summarise(data)
        with open(out + ".tmp", "w") as f:
            json.dump(data, f, indent=1)
        os.replace(out + ".tmp", out)
    if a.verify_last and not a.hot:
        data["verify"] = [run_alone(a, v, sampler, "verify", a.count, verify=True) for v in variants]
    data["hold_s"] = time.time() - t0
    sampler.stop()
    data["finished"] = time.strftime("%Y%m%d-%H%M%S")
    summarise(data)
    with open(out + ".tmp", "w") as f:
        json.dump(data, f, indent=1)
    os.replace(out + ".tmp", out)
    if a.hot:
        shutil.rmtree(a.work, ignore_errors=True)
    report(data)
    bad = any((r.get("verify") or {}).get("failed_lines") or not (r.get("verify") or {}).get("all_won") for r in data.get("verify", []))
    return 1 if bad or any("error" in r for r in data["runs"]) else 0


if __name__ == "__main__":
    sys.exit(main())
