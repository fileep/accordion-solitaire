# Lean proofs of the lost deals

A lost deal is proved in the Lean 4 kernel from a small automaton of its reachable positions. Everything is in `automaton/`, on
Lean 4.35.0 (`lean-toolchain`). The checker is proved correct for any certificate, with no `sorry`; each theorem depends only on
`propext` and `Quot.sound`.

## The files

| File | What it is |
|---|---|
| `automaton/Accordion.lean` | Rules on piles, the deal generator, the theorem that a passing certificate proves the loss, a replayed win |
| `automaton/Certificate.lean` | Loads a certificate table straight into the kernel, for speed. Needs no trust: a wrong table fails |
| `automaton/ChaCha.lean` | The second generator, ChaCha20, checked against RFC 8439's test vector, for the losses of the second-generator run |
| `automaton/dawg.py` | Builds the automaton by forward reachability on automata |
| `automaton/lean_certificates.py` | Writes the Lean text for a seed |
| `automaton/prove_all.py` | Proves every loss in batches, with a memory cap |
| `automaton/replay.py` | Checks those proofs again in nanoda, a second kernel |
| `automaton/lean_tests.py` | Writes the tests: a real win, and bad certificates the kernel must refuse |
| `automaton/check_certificates.py` | A second checker in Python that shares no code with `dawg.py`; results in `certificates.csv` |

The other files in `automaton/` and `regions/` are the earlier studies' scripts and logs (`lean2.md`, `leanproof.md`,
`regions.md`). `falsify.log` and `falsify2.log` end in a script error (a `TypeError` in `dawg.py`'s `verify`), so they hold
only the checks printed before it; `falsify3.log` and `falsify4.log` are later runs.

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

`prove_all.py` and `replay.py` run only on Windows for now: they find Lean at
`~/.elan/toolchains/leanprover--lean4---v4.35.0/bin/lean.exe`, cap each process's memory with Windows calls, and `replay.py`
uses `nanoda_bin.exe` and `leanexport.exe` from that toolchain. The Lean files themselves, and `lean` run by hand as above,
work anywhere Lean 4.35.0 does.

The second-generator run's losses are proved the same way, with `ChaCha.olean` in the build dir as well (built like
`Certificate.olean`). Each theorem then says `DealLost (ChaCha.deal seed)`: Lean computes the seed's ChaCha20 deal itself, so a
loss record with the wrong cards fails, and the same certificate claimed for another seed is refused.

```bash
python research/lean/automaton/prove_all.py hard-deals-chacha/proofs research/lean/automaton/lean_results_chacha.csv build --chacha
```

To check the proofs in a second kernel, `nanoda_bin`, which is in the Lean toolchain and written in Rust, sharing no code with Lean's:

```bash
python research/lean/automaton/replay.py research/lean/automaton/lean_results.csv research/lean/automaton/replay_results.csv build
```

## What has been checked

- All 721 losses of the trillion run are proved in the kernel, one theorem each, with `prove_all.py`: 34 batches of up to 37
  deals, 1.41 hours of CPU in all (39 minutes on 6 jobs), at most 2.7 GB in one Lean, and no batch failed. Row by row in
  `lean_results.csv`.
- nanoda accepts all 721 proofs, 70,270 declarations in the 34 batches, with only `propext` and `Quot.sound` allowed: 0.97 hours
  of CPU with the export (13 minutes on 6 jobs), and at most 2.35 GB, for the export. Row by row in `replay_results.csv`.
  Changing one number in an exported proof, by pointing an application at a different number, was refused by its type checker 6 times
  out of 6. A batch costs about the same CPU as Lean's check (on the five deals first tried: 7-12 s, plus about 4 s to export), and 0.14 GB against
  Lean's 2.3 GB, though the export itself needs 2.3 GB. `lean4lean`, also in the toolchain, can't read this toolchain's files here.
- `lean_tests.py` compiles: a 51-move win of seed 0 replays under the same rules, and a dropped edge, a false inclusion claim, a
  winnable row and an empty table are all refused.
- `check_certificates.py` passes all 721 losses of the trillion run, with every position count equal to its loss record, and agrees
  with an explicit search on 400 random rows. The certificates have 53 to 10,468 nodes (median 212) for 2 to 22,066,800 positions.

## Cost

One core of the laptop, at idle priority beside the main run, in user CPU time. Start-up is about 1.5 s and 2 GB, as Lean's own
library is loaded; a batch of deals pays that once.

| Deal | Positions | Nodes | CPU | Peak memory |
|---|---|---|---|---|
| Seed 112,249,515 | 19,386,305 | 1,135 | 7.6 s | 2.3 GB |
| Seed 1,005,616,798, the most work of the first 138 | 3,405,471 | 9,464 | 168 s | 2.6 GB |

Across the first 138 losses the check does 4.06 million table reads, so about 17 minutes of CPU, and about 3 hours for 1,450 losses
(an estimate from the two deals above). All 721 of the trillion run took 1.41 hours, start-ups included, beside nothing else
heavy but the Python loss checks.

How the checker got here, so nobody repeats it. The first version took 34 s on seed 112,249,515 and ran out of its time limit on
seed 1,005,616,798; a batch reached 40 GB and pushed the trillion run into paging.

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
