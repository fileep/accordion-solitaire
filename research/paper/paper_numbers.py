# Prints every figure in the paper that depends on the big run, so that they can all be filled in at once when it ends.
#
#   python research/paper/paper_numbers.py [run report or .status.json]     (default: the report in research/runs, or else the live status)
#   python research/paper/paper_numbers.py <deals> <lost>                    (for the statistics at another point of the run)
#
# The rate is the one measured; the 95% interval is the exact (Clopper-Pearson) one, with Garwood's and Wilson's beside it. Losses are the records in
# hard-deals/proofs below the run's position; a line says so if they disagree with the run's own count.
import csv
import json
import math
import re
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "research/runs/batch-0-2000000000000.report.txt"
SECOND = ROOT / "research/runs/chacha-0-200000000000.report.txt"  # the run with the second generator
STATUS = ROOT / "checkpoints/batch-0-2000000000000.status.json"
CERTIFICATES = ROOT / "research/lean/automaton/certificates.csv"
LEAN = ROOT / "research/lean/automaton/lean_results.csv"
REPLAY = ROOT / "research/lean/automaton/replay_results.csv"
EARLY = 22_000_000_000  # the seeds that earlier runs had already solved
Z = 1.959963985


def from_status(path):
    s = json.loads(path.read_text(encoding="utf-8"))
    return {"deals": s["done"], "lost": s["lost"], "not_won": s["not_won_by_solver"], "proofs_won": s["proofs"]["won"], "undecided": s["undecided"],
            "unverified": s["unverified"], "proof_failed": s["proof_failed"], "on_cpu": s["deals_on_cpu"], "attempts": s["gpu"]["attempts"],
            "speed": s["deals_per_s_overall"], "hours": s["hours_so_far"]}


def from_report(path):
    t = path.read_text(encoding="utf-8")

    def number(pattern):
        return float(re.search(pattern, t, re.M).group(1))

    deals = int(number(r"^deals\s+(\d+)"))
    return {"deals": deals, "lost": int(number(r"^proofs\s+(\d+) proven lost")), "not_won": int(number(r"^not won\s+(\d+)")),
            "proofs_won": int(number(r"proven lost, (\d+) won")), "undecided": int(number(r"won, (\d+) undecided")),
            "unverified": int(number(r"^failed to verify\s+(\d+)")), "proof_failed": 0, "on_cpu": deals - int(number(r"^GPU\s.*?won (\d+) of the")),
            "attempts": number(r"([\d.]+)M attempts") * 1e6, "speed": number(r"^throughput\s+(\d+) deals/s"), "hours": number(r"\(([\d.]+) s wall") / 3600}


def wilson(lost, deals):
    p = lost / deals
    d = 1 + Z * Z / deals
    centre = (p + Z * Z / (2 * deals)) / d
    half = Z * math.sqrt(p * (1 - p) / deals + Z * Z / (4 * deals * deals)) / d
    return centre - half, centre + half


def wilson_as_centre(lost, deals):
    """Wilson's interval as centre ± half-width, two significant figures on the half-width, as Blake and Gent write it. The centre
    is not the rate seen: it adds z²/2 losses in z² deals."""
    low, high = wilson(lost, deals)
    half = 100 * (high - low) / 2
    places = 1 - math.floor(math.log10(half))
    return f"{100 * (1 - (low + high) / 2):.{places}f} ± {half:.{places}f}%"


def solve(f, target, low, high):
    """The p in [low, high] where f(p) = target, for f falling as p grows, by bisection on log p."""
    a, b = math.log(low), math.log(high)
    for _ in range(200):
        m = (a + b) / 2
        a, b = (m, b) if f(math.exp(m)) > target else (a, m)
    return math.exp((a + b) / 2)


def clopper_pearson(lost, deals, alpha=0.05):
    """The exact 95% interval for a loss rate: the rates under which `lost` or more, or `lost` or fewer, has chance alpha/2.
    log C(n, i) is built from log(n - j) term by term, so that a trillion deals loses no digits."""
    log_choose = [0.0]
    for i in range(1, lost + 1):
        log_choose.append(log_choose[-1] + math.log(deals - i + 1) - math.log(i))

    def at_most(k, p):
        return sum(math.exp(log_choose[i] + i * math.log(p) + (deals - i) * math.log1p(-p)) for i in range(k + 1))

    high = solve(lambda p: at_most(lost, p), alpha / 2, 1e-30, 1 - 1e-12)
    low = 0.0 if lost == 0 else solve(lambda p: at_most(lost - 1, p), 1 - alpha / 2, 1e-30, high)
    return low, high


def garwood(lost, deals, alpha=0.05):
    """The exact Poisson interval, for a check: at these rates it must match Clopper-Pearson."""
    def at_most(k, mean):
        return sum(math.exp(-mean + i * math.log(mean) - math.lgamma(i + 1)) for i in range(k + 1))

    high = solve(lambda m: at_most(lost, m), alpha / 2, 1e-9, 10.0 * lost + 50)
    low = 0.0 if lost == 0 else solve(lambda m: at_most(lost - 1, m), 1 - alpha / 2, 1e-9, high)
    return low / deals, high / deals


def percent(lost, deals, rate=None):
    """A win rate in percent, with enough places to show the losses' first three digits."""
    rate = lost / deals if rate is None else rate
    places = max(0, 2 - math.floor(math.log10(100 * lost / deals))) if lost else 6
    return f"{100 * (1 - rate):.{places}f}%"


def binomial_two_sided(k, n, p):
    """Exact two-sided p: the outcomes no likelier than the one seen."""
    def log_pmf(i):
        return math.lgamma(n + 1) - math.lgamma(i + 1) - math.lgamma(n - i + 1) + i * math.log(p) + (n - i) * math.log1p(-p)
    seen = log_pmf(k)
    return min(1.0, sum(math.exp(log_pmf(i)) for i in range(n + 1) if log_pmf(i) <= seen + 1e-12))


def uniform_placement(seeds, deals):
    """Kolmogorov-Smirnov test of the loss seeds against uniform placement over the run."""
    n = len(seeds)
    d = max(max((i + 1) / n - s / deals, s / deals - i / n) for i, s in enumerate(seeds))
    lam = (math.sqrt(n) + 0.12 + 0.11 / math.sqrt(n)) * d
    return min(1.0, 2 * sum((-1) ** (j - 1) * math.exp(-2 * j * j * lam * lam) for j in range(1, 100)))


def ranks(values):
    order = sorted(range(len(values)), key=values.__getitem__)
    out, i = [0.0] * len(values), 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        for k in range(i, j + 1):
            out[order[k]] = (i + j) / 2
        i = j + 1
    return out


def spearman(x, y):
    a, b = ranks(x), ranks(y)
    ma, mb = statistics.mean(a), statistics.mean(b)
    return sum((p - ma) * (q - mb) for p, q in zip(a, b)) / math.sqrt(sum((p - ma) ** 2 for p in a) * sum((q - mb) ** 2 for q in b))


def certificates(losses):
    rows = [r for r in csv.DictReader(CERTIFICATES.open(encoding="utf-8")) if int(r["seed"]) in losses]
    positions, nodes = [int(r["positions"]) for r in rows], [int(r["nodes"]) for r in rows]
    print(f"certificates        {len(rows)} of {len(losses)} losses" + ("" if len(rows) == len(losses) else "   NOT ALL: run check_certificates.py"))
    print(f"  all passed        {all(r['ok'] == 'True' for r in rows)}")
    print(f"  positions         median {statistics.median(positions):,}, most {max(positions):,}")
    print(f"  nodes             median {statistics.median(nodes):,}, most {max(nodes):,}")
    print(f"  Spearman          {spearman(positions, nodes):.2f} between positions and nodes")
    print(f"  log-log slope     {log_log_slope(positions, nodes):.2f}: nodes grow roughly as positions to this power (least squares)")


def log_log_slope(x, y):
    a, b = [math.log(v) for v in x], [math.log(v) for v in y]
    ma, mb = statistics.mean(a), statistics.mean(b)
    return sum((p - ma) * (q - mb) for p, q in zip(a, b)) / sum((p - ma) ** 2 for p in a)


def lean(losses):
    if not LEAN.exists():
        print("Lean                no results yet: run prove_all.py")
        return
    rows = [r for r in csv.DictReader(LEAN.open(encoding="utf-8")) if r["ok"] == "True"]
    proved = {int(s) for r in rows for s in r["seeds"].split()}
    print(f"Lean proved         {len(proved & losses)} of {len(losses)} losses" + ("" if losses <= proved else f"   MISSING: {sorted(losses - proved)[:10]}"))
    print(f"  CPU               {sum(float(r['cpu_seconds']) for r in rows) / 3600:.2f} hours, one Lean start-up per batch ({len(rows)} batches)")
    print(f"  peak memory       {max(float(r['peak_gb']) for r in rows):.1f} GB in one Lean")
    if REPLAY.exists():
        rows = [r for r in csv.DictReader(REPLAY.open(encoding="utf-8")) if r["ok"] == "True"]
        again = {int(s) for r in rows for s in r["seeds"].split()}
        print(f"nanoda accepted     {len(again & losses)} of {len(losses)} losses" + ("" if losses <= again else f"   MISSING: {sorted(losses - again)[:10]}"))
        print(f"  CPU               {sum(float(r['cpu_seconds']) for r in rows) / 3600:.2f} hours")
    else:
        print("nanoda              no results yet: run replay.py")


def second_generator(lost, deals):
    """The run under ChaCha20 against the main run: given the losses of both, each falls in the second in proportion to its deals."""
    if not SECOND.exists():
        print("second generator    no results yet")
        return
    other = from_report(SECOND)
    low, high = clopper_pearson(other["lost"], other["deals"])
    print(f"second generator    {other['lost']} lost of {other['deals']:,}, {other['undecided']} undecided, {other['unverified']} wins failed verification")
    print(f"  losses per billion  {other['lost'] / other['deals'] * 1e9:.3f} measured, {low * 1e9:.3f} to {high * 1e9:.3f} (95%, exact), against {lost / deals * 1e9:.3f}")
    print(f"  same rate?          exact two-sided p = {binomial_two_sided(other['lost'], other['lost'] + lost, other['deals'] / (other['deals'] + deals)):.3g}")
    # Given both runs' losses, the second's share is binomial; its exact interval gives one for the ratio of the two rates.
    share_low, share_high = clopper_pearson(other["lost"], other["lost"] + lost)
    ratio = lambda share: share / (1 - share) * deals / other["deals"]
    print(f"  ratio of rates      {ratio(other['lost'] / (other['lost'] + lost)):.2f}, 95% interval {ratio(share_low):.2f} to {ratio(share_high):.2f} (exact, conditional)")


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    args = sys.argv[1:]
    if len(args) == 2:
        run = {"deals": int(args[0]), "lost": int(args[1])}
    else:
        path = Path(args[0]) if args else REPORT if REPORT.exists() else STATUS
        run = from_status(path) if path.suffix == ".json" else from_report(path)
        print(f"from                {path}")
    deals, lost = run["deals"], run["lost"]
    seeds = sorted(s for s in (int(p.name.split("-")[1]) for p in (ROOT / "hard-deals/proofs").glob("seed-*-loss.json")) if s < deals)
    if len(seeds) != lost:
        print(f"NOTE: {len(seeds)} loss records below deal {deals:,}, and the run counts {lost}")
    low, high = clopper_pearson(lost, deals)
    poisson, score = garwood(lost, deals), wilson(lost, deals)
    print(f"deals               {deals:,}")
    print(f"lost                {lost:,}")
    print(f"win rate            {percent(lost, deals)} measured: {lost / deals * 1e9:.3f} lost per billion, 1 deal in {deals / lost:,.0f}")
    print(f"95% interval        {low * 1e9:.3f} to {high * 1e9:.3f} lost per billion (exact, Clopper-Pearson), 1 deal in {1 / high:,.0f} to {1 / low:,.0f}")
    print(f"  as a win rate     {percent(lost, deals, high)} to {percent(lost, deals, low)}; ±{50 * (high - low) / (lost / deals):.1f}% of the rate")
    print(f"  Garwood (Poisson) {poisson[0] * 1e9:.3f} to {poisson[1] * 1e9:.3f} lost per billion, a check")
    print(f"  Wilson            {score[0] * 1e9:.3f} to {score[1] * 1e9:.3f} lost per billion, {percent(lost, deals, score[1])} to {percent(lost, deals, score[0])}; "
          f"as Blake and Gent write it, {wilson_as_centre(lost, deals)}")
    if "not_won" in run:
        print(f"exhaustive stage    {run['not_won']} reached it: {run['proofs_won']} won, {lost} proved lost, {run['undecided']} undecided")
        print(f"failures            {run['unverified']} wins failed verification, {run['proof_failed']} proofs failed")
        print(f"GPU                 {run['attempts'] / deals:.1f} tries a deal, misses {run['on_cpu'] / deals * 1e5:.1f} deals in 100,000")
        print(f"speed               {run['speed'] / 1e6:.1f}M deals/s over {run['hours']:.0f} hours")
    early = sum(s < EARLY for s in seeds)
    if seeds and deals > EARLY:
        print(f"first 22 billion    {early} lost, {early / (EARLY / 1e9):.2f} per billion; later {(len(seeds) - early) / ((deals - EARLY) / 1e9):.2f}; "
              f"exact two-sided p = {binomial_two_sided(early, len(seeds), EARLY / deals):.3g}")
        print(f"uniform placement   Kolmogorov-Smirnov p = {uniform_placement(seeds, deals):.3g}")
    second_generator(lost, deals)
    certificates(set(seeds))
    lean(set(seeds))


if __name__ == "__main__":
    main()
