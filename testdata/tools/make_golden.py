# Builds the golden test data in testdata/ from an implementation independent of the Rust code:
# the deal generator and the deck-input rules are re-implemented here from docs/FORMATS.md, and wins
# are found by a small Python search. The Rust golden tests (crates/core/tests/golden.rs) must agree
# with every file this writes, which checks both the code and the written spec.
#
#   python testdata/tools/make_golden.py        (from the repository root)
import base64
import json
import os
import random
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
M64 = (1 << 64) - 1
RANKS = "A23456789TJQK"
SUITS = "CDHS"


# ---- deal generation (docs/FORMATS.md "Deals from seeds") -------------------------------------------

class SplitMix64:
    def __init__(self, seed):
        self.state = seed & M64

    def next(self):
        self.state = (self.state + 0x9E3779B97F4A7C15) & M64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
        return z ^ (z >> 31)


def rotl(x, k):
    return ((x << k) | (x >> (64 - k))) & M64


class Xoshiro256StarStar:
    def __init__(self, seed):
        sm = SplitMix64(seed)
        self.s = [sm.next() for _ in range(4)]

    def next(self):
        s = self.s
        result = (rotl((s[1] * 5) & M64, 7) * 9) & M64
        t = (s[1] << 17) & M64
        s[2] ^= s[0]
        s[3] ^= s[1]
        s[1] ^= s[2]
        s[0] ^= s[3]
        s[2] ^= t
        s[3] = rotl(s[3], 45)
        return result

    def below(self, n):
        m = self.next() * n
        if (m & M64) < n:
            threshold = ((1 << 64) - n) % n
            while (m & M64) < threshold:
                m = self.next() * n
        return m >> 64


def name(i):  # card number 0-51, canonical order: clubs A..K, diamonds, hearts, spades
    return RANKS[i % 13] + SUITS[i // 13]


def deal_from_seed(seed):
    rng = Xoshiro256StarStar(seed)
    cards = list(range(52))
    for i in range(51, 0, -1):
        j = rng.below(i + 1)
        cards[i], cards[j] = cards[j], cards[i]
    return cards


# ---- deck input (docs/FORMATS.md "Deck input") -----------------------------------------------------

SUIT_OF = {"C": 0, "♣": 0, "♧": 0, "D": 1, "♦": 1, "♢": 1, "H": 2, "♥": 2, "♡": 2, "S": 3, "♠": 3, "♤": 3}


def parse_card(token):
    t = token.strip().replace("️", "")
    if not t or t[-1].upper() not in SUIT_OF:
        return None
    suit, rank_text = SUIT_OF[t[-1].upper()], t[:-1].upper()
    if rank_text == "10":
        rank = 9
    elif len(rank_text) == 1 and rank_text in RANKS:
        rank = RANKS.index(rank_text)
    else:
        return None
    return suit * 13 + rank


def tokens(text):
    return [t for t in text.replace(",", " ").split() if t]


def deck_issues(numbered):  # [(position, card)] -> duplicate and missing issues
    seen, issues = {}, []
    for pos, c in numbered:
        if c in seen:
            issues.append({"kind": "duplicate", "card": name(c), "first": seen[c], "second": pos})
        else:
            seen[c] = pos
    missing = [name(c) for c in range(52) if c not in seen]
    if missing:
        issues.append({"kind": "missing", "cards": missing})
    return issues


def check_deck(text):
    numbered, issues = [], []
    for pos, tok in enumerate(tokens(text), 1):
        c = parse_card(tok)
        if c is None:
            issues.append({"kind": "unknown_token", "position": pos, "token": tok})
        else:
            numbered.append((pos, c))
    issues += deck_issues(numbered)
    return (" ".join(name(c) for _, c in numbered), None) if not issues else (None, issues)


def check_position(text):
    numbered, issues, piles, pos = [], [], [], 1
    for p, segment in enumerate(text.split("|"), 1):
        toks = tokens(segment)
        if not toks:
            issues.append({"kind": "empty_pile", "pile": p})
        pile = []
        for tok in toks:
            c = parse_card(tok)
            if c is None:
                issues.append({"kind": "unknown_token", "position": pos, "token": tok})
            else:
                numbered.append((pos, c))
                pile.append(name(c))
            pos += 1
        piles.append(pile)
    issues += deck_issues(numbered)
    return (piles, None) if not issues else (None, issues)


# ---- a small independent solver ----------------------------------------------------------------------

def matches(a, b):
    return a // 13 == b // 13 or a % 13 == b % 13


def moves_of(tops):
    for i in range(1, len(tops)):
        if matches(tops[i], tops[i - 1]):
            yield i, 1
        if i >= 3 and matches(tops[i], tops[i - 3]):
            yield i, 3


def play(tops, i, d):
    return tops[:i - d] + (tops[i],) + tops[i - d + 1:i] + tops[i + 1:]


def solve(deal, budget=3_000_000):
    dead, line, nodes = set(), [], 0

    def go(tops):
        nonlocal nodes
        nodes += 1
        if nodes > budget:
            raise TimeoutError
        if len(tops) == 1:
            return True
        if tops in dead:
            return False
        for i, d in moves_of(tops):
            line.append((i, d))
            if go(play(tops, i, d)):
                return True
            line.pop()
        dead.add(tops)
        return False

    try:
        return list(line) if go(tuple(deal)) else None
    except TimeoutError:
        return None


def card_moves(deal, line):
    tops, out = tuple(deal), []
    for i, d in line:
        out.append(f"{name(tops[i])}>{name(tops[i - d])}")
        tops = play(tops, i, d)
    return out, tops


def compact(line):
    return base64.b64encode(bytes((i << 1) | (d == 3) for i, d in line)).decode()


def piles_after(deal, line):  # full piles, bottom to top, after playing `line`
    piles = [[c] for c in deal]
    for i, d in line:
        moving = piles.pop(i)
        piles[i - d].extend(moving)
    return piles


def reachable(start, cap=300_000):
    """Exhaustive search over distinct top-card sequences: (positions reachable, winnable?, deepest line)."""
    start = tuple(start)
    parent = {start: None}
    stack, win, deepest = [start], False, start
    while stack:
        s = stack.pop()
        if len(s) == 1:
            win = True
        if len(s) < len(deepest):
            deepest = s
        for i, d in moves_of(s):
            t = play(s, i, d)
            if t not in parent:
                parent[t] = (s, (i, d))
                stack.append(t)
                if len(parent) > cap:
                    return None, None, None
    line, s = [], deepest
    while parent[s] is not None:
        s, mv = parent[s]
        line.append(mv)
    return len(parent), win, line[::-1]


def arrange_move_free(cards, suffix=()):
    """Orders `cards` so that, followed by `suffix`, no card matches the card 1 or 3 places to its left.
    Backtracking, trying first the cards of the suit with the most cards left (so no suit is stranded at the
    end), then canonical order. Deterministic."""
    n, out, used = len(cards), [], set()

    def clashes(i):
        c = out[i]
        if (i >= 1 and matches(c, out[i - 1])) or (i >= 3 and matches(c, out[i - 3])):
            return True
        return any(n + j - i in (1, 3) and matches(c, s) for j, s in enumerate(suffix))

    def place(i):
        if i == n:
            return True
        left = [sum(1 for c in cards if c // 13 == s and c not in used) for s in range(4)]
        for c in sorted((c for c in cards if c not in used), key=lambda c: (-left[c // 13], c)):
            out.append(c)
            if not clashes(i):
                used.add(c)
                if place(i + 1):
                    return True
                used.discard(c)
            out.pop()
        return False

    assert place(0), "no move-free arrangement found"
    return out


def no_move_deal():
    """A deal with no legal move at all."""
    return arrange_move_free(list(range(52)))


def dead_end_deal():
    """An unwinnable deal that still has legal moves, by construction:
    - the first 43 cards are frozen: no two match 1 or 3 apart, so none can ever move or be covered from inside;
    - a tail pile can only ever reach the last three frozen cards (AC 2D 3C), from 1 or 3 places away;
    - the tail (hearts and spades 4-8) shares no suit or rank with those three, so it plays among itself but
      never touches the frozen part, and at least 44 piles always remain.
    Exhaustive search over every reachable position confirms it."""
    boundary = [parse_card(t) for t in "AC 2D 3C".split()]
    tail = [parse_card(t) for t in "4H 4S 5H 5S 6H 6S 7H 7S 8H".split()]
    assert not any(matches(b, t) for b in boundary for t in tail)
    rest = [c for c in range(52) if c not in boundary and c not in tail]
    deal = arrange_move_free(rest, suffix=boundary) + boundary + tail
    n, win, line = reachable(deal)
    assert n is not None and not win and sum(1 for _ in moves_of(tuple(deal))) >= 3
    return n, deal, line


def py_verify(cards, moves_text, claims_win):
    """Independent rule check of a move list: (ok, 1-based move at fault or None)."""
    tops = tuple(cards)
    for k, text in enumerate(moves_text, 1):
        mover, _, target = text.partition(">")
        x, y = parse_card(mover), parse_card(target)
        if x is None or y is None or x not in tops or y not in tops:
            return False, k
        i, j = tops.index(x), tops.index(y)
        if i - j not in (1, 3) or not matches(x, y):
            return False, k
        tops = play(tops, i, i - j)
    if claims_win and len(tops) != 1:
        return False, None
    return True, None


def mutations(cards, moves_text, rng, count):
    """Near-miss variants of a winning line: retargeted, re-mover'd, swapped, dropped, duplicated, extended,
    truncated and diverging moves. Some stay legal wins (e.g. swapping two independent moves); the
    independent checker decides which."""
    out = []
    n = len(moves_text)
    for _ in range(count):
        m = list(moves_text)
        k = rng.randrange(n)
        tops = tuple(cards)
        for text in m[:k]:
            a, _, b = text.partition(">")
            x, y = parse_card(a), parse_card(b)
            tops = play(tops, tops.index(x), tops.index(x) - tops.index(y))
        kind = rng.choice(["retarget", "remover", "swap", "drop", "duplicate", "extend", "truncate", "diverge"])
        mover, _, target = m[k].partition(">")
        # Replacement cards come from the whole deck, so some are already buried (covered-card cheats).
        others = [c for c in range(52) if name(c) not in (mover, target)]
        if kind == "retarget":
            m[k] = f"{mover}>{name(rng.choice(others))}"
        elif kind == "remover":
            m[k] = f"{name(rng.choice(others))}>{target}"
        elif kind == "swap" and k + 1 < n:
            m[k], m[k + 1] = m[k + 1], m[k]
        elif kind == "drop":
            del m[k]
        elif kind == "duplicate":
            m.insert(k, m[k])
        elif kind == "extend":
            m.append(f"{mover}>{target}")
        elif kind == "truncate":
            m = m[:k]
        else:  # diverge: a different legal move here, then the original remainder
            options = [(i, d) for i, d in moves_of(tops) if f"{name(tops[i])}>{name(tops[i - d])}" != m[k]]
            if options:
                i, d = rng.choice(options)
                m[k] = f"{name(tops[i])}>{name(tops[i - d])}"
        out.append(m)
    return out


# ---- writing ---------------------------------------------------------------------------------------------

RULES = {"deal": "open", "leaps": [1, 3], "match": "suit-or-rank", "win": "one-pile"}


def record(cards, result, moves=None, seed=None, moves_b64=None, rules=None):
    deal = {"cards": " ".join(name(c) for c in cards)}
    if seed is not None:
        deal = {"seed": seed, **deal}
    rec = {"format": "accordion-solve/1", "rules": rules or RULES, "deal": deal, "result": result}
    if moves is not None:
        rec["moves"] = moves
    if moves_b64 is not None:
        rec["moves_b64"] = moves_b64
    rec["stats"] = {"source": "testdata/tools/make_golden.py"}
    return rec


def write_json(path, obj):
    with open(os.path.join(ROOT, path), "w", encoding="utf-8", newline="\n") as f:
        json.dump(obj, f, indent=2, ensure_ascii=False)
        f.write("\n")


def main():
    os.makedirs(os.path.join(ROOT, "games"), exist_ok=True)

    seeds = [0, 1, 2, 42, 12345, 2**32, 2**63, M64]
    write_json("deals_by_seed.json", {
        "about": "Deal::from_seed(seed) must print exactly these cards (spec: docs/FORMATS.md).",
        "deals": [{"seed": s, "cards": " ".join(name(c) for c in deal_from_seed(s))} for s in seeds],
    })

    # A seeded deal with a win that uses leaps, found by the Python solver.
    for seed in range(42, 100):
        deal = deal_from_seed(seed)
        line = solve(deal)
        if line and sum(d == 3 for _, d in line) >= 3:
            break
    else:
        sys.exit("no seed in 42..99 solved with leaps")
    win_text, final = card_moves(deal, line)
    leaps = sum(d == 3 for _, d in line)
    print(f"seed {seed}: win in {len(line)} moves with {leaps} leaps, final card {name(final[0])}")

    # The snake: every neighbour matches, so 51 slides of pile 1 onto pile 0 win.
    snake = list(range(13)) + [13 + r for r in range(12, -1, -1)] + [26 + r for r in range(13)] + [39 + r for r in range(12, -1, -1)]
    snake_line = [(1, 1)] * 51
    snake_text, _ = card_moves(snake, snake_line)

    games, manifest = {}, []

    def game(file, recs, expect):
        games[file] = recs
        manifest.append({"file": file, "expect": expect})

    ok = lambda result, moves, piles: {"ok": True, "result": result, "moves": moves, "piles_left": piles}
    fail = lambda move=None: {"ok": False, "move": move}

    game("snake_slides.json", [record(snake, "win", snake_text)], [ok("win", 51, 1)])
    game(f"seed_{seed}_win.json", [record(deal, "win", win_text, seed)], [ok("win", 51, 1)])
    game(f"seed_{seed}_win_compact.json", [record(deal, "win", None, seed, compact(line))], [ok("win", 51, 1)])
    game(f"seed_{seed}_win_both.json", [record(deal, "win", win_text, seed, compact(line))], [ok("win", 51, 1)])
    game(f"seed_{seed}_partial.json", [record(deal, "unknown", win_text[:20], seed)], [ok("unknown", 20, 32)])
    revenge = {**RULES, "final_card": name(final[0])}
    game("revenge_final_card.json", [record(deal, "win", win_text, seed, rules=revenge)], [ok("win", 51, 1)])

    # Failures, each with the 1-based move at fault (or null when the record as a whole is wrong).
    tops = tuple(deal)
    t = tuple(deal)
    for i, d in line[:10]:
        t = play(t, i, d)
    p = 2  # pile 2 onto pile 0 is never a legal distance
    bad = win_text[:10] + [f"{name(t[p])}>{name(t[p - 2])}"] + win_text[11:]
    game("bad_wrong_distance.json", [record(deal, "win", bad, seed)], [fail(11)])
    q = next(i for i in range(1, 52) if not matches(tops[i], tops[i - 1]))
    game("bad_no_match.json", [record(deal, "win", [f"{name(tops[q])}>{name(tops[q - 1])}"] + win_text[1:], seed)], [fail(1)])
    game("bad_not_finished.json", [record(deal, "win", win_text[:50], seed)], [fail()])
    game("bad_seed_mismatch.json", [record(deal_from_seed(seed + 1), "win", win_text, seed)], [fail()])
    other_first = next((i, d) for i, d in moves_of(tops) if (i, d) != line[0])
    game("bad_compact_mismatch.json", [record(deal, "win", win_text, seed, compact([other_first] + line[1:]))], [fail(1)])
    dup = list(deal)
    dup[40] = dup[3]
    game("bad_duplicate_card.json", [record(dup, "win", win_text)], [fail()])
    wrong_final = {**RULES, "final_card": name(next(c for c in range(52) if c != final[0]))}
    game("bad_revenge_final_card.json", [record(deal, "win", win_text, seed, rules=wrong_final)], [fail()])

    # More cheating moves a buggy solver could make, each injected as move 6 of the real win.
    t5 = tuple(deal)
    for i, d in line[:5]:
        t5 = play(t5, i, d)
    covered = next(c for c in deal if c not in t5)  # a card already buried under another pile
    p = next(i for i in range(4, len(t5)) if matches(t5[i], t5[i - 4]))  # a matching pair 4 apart

    def cheat(file, bad_move):
        game(file, [record(deal, "win", win_text[:5] + [bad_move] + win_text[6:], seed)], [fail(6)])

    cheat("bad_covered_mover.json", f"{name(covered)}>{name(t5[0])}")
    cheat("bad_covered_target.json", f"{name(t5[1])}>{name(covered)}")
    cheat("bad_moves_right.json", f"{name(t5[0])}>{name(t5[1])}")
    cheat("bad_onto_itself.json", f"{name(t5[3])}>{name(t5[3])}")
    cheat("bad_four_left.json", f"{name(t5[p])}>{name(t5[p - 4])}")
    extra = f"{name(final[0])}>{name(next(c for c in range(52) if c != final[0]))}"
    game("bad_move_after_win.json", [record(deal, "win", win_text + [extra], seed)], [fail(52)])
    game("bad_compact_illegal.json", [record(deal, "win", None, seed, compact([(q, 1)] + line[1:]))], [fail(1)])

    # Unwinnable deals. "loss" records verify their moves only; a claimed win on them must always fail.
    nm = no_move_deal()
    assert not any(True for _ in moves_of(tuple(nm)))
    de_states, de, de_line = dead_end_deal()
    de_text, _ = card_moves(de, de_line)
    print(f"dead-end deal: {sum(1 for _ in moves_of(tuple(de)))} opening moves, {de_states} reachable positions, none won; "
          f"best line leaves {52 - len(de_line)} piles")
    game("unwinnable_no_moves.json", [record(nm, "loss", [])], [ok("loss", 0, 52)])
    game("unwinnable_dead_end.json", [record(de, "loss", de_text)], [ok("loss", len(de_line), 52 - len(de_line))])
    game("bad_fake_win_no_moves.json", [record(nm, "win", [f"{name(nm[1])}>{name(nm[0])}"])], [fail(1)])
    game("bad_fake_win_dead_end.json", [record(de, "win", de_text)], [fail()])
    game("bad_fake_win_dead_end_cheat.json", [record(de, "win", de_text + [f"{name(de[1])}>{name(de[0])}"])], [fail(len(de_line) + 1)])

    # A realistic dead end: a legal random line from the seed deal into a position that can't be won.
    prng = random.Random(42)
    while True:
        tops, rline = tuple(deal), []
        for _ in range(36):
            opts = list(moves_of(tops))
            if not opts:
                break
            i, d = prng.choice(opts)
            rline.append((i, d))
            tops = play(tops, i, d)
        n36, w36, _ = reachable(tops, cap=50_000)
        if len(rline) == 36 and n36 is not None and not w36 and any(True for _ in moves_of(tops)):
            break
    random_dead = piles_after(deal, rline)
    print(f"seed {seed} random line: 36 legal moves into an unwinnable {len(random_dead)}-pile position ({n36} reachable)")

    # Exhaustive-search reference: Rust must reach the same number of distinct positions and the same verdict.
    seed_game_40 = piles_after(deal, line[:40])
    n40, w40, _ = reachable([p[-1] for p in seed_game_40])
    exhaustive = {
        "about": "Distinct top-card sequences reachable from each start (including it), and whether any is a win.",
        "cases": [
            {"name": "no legal moves", "position": " | ".join(name(c) for c in nm), "reachable": 1, "winnable": False},
            {"name": "dead end", "position": " | ".join(name(c) for c in de), "reachable": de_states, "winnable": False},
            {"name": f"seed {seed} after 40 moves", "position": " | ".join(" ".join(name(c) for c in p) for p in seed_game_40),
             "reachable": n40, "winnable": w40},
            {"name": f"seed {seed} after 36 random moves (dead end)",
             "position": " | ".join(" ".join(name(c) for c in p) for p in random_dead), "reachable": n36, "winnable": False},
        ],
    }
    assert w40, "the seed game's position after 40 moves must be winnable"

    # Mutation cross-check: near-miss variants of real wins, judged by this file's independent rule checker.
    rng = random.Random(2026)
    bases = [(deal, win_text, seed), (snake, snake_text, None)]
    for s in range(seed + 1, seed + 40):
        if len(bases) >= 4:
            break
        d = deal_from_seed(s)
        l = solve(d, budget=500_000)
        if l:
            bases.append((d, card_moves(d, l)[0], s))
    mutated, mutated_expect = [], []
    for cards, text, s in bases:
        for m in mutations(cards, text, rng, 75):
            mutated.append(record(cards, "win", m, s))
            good, at = py_verify(cards, m, True)
            mutated_expect.append(ok("win", len(m), 1) if good else fail(at))
    legal = sum(e["ok"] for e in mutated_expect)
    print(f"mutations: {len(mutated)} variants of {len(bases)} wins, {legal} still legal wins, {len(mutated) - legal} rejected")

    for file, recs in games.items():
        write_json(os.path.join("games", file), recs[0])
    batch = [record(deal, "win", win_text, seed), record(deal, "unknown", win_text[:20], seed), record(snake, "win", snake_text)]
    with open(os.path.join(ROOT, "games", "batch.jsonl"), "w", encoding="utf-8", newline="\n") as f:
        for rec in batch:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    manifest.append({"file": "batch.jsonl", "expect": [ok("win", 51, 1), ok("unknown", 20, 32), ok("win", 51, 1)]})
    with open(os.path.join(ROOT, "games", "mutations.jsonl"), "w", encoding="utf-8", newline="\n") as f:
        for rec in mutated:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    manifest.append({"file": "mutations.jsonl", "expect": mutated_expect})
    write_json("exhaustive.json", exhaustive)
    write_json(os.path.join("games", "manifest.json"), {
        "about": "Expected acc verify outcome per record; move numbers are 1-based, null means the record as a whole.",
        "games": manifest,
    })

    # Deck and position input cases, with issues computed by this file's own reading of the spec.
    canon1 = " ".join(name(c) for c in deal_from_seed(1))
    fancy = canon1.lower().replace(" ", ",\n").replace("th", "10h").replace("ks", "K♠").replace("ad", "A♦️").replace("qc", "q♧")
    cards1 = canon1.split()
    with_problems = list(cards1)
    with_problems[4] = "1H"
    with_problems[39] = with_problems[2]
    repeated = cards1 + [cards1[7]]
    deck_cases = [
        ("canonical", canon1),
        ("lowercase, commas, newlines, 10, suit symbols, emoji selector", fancy),
        ("unknown token and duplicate", " ".join(with_problems)),
        ("51 cards", " ".join(cards1[:-1])),
        ("53 cards (one repeated)", " ".join(repeated)),
        ("empty", ""),
        ("junk around valid cards", "deck: " + canon1 + " ;"),
    ]
    position_cases = [
        ("two piles", cards1[0] + " " + cards1[1] + " | " + " ".join(cards1[2:])),
        ("empty pile", cards1[0] + " | | " + " ".join(cards1[1:])),
        ("one pile", " ".join(cards1)),
    ]
    out = {"about": "Deal::parse / parse_position results; issue kinds and positions as in docs/FORMATS.md.", "decks": [], "positions": []}
    for label, text in deck_cases:
        cards, issues = check_deck(text)
        out["decks"].append({"name": label, "input": text, **({"ok": cards} if cards else {"issues": issues})})
    for label, text in position_cases:
        piles, issues = check_position(text)
        out["positions"].append({"name": label, "input": text, **({"ok": piles} if piles else {"issues": issues})})
    write_json("deck_input.json", out)
    print(f"wrote deals_by_seed.json, deck_input.json and {len(games) + 1} files in games/")


if __name__ == "__main__":
    main()
