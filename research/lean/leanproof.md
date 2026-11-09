# A Lean proof of Accordion wins and losses

[M] = measured here or recorded in the repo. [E] = my estimate, which may be off by 3-10x. Lean was not installed or run, so every Lean speed is [E].

**Answer.** Prove the losses and only sample the wins. Lean can enumerate every position of each lost deal itself, so it needs no certificate, no pruning proofs and no solver change. About 1,040 losses is 0.5-5 core-hours compiled [E]. A proof of all trillion wins is not practical. The first step is half a day.

## 1. What is proved, and what is trusted

The theorem: for each named seed, no sequence of legal moves from that seed's deal ends in one pile. Lean needs about 30 lines of rules (piles, slide, leap, match, win) and about 40 lines for the generator [E]. The solver never enters the argument.

| Design | Trusted, beyond the rules and generator as written in Lean | Limit |
|---|---|---|
| Kernel only (`decide +kernel`) | Lean's kernel and GMP. Can be replayed by lean4checker. | Slow |
| Native (`native_decide`) | Also the Lean compiler, C compiler, runtime and `@[implemented_by]` code. Reported as an axiom; Lean's docs say outside checkers cannot check these proofs. | Fast |
| Separate checker program | The same as native. | Prints a result, not a theorem |

- **Rust is outside the trusted base in all three.** Winning lines are hints: a bad line can only make the check fail. A loss needs just its seed, so the input for 1,040 losses is about 20 KB [E].
- **The real remaining risk is the rules text.** If "Accordion" is misread, Rust and Lean share the mistake. Thirty readable lines are the thing to review.
- **The generator is easy.** Transcribe it, and test it against the 23 seed/card pairs in the dossiers. That it gives a permutation is an easy proof. That it is uniform cannot be proved, and the theorem doesn't need it.
- **What it cannot say:** "exactly 1,040 losses in a trillion". That needs every win proved too. "Each of these 1,040 deals is lost" is what it can say.

## 2. Wins

A win is already its own certificate: 51 moves that anyone can replay. Lean adds least here. One theorem per deal is the wrong shape. The right shape is one checker, proved correct once, run over a list.

| Method | Per deal | 1,000 deals | 1 billion | 1 trillion |
|---|---|---|---|---|
| Rust replay today | 604-699 ns [M] | instant | 11 min of one core | 7-8 core-days |
| Lean, compiled | 2-10 us [E] | instant | 0.6-3 core-hours | 23-116 core-days, 1.5-7 days on 16 threads |
| Lean, kernel | 0.01-1 s, say 0.1 s [E] | about 2 min | about 3 core-years | about 3,000 core-years |

Storage decides it. The run keeps no lines. At 7 bits a move they are 45 bytes a deal: 45 GB for a billion, 45 TB for a trillion, about 15 TB packed [E]. The disk has 303 GB free [M, Nov 8]. Running a Lean checker inside `acc batch` would avoid storage, but costs 3-15x the current CPU verifying and leaves nothing anyone can recheck. So prove the checker in general, then run it on a chosen set: a gallery, or the first million seeds.

## 3. Losses

**A certificate is only useful when checking is cheaper than finding.** For an exhaustive loss the two are the same work, so Lean should simply enumerate. Each move removes exactly one pile, so positions fall into layers by pile count and only two layers need to be in memory. The soundness proof needs only that every child of a layer lands in the next one (extras are harmless), and that the one-pile layer is empty.

Recorded sizes, 23 proven losses, plain run [M]:

| | Smallest | Median | Mean | Largest | Sum |
|---|---|---|---|---|---|
| Positions reachable | 128 | 79,426 | 1.17M | 19,386,305 | 26.9M |
| Moves generated | 416 | 379,939 | 6.3M | 110,959,471 | 145.9M |
| Rust seconds | 0.00 | 0.046 | 1.48 | 29.2 | 34 |

Two details differ from the brief. A median proof is 0.05 s, not 1-2 s. The 19.5 billion figure in `INDEX.md` is solver effort, not proof size; that deal's proof is 19.4M positions.

**Pruning is not worth proving [M].** The independent run (tests, sleep sets, tables) stores only 0.4% fewer positions than the plain run, and generates 69% fewer moves. So all that proof work buys at most 3x in time and nothing in memory. The likely cause: 17 of the 23 deals never get below 27 piles (move 25), and the dead-end tests can't fire before move 15 (left-leaf, move 26) [RESEARCH.md], so they have little room to act. Sleep sets combined with a dead-position set are also the subtlest proof, and endgame tables are a 20 GB input needing their own proof. The plain run needs none of this, and it is what `acc` already reruns on every loss.

**A plain-Python check [M]:** layer-by-layer enumeration on the 12 smallest losses matched the dossiers exactly on both counts (371,486 positions, 1,714,580 moves) in 4.3 s, 398k moves/s. That is a third independent implementation agreeing. The peak layer was 4-27% of a deal's positions, and an average position had 27-48 piles.

| Scaled to 1,040 losses | Figure |
|---|---|
| Positions, moves | 1.2-1.6 billion, 6.6-8.6 billion [E, from the 23] |
| Largest single loss | 150-230M positions [E, log-normal tail fitted to only 23 points, shaky] |
| Compiled Lean, 0.5-4M moves/s | 0.5-5 core-hours in all, about 20 min for the largest [E]. Python's pace would be 5-6 hours. |
| Memory | About 8 GB for two layers of the largest, at 120 bytes a position [E]; typical deals, megabytes |
| Kernel only, 1-10 ms a move | Smallest loss, seconds. Median, 6 min to 1 hour. Deals up to 1M moves (16 of 23 [M]), about 3 hours at worst. All 1,040: 3 months to 3 years of one core, and the largest would likely run out of memory [E]. |
| If a certificate file were still wanted | Sorted layers, 6 bits a pile, 36-48 GB raw [E]. It only lowers memory on the rare giants. |

Solver changes needed: none. The seeds are already in `hard-deals/proofs/`.

Side note: many reachable counts factor into small primes (128 = 2^7, 15,840 = 2^5·3^2·5·11), which suggests rows split into independent regions. A proof by regions could shrink the sets a lot. Untested, and unnecessary at these sizes.

## 4. Plan

| Stage | Work | Size [E] |
|---|---|---|
| 0, first milestone | Lean file with rules and generator. Prove seed 3,417,058,778 (128 positions) lost and one 51-move win, kernel only. | Half a day |
| 1 | General checker and theorem: `check d = true` implies not winnable. Includes "only top cards matter". | 2-4 days, 300-600 lines |
| 2 | Fast version (flat arrays, `UInt64`) proved to match. Run on the 23 recorded losses and compare with `dead_positions`. This replaces my estimates with real speeds. | 1-2 days |
| 3 | After the trillion run: generate the seed list, prove all ~1,040, give big-memory treatment to any giants. | A day plus hours of CPU |
| 4, optional | Win sample; kernel-only proofs for all deals under about 1M moves. | Days |

Easy: rules, generator, closure argument, top-card abstraction. Hard: proving the fast version matches, and making the kernel version run (structural recursion with fuel, `Nat` encodings, shallow recursion). The `Nat` encodings help because the kernel has fast `Nat` arithmetic.

What could go wrong:
- A misread rule shared by Rust and Lean.
- Stack overflows or interpreter speed under `native_decide`; use `precompileModules` or a compiled program.
- Lean version churn: 4.29 changed how native computations appear as axioms, and well-founded recursion's reducibility has shifted.
- A rare giant loss exceeding this laptop's RAM.
- Native proofs resting on the compiler, which has had reported soundness bugs in `native_decide` paths.

## 5. Recommendation

**Worth doing only in this small form:** a theorem for the named losses, with a sample of wins, after the speed work and the trillion run. It wins no extra deals and adds no speed. Losses are already double-checked in Rust, and the likeliest error (a misread rule) would survive Lean. It buys a public, independently checkable claim about the only results that aren't self-certifying. If no such claim is planned, skip it.

Use `native_decide` or a compiled checker for all ~1,040, and kernel-only for as many small ones as is cheap. Don't prove pruning, don't touch the solver, and don't attempt all wins.

**Cheapest first step:** stage 0, which needs Lean installed (`elan`, by you). If stage 0 feels clumsy, stop there.

Sources: lean-lang.org/doc/reference/4.28.0-rc1/ValidatingProofs/ (trust); lean-lang.org/doc/reference/4.28.0/Basic-Types/Natural-Numbers/ (kernel `Nat`); lean-lang.org/doc/reference/4.36.0-rc1/releases/v4.29.0/ (native axioms); leanprover-community Zulip archive (`native_decide` trust and soundness threads, kernel recursion).
