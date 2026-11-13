# Formats

Cards, decks, the deals of seeds, moves and solve files (`accordion-solve/1`) are frozen as of Phase 1. The Rust
implementation is `crates/core`. An independent Python implementation, `testdata/tools/make_golden.py`, generates the
golden files that the Rust tests must reproduce, so this document, the Rust code and the Python code all have to agree.
The files a run writes (report, status, log of events, speed log, dossiers, loss records) are at the end.

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

### The second generator

A build with the `second-generator` feature (`cargo build --release --features second-generator`) deals from ChaCha20
instead, to test that the win rate doesn't depend on the generator. The same seed gives a different deal, so its results
are kept apart, and its run reports have a `generator` line.

1. **ChaCha20** (RFC 8439, 20 rounds): the key's eight 32-bit words are `seed mod 2^32`, `seed >> 32` and six zeros; the
   nonce is three zero words, and the block counter starts at 0 and goes up by 1 a block.
2. **Bytes:** each 64-byte block is read in order, word 0 first, each word low byte first.
3. **Uniform `below(n)`** for 1 ≤ n ≤ 52 (mask and redraw): let `mask = 2^b − 1`, where `b` is the number of bits in
   `n − 1`; draw a byte, keep `byte & mask`, and redraw until it is below `n`.
4. **Fisher–Yates:** as above, `j = below(i + 1)` for `i` from 51 down to 1.

The code is `ChaCha20` in `crates/core/src/rng.rs`; its tests check the block function against RFC 8439's test vector.

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

Errors name the 1-based move at fault when there is one. Nothing counts as a win until it passes verification. [PLAN.md](PLAN.md) §6.2 lists the golden
cases that exercise each check, including unwinnable deals.

A `loss` record passes when its moves replay legally; the loss itself can't be confirmed by replay, so `acc verify` reports it as
claimed and says how to prove it again (`acc prove`).

## Files a run writes

`acc batch` (and so `acc run`) keeps its files beside its checkpoint, `checkpoints/batch-<from>-<count>.json` by default, named
after it: the checkpoint's path with another ending. They are explained for users in [READING_RESULTS.md](READING_RESULTS.md); this
is their exact form. Unlike the solve format above, they aren't frozen: fields may be added, but none is renamed or removed.

### The report

`<name>.report.txt`, written when a run that took more than a minute (or was carried on) finishes, and printed on standard
output by every run. Each line is a label padded to 17 characters, then text. The labels, in order:

| Label | Text |
|---|---|
| `deals` | `<count> (seeds <first>..<last>)` |
| `generator` | Only in a build with `--features second-generator`: `<name>: not the deals of the main runs` |
| `won` | `<won> (every line replayed: by the reference rules, or a GPU line by the quick check and 1 in 256 by the reference engine too)`, then `; by stage: <stage> <count>, ...` unless every win was `thorough`. Stages: `gpu`, `ladder-0` to `ladder-6`, `thorough`, `prove`. Reports of earlier versions say `(every one verified by the reference rules)`. |
| `not won` | `<n> by the solver (<e> exhausted, <u> hit the limit of <max-nodes> positions)` |
| `proofs` | `<lost> proven lost, <won> won, <undecided> undecided (two exhaustive runs of up to <prove> positions each)`, then `; <n> FAILED` if any failed. Absent with `--prove 0`. |
| `failed to verify` | `<n>` |
| `repeat odds` | `<x> that any two of these deals are the same game (suits and ranks renamed), <y> the same deal, if the generator behaves like a random one (...)`, in e-notation |
| `distinct games` | `all <n> deals differ, even with suits and ranks renamed`; for a sample, `not all checked: ...` with the sample's size and what it rules out; `<k> pairs of seeds deal the same game ...: <a>=<b>, ...` if any were found; `NOT CHECKED: <why>`; or `not checked (--no-games-check)` |
| `win rate` | `<p>%   95% CI [<lo>%, <hi>%] (Wilson; undecided deals counted as losses for the lower bound, wins for the upper)`, to 10 decimals (12 from 10^13 deals) |
| `positions/deal` | `mean <m>  median <m>  p90 <x>  p99 <x>  p99.9 <x>  max <x>`, then ` (<n> deals solved on the CPU)` with the GPU |
| `ms/deal (1 core)` | the same for time, in milliseconds |
| `cost sample` | only when more deals were solved on the CPU than their costs were kept for (125 million): which deals the median and percentiles cover |
| `GPU` | `<name>: won <w> of the <n> deals it was given (<p>%), <f> lines failed verification; <a>M attempts, <r>M positions/s`, then `; STOPPED EARLY: <why>` if it failed. With the GPU only. |
| `CPU threads` | shares of the CPU threads' time: `verifying GPU lines <p>% (<t> us a line), the GPU's misses <p>%, own deals <p>%, waiting for the GPU <p>%, handing results on <p>%, other <p>%`. With the GPU only. |
| `throughput` | `<d> deals/s, <c>M CPU positions/s on <hardware> (<s> s wall)`, with `, over <n> sessions` if it paused and carried on |
| `commits` | the commits the run was built from, if more than one |

`acc run` and `acc results` read the counts of `deals`, `won`, `not won`, `proofs` and `failed to verify` by these labels.

### The status file

`<name>.status.json`, rewritten (whole, by a rename) at every checkpoint, at a pause, once every deal is done and at the end, for
reading while the run goes.
`acc status` reads it.

| Field | Meaning |
|---|---|
| `state` | `running`, `finishing` (every deal done, the last checks running), `paused`, `paused: no room`, `paused: the GPU failed`, or `finished` |
| `updated` | When it was written, UTC, `YYYY-MM-DDTHH:MM:SSZ` |
| `from`, `count`, `done`, `left`, `percent` | The run's first seed and deals; deals done in seed order, and left |
| `deals_per_s_this_session`, `deals_per_s_overall` | Speeds |
| `hours_left`, `hours_so_far` | At this session's speed (null before there is one); time spent on deals over all sessions |
| `sessions` | How many times the run has started or carried on |
| `commit`, `hardware` | The commit of the folder it runs in (empty if that isn't a git folder), and `<n> threads` or `<n> CPU threads + GPU` |
| `won` | Deals won so far, by the solver and by proofs, as the report counts them |
| `lost`, `proving`, `undecided`, `unverified`, `proof_failed` | As in the progress line |
| `not_won_by_solver`, `failed_verification` | Deals the solver gave up on; winning lines that failed their check |
| `proofs` | `lost`, `won`, `undecided`, `failed`, `in_hand` |
| `gpu` | `won`, `launches`, `attempts` |
| `deals_on_cpu` | Deals solved on the processor |
| `games_log_gb`, `disk_free_gb` | The sample's log so far; free space where the checkpoint is (null if it can't be read) |
| `dossiers` | Where the dossiers go, or null with `--no-dossiers` |
| `save_every_s` | Seconds between checkpoints. Its presence also says the run takes a lock (`<name>.lock`). |
| `checkpoint` | The checkpoint's path |

Status files from older versions lack `won`, `dossiers`, `save_every_s` and `checkpoint`; readers must not need them. Such a
file was written by an acc that takes no lock, which in its last checks (`finishing`) rewrites it only when something changes,
and when it carries on, not before its first checkpoint (its log of events says at once that a session has started): `acc
status` and the rest judge those runs by these times, and `acc resume` and `acc run` touch them only with `--force`.

### The log of events

`<name>.events.log`: one line per event, `<UTC time> <what>`, appended. The events: `session <n> started at <done> deals, on commit
<c>`; `paused at <done> deals` (with `: the GPU failed: <why>` when it did); `paused after all the deals`; `seed <s>: proven lost`,
`won by the exhaustive search`, `undecided` or `PROOF FAILED`; `seed <s>: VERIFICATION FAILED: <why>`; `less than <x> GB free in <dir>:
pausing; make room and carry on`; `checkpoint: <error>`; `session <n> could not open the GPU: <why>`; `no deal finished in 15 minutes:
stopping`; `finished: <count> deals, report in <file>`.

### The speed log

`logs/batch-<YYYYMMDD-HHMMSS>Z.csv`, one per session, named by its start in UTC. The first line is a comment,
`# acc batch: seeds <first>..<last> (<count> deals[, carrying on after <done>]), <hardware>, commit <c>, started <time>Z`; then the header
and a row a second:

| Column | Meaning |
|---|---|
| `unix_time` | Seconds since 1970, UTC |
| `elapsed_s` | Seconds since the session started |
| `done`, `left` | Deals done in seed order, and left |
| `deals_per_s` | Since the row before |
| `gpu_wins_per_s`, `gpu_mpositions_per_s` | The GPU's wins a second, and millions of positions a second |
| `gpu_backlog` | GPU results waiting for the CPU threads to check them |

### Dossiers

`<dossiers>/<id>.json` (default `output/hard-deals/`), format `accordion-dossier/1`: the notes on a deal the solver didn't win at once.
`<id>` is `seed-<n>`, or `deck-<16 hex digits>` (FNV-1a of the canonical cards) for a deck that was typed. A run of the same deal adds to
its history.

| Field | Meaning |
|---|---|
| `format`, `id` | As above |
| `deal` | `cards` (canonical text) and `seed` if it has one |
| `rules` | As in solve files |
| `status` | `unknown` (the limit was reached), `exhausted` (every line searched), `solved (hard)` (won later), or `loss (proven)` |
| `features` | `initial_legal_moves`, `piles_with_a_move`: two counts of the opening position |
| `best` | `piles_left` and `moves` (card notation): the line that got furthest |
| `solution` | The winning line, once there is one |
| `history` | One entry per attempt, oldest first, each with `date` and `stage` |

A solver attempt (`stage` `batch`, `solve`, or `escalate-<n>`) has `result` (`win`, `exhausted`, `unknown`), `solver` (`acc <version>`),
`threads`, `max_nodes_per_thread`, `move_order_seed`, `config` (the solver's settings in full), `nodes`, `attempts`, `table_hits`,
`dead_end_cuts` and `seconds`. A proof (`stage` `prove`) has `solver`, `max_positions`, `plain` and `independent`. `plain` and
`independent.run` each have `result` (`lost`, `won`, `unfinished`), `positions`, `dead_positions`, `evicted_positions` (forgotten for lack of
memory, and searched again), `set_bytes` and `seconds`; `independent` also has `endgame_piles`, `move_order_seed` and `pattern_piles`.

`<dossiers>/INDEX.md` lists every dossier in a table, and the gallery (`--gallery`) if there is one; acc rewrites it after every run.
`<dossiers>/gallery/seed-<n>.json` are solve files of the hardest deals won, with `rank`, `positions`, `attempts`, `stage`, `run` and
`solver` in `stats`.

### Loss records

`<dossiers>/proofs/seed-<n>-loss.json`: a solve record (above) with `"result": "loss"`, the dossier's best line as `moves`, and
`stats.proof` holding the proof as in the dossier's history. `acc prove --out FILE` writes the same record.

### Other files

| File | Form |
|---|---|
| `<name>.json` | The checkpoint: JSON, with `version` 3. Read only by `acc batch`; nothing else should open it while the run goes, as that can make a save fail. |
| `<name>.costs` | For each deal solved on the CPU, in seed order, up to the first 125 million: its positions and its time in microseconds, each a little-endian unsigned 64-bit integer (16 bytes a deal). |
| `<name>.games` | The sample of the check for repeated games: 11 bytes a deal in the sample, in seed order: the low 5 bytes of the renamed deal's fingerprint, then the low 6 bytes of its position in the run, both little-endian. |
| `<name>.lock` | Empty; held locked while the run goes. |
| `<name>.pause` | Empty; its appearance asks the run to pause, and the run deletes it. |
| `endgame/endgame-<k>.bin` | The endgame table for `k` piles: the 8 bytes `ACCEGTB1`, `k` and the number of patterns as little-endian 64-bit integers, one bit per suit/rank pattern in 64-bit little-endian words (1: can be won), and a checksum of the words (FNV-1a's constants, a word at a time). |
