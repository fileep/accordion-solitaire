# Lean proofs of the lost deals: studies so far

Nothing here has been run in Lean. Lean is not installed, `automaton/Accordion.lean` has never been compiled and its proofs are mostly outlines with `sorry`. In the reports, [M] means measured and [E] means estimated; every Lean speed is [E].

Read them in this order. Each later study overrides the one before it where they differ.

| File | Date | What it says | Standing |
|---|---|---|---|
| `leanproof.md` | 2026-11-08 | Trust model, what the theorem can and can't say, wins vs losses, staged plan | Current on trust and wins. Its "kernel-only is impractical for all losses" is overtaken |
| `regions.md` | 2026-11-08 | A closed split of the row into blocks proves a jammed deal lost; 17 of 23 split | Correct as far as tested, but no longer needed. A hand argument, not machine-checked |
| `lean2.md` | 2026-11-09 | The whole reachable set fits a small automaton over the row of top cards; the check is one pass over its nodes | Latest. This is the route to follow |

## The main result

Write a position as the word of its top cards. A lost deal's reachable set is a small minimal automaton: 26.9M positions across the 23 recorded losses become 29,171 nodes, and the 19.4M-position deal becomes 1,135. The proof is three checks: the deal's word is accepted, no one-card word is accepted, and every slide or leap from an accepted word stays accepted. Kernel-only is estimated at 2-36 core-hours for about 1,040 losses.

## What has been checked

- **The agent's tests** are in `lean2.md` section 2: counts match the records on 23 of 23 losses, and mutated automatons are rejected.
- **A separate check** (`automaton/spot_check.py`, written without reading `dawg.py`) enumerated three near-wins, built the automaton and ran the three checks:

| Seed | Words = record | Moves = record | Nodes (claimed) | Check | One-word deletions caught |
|---|---|---|---|---|---|
| 12929752010 | 79,426 | 223,841 | 2,689 (2,689) | pass | 5 of 5 |
| 18238151902 | 168,098 | 568,832 | 2,413 (2,413) | pass | 5 of 5 |
| 5895817790 | 957,502 | 4,359,712 | 1,734 (1,734) | pass | 5 of 5 |

- **A second checker on every loss so far** (`automaton/check_certificates.py`, results in `automaton/certificates.csv`, 2026-11-09). It shares no code with `dawg.py`: the rules and generator are `testdata/tools/make_golden.py`'s, and the check is the one `check` states in `Accordion.lean`. All 138 losses found up to seed 187 billion pass, and each position count equals its record, the 19.4M deal's 1,135 nodes included. The check agreed with an explicit search on 400 random short rows (52 with a win) and rejected all 165 damaged certificates.
- **How certificates are made:** `dawg.py` computes the reachable set layer by layer on automata, never position by position, so its cost follows the nodes (about a second for the 19.4M deal). It is still a full forward search of the deal, done symbolically. What needs no search is the check: the three conditions prove the loss whichever program found the automaton.
- **The draft, reviewed but still not compiled:** the generator matches `crates/core/src/rng.rs` and `deal.rs`, and the rules match `make_golden.py`. `check` had a hole: it never required the root and edge targets to be nodes it had looked at, so a table with `size = 0` passed and would have proved any deal lost. `inBounds` closes it. `DealLost` now states the result on real piles, with the step down to top cards left as an outline.

## Limits for any claim

- It covers named losses only. It says nothing about the count of wins, so "exactly N losses in M deals" also depends on the win verification in `acc`.
- A misread rule would pass through every route. The rules text in Lean is the part to review.
- Certificates exist for the 138 losses so far: at most 10,468 nodes and 676,119 lookups [M]. The rest of the run may hold larger ones. A real won deal blew up (238,723 nodes in one layer by 34 piles), so there is no claim about wins.
- The estimates in `lean2.md` assume about 1,040 losses. The run in progress is 2 trillion deals, about 1,480 losses at its rate so far. The 138 took 4.06M lookups, 29,423 each [M], so about 44M in all and 1-24 core-hours at `lean2.md`'s 0.1-2 ms a lookup [E]. `check_certificates.py` counts a lookup a little differently from `dawg.py`: 35,042 against 41,651 on the 19.4M deal.

## Next steps (from `lean2.md` section 5)

1. Stage 0 in Lean: rules, generator tested on the recorded seed/card pairs, the checker, then seeds 3,417,058,778 and 112,249,515. This replaces the timing estimates with real ones. It needs Lean installed (`elan`), which is the user's to do.
2. Generate a certificate for every loss and recheck each in a second language.
3. Kernel-check them all, using `native_decide` only for a deal whose automaton is too big.

## Other files

- `automaton/`: `Accordion.lean` (draft), `dawg.py` (builds and checks automatons), `check_certificates.py` and `certificates.csv` (second checker and its results), `anatomy.py`, `falsify.py`, `spot_check.py`, logs and results.
- `regions/`: scripts and per-deal results for `regions.md`.
- `first-study/`: Python used for `leanproof.md`.

Scripts were written for one machine and may have absolute paths in them.
