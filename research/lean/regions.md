# Splitting a lost deal into independent regions

> Working notes from a study of 2026-11-09, kept as written. [E] figures were estimates made before Lean was
> installed; [README.md](README.md) has what was built and measured since.

[M] = measured here. [E] = my estimate. Lean was not run, so every Lean figure is [E]. Scripts and the table for each deal are in [regions/](regions/) (`per_deal.txt`).

**Answer.** The hypothesis holds for 17 of the 23 recorded losses [M]: exactly the 17 that jam at 27 or more piles. Those proofs shrink by a median of 79x in positions and 187x in moves, down to a few seconds of kernel time each [E]. The other 6 can't be split at all, and they hold 92% of the work [M]. A kernel-only proof becomes plausible for about three quarters of the losses, not for all.

## 1. The theorem

Play a block of consecutive cards alone, allowing only moves between its own piles, and call the positions it can reach its Reach. A split of the row into blocks is **closed** if, taking any one position per block, no legal move carries a pile from one block to another.

**Theorem.** If a closed split has 2 or more blocks, the deal is lost. Every reachable position is one position per block joined together, so counts multiply exactly. The fewest piles ever reached is the sum of each block's fewest.

**Proof sketch.** Follow any line of play up to its first crossing. Until then the position is one Reach position per block. A move inside a block depends only on that block, because only that block's piles lie between its two piles. A removed pile shifts only its own block, so the distance across a boundary depends only on the piles counted from that boundary. Closedness says no crossing is available, so none ever happens. Each block's first pile never moves, because moving would be a crossing. So at least 2 piles always remain.

**The check is small.** A crossing can only start from a block's first 3 piles and land on the last 3 piles of the row to its left. Each test compares one left slot with one right slot, so the set of cards each slot can hold is enough.

**Where it could fail:**
- A leap can pass over a block that has shrunk to 1 or 2 piles and land in the block beyond. The test must include this. Leaving it out produced 818 false claims (below).
- Exact counts also need the blocks' card sets to be disjoint, which they are, so no position is counted twice.
- "Only top cards matter" and the reading of the rules carry over from the earlier study.

**Status:** a hand argument, not machine-checked, but tested hard as below.

**Finding the split.** Start with each card as its own block. Enumerate each block alone, merge any blocks joined by a possible crossing, and repeat until stable. Each merge is forced by a real move, so this gives the finest closed split there is. If it ends with one block, no split exists.

## 2. Measurements

| [M] | Split (17 deals) | Not split (6 deals) |
|---|---|---|
| Reachable positions | 128 to 923,520 | 79,426 to 19,386,305 |
| Blocks | 19 to 44 | 1 |
| Widest block, median | 17 cards | 52 |
| Product of parts = exact count | 17 of 17 | n/a |
| Fewest piles from parts = recorded best | 17 of 17 | n/a |
| Shrink in positions (exact / sum of parts) | median 79x, range 2.4x to 564x | none |
| Shrink in moves | median 187x, range 15x to 3,688x | none |
| Total, all deals in the group | 2.23M positions become 19,245; 11.1M moves become 44,299 | 24.7M positions, 134.8M moves |
| Largest proof after splitting | 5,250 positions, 11,965 moves | n/a |

**The split-or-not line is the pile count.** All 17 deals that stop at 27 or more piles split. All 6 that reach 2 or 3 piles don't, as the row really does merge. That also explains why the dead-end tests in the earlier study did so little. The small-prime pattern is a symptom only: a split deal's count can still carry a large prime when one block dominates (1,913 and 1,759 are two such primes).

**The two large deals.** The 3.4M deal needed a 157 s run to settle: one block, 3,405,471 positions, matching the record [M]. The 19.4M deal merged into one block at a size the limit allowed, so no full enumeration was needed [M]; it was not enumerated exactly.

**Checks against exact enumeration [M], none failed:**
- Every legal move of every reachable position of 15 of the 17 split deals, with piles tracked by bottom card: zero crossed a boundary. Two deals were over the size limit for this check.
- On 9 deals the reachable set equals the literal product of the blocks' sets.
- On 20 verified winning lines (1,040 positions), the method never claimed a split. Only about 40% of those positions were real tests, since the others hit the 20,000-position limit and made no claim.
- Deliberately broken rules are caught. No leaps over short blocks: 818 of 1,040 winning-line positions falsely split, 18 of 18 products wrong. Only the first pile may leap: 15 of 18 products wrong, 4 false splits in 224. Slides only: 18 of 18 wrong, 942 false splits. The unbroken control: 0 and 0.

**Window proofs failed: 0 of 23 [M]** (windows of up to 8 cards each side, 4,000 positions, 6 s a deal). A window that ignores its neighbours must let piles leave its left edge, so it can always shrink past the slots that matter. Fixing that needs knowledge of the neighbouring blocks, which is just the split above. Since the whole split is already tiny, I stopped there.

**Barrier pruning does not help the 6 [M].** Expanding positions and stopping wherever a split appears finished on 3 of them. In moves, its best run was 0.87x, 1.14x and 1.46x of the plain count. In positions it was worse than plain, 0.51x to 0.72x. The leaves' blocks are as big as what they replace. The 726,019 deal timed out at 45 s, and the two largest weren't tried. So the 6 need a different idea, such as a compact invariant for the endgame. Nothing here suggests one exists.

## 3. Scaled to about 1,040 losses

| | Split | Not split |
|---|---|---|
| Share of losses | 74% [M], 54-88% (95% range) | 26% |
| Deals [E] | About 770 (560-910) | About 270 (130-480) |
| Moves per deal | Mean 2,606, max 11,965 [M] | Mean 22.5M, max 111M [M] |
| Kernel per deal at 1-10 ms a move [E] | 3-26 s, largest 12-120 s | Largest seen 31-310 hours |
| Kernel, all of them [E] | **0.6-6 core-hours** | 70-700 core-days, and the giants likely run out of memory |

The earlier estimate for all losses by kernel was 3 months to 3 years [E]; the unsplit deals still carry nearly all of it. Extra Lean work is one lemma, proved once: the splitting theorem above, about 150-300 lines [E].

## 4. Does it change the recommendation?

**Partly.** Kernel-only is now plausible for the roughly 770 jammed losses, which would give a kernel-checked theorem for about three quarters of them in hours of CPU. Kernel-only for all 1,040 remains implausible: the near-win losses are 26% of the deals and 92% of the moves, and splitting does nothing for them. They still need `native_decide` or a compiled checker. A kernel pass is reasonable only on the smallest unsplit ones: 224k and 569k moves [M], about 4 minutes to 1.5 hours [E].

The plan otherwise stands, with one addition: prove the splitting lemma at stage 1 and run jammed losses by regions. It wins no extra deals and adds no speed.
