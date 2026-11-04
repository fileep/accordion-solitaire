# Formats — `accordion-solve/1`

These are frozen as of Phase 1. The Rust implementation is `crates/core`. An independent Python implementation,
`testdata/tools/make_golden.py`, generates the golden files that the Rust tests must reproduce, so this document, the
Rust code and the Python code all have to agree.

## Cards

- **Text:** two characters, a rank then a suit.
  - Ranks: `A 2 3 4 5 6 7 8 9 T J Q K`.
  - Suits: `C D H S` (clubs, diamonds, hearts, spades).
  - Example: `TH` is the ten of hearts.
- **Canonical order and card numbers 0–51:** clubs A…K, then diamonds, hearts, spades. Card number = `suit × 13 + rank`, with
  suit 0–3 and rank 0–12 (A = 0, K = 12).
- **Engine byte:** `suit << 4 | rank`. Two cards match (same suit or same rank) iff `x = a ^ b` has `x & 0x0F == 0` or `x & 0x70 == 0`.
  The padding sentinel `0x7F` never matches anything.

## Deck input

This covers text a person types or pastes, on the command line or in the visualizer. Reading is lenient and validation is strict.

**Reading:**

- Tokens are separated by whitespace or commas.
- Each token is one card, read case-insensitively:
  - the last character is the suit: `C D H S`, `♣ ♦ ♥ ♠` or `♧ ♢ ♡ ♤` (an emoji variation selector U+FE0F is ignored);
  - what comes before it is the rank: one of `A 2–9 T J Q K`, or `10` for the ten.
  - So `10h`, `TH` and `10♥` are the same card.
- Order is left to right, as dealt.

**Validation** requires exactly the 52 distinct cards. Every problem is reported at once, with 1-based token positions (every
token counts, including bad ones), in this order:

| `kind` | Fields | Meaning |
|---|---|---|
| `unknown_token` | `position`, `token` | The token is not a card |
| `duplicate` | `card`, `first`, `second` | A card appears again; one issue per repeat |
| `missing` | `cards` (canonical order) | Cards that never appear |

A deck is written back in canonical text: cards separated by single spaces.

**Positions** (games in progress, optional) use the same notation:

- Piles are separated by `|`, and each is listed bottom → top.
- All 52 cards must appear once.
- A pile with no tokens is reported as `{"kind": "empty_pile", "pile": n}` (1-based), before the duplicate and missing issues.
- Token positions count across piles.

## Deals from seeds

`Deal::from_seed(seed)` for an unsigned 64-bit `seed`:

1. **SplitMix64**, with state set to `seed`:
   ```
   state += 0x9E3779B97F4A7C15
   z = state
   z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
   z = (z ^ (z >> 27)) * 0x94D049BB133111EB
   return z ^ (z >> 31)
   ```
   All arithmetic is mod 2^64.
2. **xoshiro256\*\* 1.0**, with state `s[0..4]` set to the first four SplitMix64 outputs:
   ```
   result = rotl(s[1] * 5, 7) * 9
   t = s[1] << 17
   s[2] ^= s[0]; s[3] ^= s[1]; s[1] ^= s[2]; s[0] ^= s[3]; s[2] ^= t; s[3] = rotl(s[3], 45)
   return result
   ```
3. **Uniform `below(n)`** (Lemire):
   - Let `m = next() × n` as a 128-bit product.
   - If `low64(m) < n`: set `t = (2^64 − n) mod n`, and while `low64(m) < t`, redraw `m = next() × n`.
   - Return `m >> 64`.
4. **Fisher–Yates:** start from canonical order. For `i` from 51 down to 1: `j = below(i + 1)`, then swap positions `i` and `j`.

Reference values for testing an implementation:

- SplitMix64(1234567) starts 6457827717110365317, 3203168211198807973, 9817491932198370423.
- xoshiro256\*\* from state [1, 2, 3, 4] starts 11520, 0, 1509978240.
- Seeded deals are listed in `testdata/deals_by_seed.json`.

## Moves

- **Text:** `7H>7C`, meaning the pile whose top card is 7H moves onto the pile whose top card is 7C.
  - Top cards are unique, so this needs no pile numbering.
  - The target must be exactly 1 or 3 piles to the left, and the two cards must match.
  - Cards are read leniently, as in deck input.
- **Compact (engine and batches):** one byte per move, `pile << 1 | leap`.
  - `pile` is the 0-based index of the moving pile in the current row.
  - `leap` is 1 for a move 3 to the left and 0 for 1 to the left.
  - A whole line is standard padded base64 (RFC 4648 §4).

## Solve files

A solve file is one JSON object per game, as a `.json` file. Batches are JSON Lines (`.jsonl`): one object per line, with blank lines
ignored. A reader treats a file as a single object if the whole file parses as one JSON object, and as JSON Lines otherwise.

```json
{
  "format": "accordion-solve/1",
  "rules": { "deal": "open", "leaps": [1, 3], "match": "suit-or-rank", "win": "one-pile" },
  "deal": { "seed": 42, "cards": "…52 cards…" },
  "result": "win",
  "moves": ["7H>7C", "…"],
  "moves_b64": "…optional…",
  "stats": { "…": "free-form, carried through unchanged" }
}
```

| Field | Rules |
|---|---|
| `format` | Exactly `accordion-solve/1` |
| `rules` | Only the values shown are supported. Optional `final_card` (a card) is the Accordion's Revenge hook: a win must finish with that card on top |
| `deal.seed` | Optional unsigned 64-bit integer. If present, `deal.cards` must equal the deal for that seed. JavaScript should parse files through the WASM engine (or as BigInt), since seeds above 2^53 don't fit a JS number |
| `deal.cards` | Required, in deck-input notation. It is the authoritative deal |
| `result` | `win`, `unknown` (not won yet; `moves` is the best line so far) or `loss` (only from a completed, cross-checked exhaustive search; replay can't confirm it) |
| `moves` | Card-notation moves, possibly a partial line |
| `moves_b64` | The same moves in compact form. Either field may be given; if both are, they must agree move for move |
| `stats` | Optional object, not interpreted |

Unknown fields are ignored.

## Verification (`acc verify`)

A record passes when all of the following hold:

1. The format and rules are supported.
2. The cards form a valid deal that equals the seed's deal, if a seed is given.
3. Every move is legal when replayed by the reference engine:
   - the mover and the target are both top cards (never buried ones);
   - the target is exactly 1 or 3 piles to the left (not 0, 2, 4 or more, and never to the right);
   - the two cards share a suit or a rank.

   The whole pile moves and the gap closes. If both `moves` and `moves_b64` are given, they must be the same moves.
4. After replay, all 52 cards are still present exactly once.
5. A `win` ends in one pile after exactly 51 moves, with no move after that. The pile has `final_card` on top if the rules name one.

Errors name the 1-based move at fault when there is one. Nothing counts as a win until it passes verification. PLAN.md §6.2 lists the golden
cases that exercise each check, including unwinnable deals.
