# Lean proofs of the lost deals

A lost deal is proved in the Lean 4 kernel from a small automaton of its reachable positions. Everything is in `automaton/`, on
Lean 4.35.0 (`lean-toolchain`). The checker is proved correct for any certificate, with no `sorry`; each theorem depends only on
`propext` and `Quot.sound`.

## The files

| File | What it is |
|---|---|
| `automaton/Accordion.lean` | Rules on piles, the deal generator, the theorem that a passing certificate proves the loss, a replayed win |
| `automaton/Certificate.lean` | Loads a certificate table straight into the kernel, for speed. Needs no trust: a wrong table fails |
| `automaton/dawg.py` | Builds the automaton by forward reachability on automata |
| `automaton/lean_certificates.py` | Writes the Lean text for a seed |
| `automaton/prove_all.py` | Proves every loss in batches, with a memory cap |
| `automaton/replay.py` | Checks those proofs again in nanoda, a second kernel |
| `automaton/lean_tests.py` | Writes the tests: a real win, and bad certificates the kernel must refuse |
| `automaton/check_certificates.py` | A second checker in Python that shares no code with `dawg.py`; results in `certificates.csv` |

To build and prove:

```bash
lean -R research/lean/automaton -o build/Accordion.olean research/lean/automaton/Accordion.lean
```

```bash
LEAN_PATH=build lean -R research/lean/automaton -o build/Certificate.olean research/lean/automaton/Certificate.lean
```

```bash
python research/lean/automaton/prove_all.py hard-deals/proofs research/lean/automaton/lean_results.csv build
```

`prove_all.py` takes a number of jobs as a fourth argument, for when nothing else is running. Each Lean takes up to about 3 GB.

To check the proofs in a second kernel, `nanoda_bin`, which is in the Lean toolchain and written in Rust, sharing no code with Lean's:

```bash
python research/lean/automaton/replay.py research/lean/automaton/lean_results.csv research/lean/automaton/replay_results.csv build
```

## What has been checked

- With the current checker, these losses are proved in the kernel: seeds 3,417,058,778, 7,311,711,652, 112,249,515, 202,346,720,
  1,005,616,798 and 124,280,819,596. The full list of losses has not been run: that waits for the end of the 2 trillion deal run.
- nanoda accepts the five smaller of those proofs, checking about 1,750 declarations a batch with only `propext` and `Quot.sound` allowed.
  Changing one number in an exported proof, by pointing an application at a different number, was refused by its type checker 6 times
  out of 6. A batch costs about the same CPU as Lean's check (for the five deals: 7-12 s, plus about 4 s to export), and 0.14 GB against
  Lean's 2.3 GB, though the export itself needs 2.3 GB. `lean4lean`, also in the toolchain, can't read this toolchain's files here.
- `lean_tests.py` compiles: a 51-move win of seed 0 replays under the same rules, and a dropped edge, a false inclusion claim, a
  winnable row and an empty table are all refused.
- `check_certificates.py` passes all 138 losses it was run on, and agrees with an explicit search on 400 random rows.

## Cost

One core of the laptop, at idle priority beside the main run, in user CPU time. Start-up is about 1.5 s and 2 GB, as Lean's own
library is loaded; a batch of deals pays that once.

| Deal | Positions | Nodes | CPU | Peak memory |
|---|---|---|---|---|
| Seed 112,249,515 | 19,386,305 | 1,135 | 7.6 s | 2.3 GB |
| Seed 1,005,616,798, the most work of the first 138 | 3,405,471 | 9,464 | 168 s | 2.6 GB |

Across the first 138 losses the check does 4.06 million table reads, so about 17 minutes of CPU, and about 3 hours for 1,450 losses
(an estimate from the two deals above).

How the checker got here, so nobody repeats it. The first version took 34 s on seed 112,249,515 and ran out of its time limit on
seed 1,005,616,798; a batch reached 40 GB and pushed the 2 trillion run into paging.

- **Inclusion claims as a search tree instead of a list** was the largest gain, 5.5 times on the hardest deal. A node can have 2,697
  claims, and scanning them was 55 million steps.
- **Many small theorems per deal** caps memory, as the kernel frees its caches after each declaration. A theorem must also stay
  under about 1,000 nodes, because the kernel recurses once per node of the run (`MAX_RUN`).
- **Recursors instead of compiled recursion and `match`, and small loop bodies** gave about a factor of 2.5, and one Lean start-up per batch saves 1.5 s a deal.
- **Tried and dropped:** one packed `Nat` table of all edges, read with shifts. It was 4 times slower, as every read copies a large number.

## Limits

- It covers named losses only. The count of wins rests on the replay in `acc`.
- A misread rule would pass every check, so the text of the rules is the part to review.
- Won deals don't compress this way: one reached 238,723 nodes in a single layer at 34 piles.

## The earlier studies

Later studies override earlier ones. `leanproof.md` set out the trust model and the plan, `regions.md` found the closed splits of
jammed deals, and `lean2.md` found the automaton route. Their Lean timings are estimates made before Lean was installed, and
`lean2.md` assumes about 1,040 losses.

Scripts were written for one machine and may have absolute paths in them.
