# Replays winning lines with the independent Python rules and generator of make_golden.py, which share no code with the Rust
# ones, to check the win side of a run at scale. For every record: its cards must be the deal of its seed by the Python
# generator, every move must put a pile on a matching card 1 or 3 places to its left, and a win must leave one pile after
# exactly 51 moves. A rule too loose in the solver and its replay would show here as an illegal move.
#
#   acc batch --from 500000000000 --count 1000000 --gpu on --no-dossiers --no-log --out lines.jsonl
#   python testdata/tools/replay_lines.py lines.jsonl [more.jsonl ...] [--jobs N]     (from the repository root)
#
# It prints one summary line per file and exits with 1 if any record fails.
import base64
import json
import multiprocessing
import sys
from collections import Counter

from make_golden import deal_from_seed, matches, parse_card, play


def card_line(cards, moves):
    """Text moves ("7H>7C": the pile topped by 7H onto the pile topped by 7C) as (pile, distance) pairs, or None if a card
    is not a top card or the distance is not 1 or 3."""
    tops, out = list(cards), []
    for move in moves:
        mover, target = (parse_card(t) for t in move.split(">"))
        if mover not in tops or target not in tops:
            return None
        i, j = tops.index(mover), tops.index(target)
        if i - j not in (1, 3):
            return None
        out.append((i, i - j))
        tops = list(play(tuple(tops), i, i - j))
    return out


def check(text):
    """('win' | 'not a win' | a failure, seed) for one JSON line."""
    record = json.loads(text)
    seed = record["deal"].get("seed")
    cards = [parse_card(t) for t in record["deal"]["cards"].split()]
    if seed is None or cards != deal_from_seed(seed):
        return "cards are not the seed's deal", seed
    if record["result"] != "win":
        return "not a win", seed
    if record.get("moves_b64"):
        line = [(b >> 1, 3 if b & 1 else 1) for b in base64.b64decode(record["moves_b64"])]
    else:
        line = card_line(cards, record["moves"])
        if line is None:
            return "illegal move", seed
    tops = tuple(cards)
    for i, d in line:
        if not d <= i < len(tops) or not matches(tops[i], tops[i - d]):
            return "illegal move", seed
        tops = play(tops, i, d)
    if len(line) != 51 or len(tops) != 1:
        return "does not end in one pile after 51 moves", seed
    return "win", seed


def lines(path):
    with open(path, encoding="utf-8") as f:
        for text in f:
            if text.strip():
                yield text


def main():
    args = sys.argv[1:]
    jobs = multiprocessing.cpu_count() - 1
    if "--jobs" in args:
        k = args.index("--jobs")
        jobs = int(args[k + 1])
        del args[k:k + 2]
    failed = 0
    with multiprocessing.Pool(jobs) as pool:
        for path in args:
            counts, first = Counter(), {}
            for outcome, seed in pool.imap(check, lines(path), chunksize=4000):
                counts[outcome] += 1
                first.setdefault(outcome, seed)
            bad = {k: v for k, v in counts.items() if k not in ("win", "not a win")}
            failed += sum(bad.values())
            seeds = [first[k] for k in counts if k in first]
            print(f"{path}: {sum(counts.values())} records, seeds {min(seeds)} up, {counts['win']} wins replayed, "
                  f"{counts['not a win']} not wins, {sum(bad.values())} failed" + "".join(f"; {k}: {v} (first seed {first[k]})" for k, v in bad.items()),
                  flush=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
