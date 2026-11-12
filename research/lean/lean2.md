# Lean proofs of the lost deals: second study

> Working notes from a study of 2026-11-09, kept as written. [E] figures were estimates made before Lean was
> installed; [README.md](README.md) has what was built and measured since.

[M] = measured here. [E] = my estimate. Lean was not installed or run, so every Lean figure is [E] and `Accordion.lean` has never been compiled.

**Answer.** The near-wins are not big proofs. Their reachable set fits in a tiny automaton: the 19.4M-position, 111M-move deal becomes 1,135 nodes, and checking it takes 41,651 lookups [M]. That overturns the earlier "near-wins need `native_decide`": kernel-only looks possible for every loss, about 2-36 core-hours in all [E], down from 70-700 core-days. The splitting theorem is no longer needed.

## 1. What makes the near-wins lost

| Seed [M] | Positions | Moves | Best piles | Positions that can reach best | Final positions | Automaton nodes (shrink) | Check lookups |
|---|---|---|---|---|---|---|---|
| 12929752010 | 79,426 | 223,841 | 2 | 2.7% | 45 | 2,689 (30x) | 76,871 |
| 18238151902 | 168,098 | 568,832 | 3 | 0.25% | 13 | 2,413 (70x) | 84,220 |
| 7615968364 | 726,019 | 2.55M | 2 | 0.7% | 30 | 8,772 (83x) | 394,642 |
| 5895817790 | 957,502 | 4.36M | 3 | 0.05% | 4 | 1,734 (552x) | 79,130 |
| 1005616798 | 3,405,471 | 16.1M | 2 | 0.04% | 6 | 9,464 (360x) | 762,846 |
| 112249515 | 19,386,305 | 111M | 2 | 0.09% | 1 | 1,135 (17,081x) | 41,651 |

- **A jam with a narrow escape.** 67-99.8% of positions still have 20 or more piles, and only 0.006-15% are under 10 [M]. The size is local choices multiplying, not endgames.
- **Hourglass.** A thin corridor gets deep: on the layers sampled, only 3-5 positions per layer can still reach the best from 33 piles down to 9 (seed 1005616798), 5 per layer from 39 to 27 (seed 5895817790), and 1 to 70 per layer from 33 piles down (the 19.4M deal) [M].
- **No single obstruction.** Lines die at nearly every pile count; dead ends are 0.4-6.8% of positions [M]. Final positions number 1 to 45, and no fixed pair of cards survives everywhere. The first pile is almost always one fixed card until about 10 piles remain, in 5 of the 6 [M]. Nothing suggests a small card-based invariant.

**Abstractions over positions barely merge anything** [M, the two deals small enough for all tests; 0 conflicts in fewest piles or move count]:

| Candidate | 79k deal | 168k deal |
|---|---|---|
| Erase a rank or suit no other visible card shares | 66,132 (1.20x) | 167,571 (1.003x) |
| Same, renamed by first appearance | 65,766 (1.21x) | 167,526 (1.003x) |
| Pattern of matches only | 65,634 (1.21x) | 167,518 (1.003x) |
| Orphan-card lemma (positions kept) | 71,767 (1.11x) | 167,869 (1.001x) |
| **One automaton (nodes)** | **2,689 (30x)** | **2,413 (70x)** |

Positions differ in independent local details that do matter, so they are not equivalent; but they share structure.

## 2. The compact proof: one automaton

Treat a position as the word of its top cards. The set of all reachable positions becomes a minimal deterministic automaton, read left to right. Because a move is a local rewrite of that word, its image over the whole set is a short memoised recursion (`dawg.py`, about 40 lines of logic).

**The proof is three checks.** The deal's row is accepted; no one-card word is accepted; and at every node, a slide or leap with target letter `a` lands back inside what that node already allows. The relation is the identity on nodes (pairs checked = nodes, 23 of 23 deals [M]), so there is no search, only one pass over the nodes plus a table of inclusion pairs (1-5 per node). Nodes, not positions, set the cost.

Results for all 23 losses [M]: 26.9M positions and 145.9M moves become 29,171 nodes and 1.48M lookups; each deal takes 0-6 s in Python at idle. The largest near-win by positions has the smallest automaton of the six, so size does not track position count. Jammed deals: 55-517 nodes, smaller than their region proof on 14 of 17 and within 13% on the rest. On the near-wins, one automaton has 7-28x fewer nodes than one per layer.

**Tested, none failed [M]:**
- Position and move counts equal the repo's records on 23 of 23, including the 19.4M deal the earlier study did not enumerate.
- On 15 deals, the automaton's words equal an independent enumeration, and every move of every word stays in the set.
- On each of the 6 near-wins, 20,000 random words: all 56k-115k moves stay in the set.
- Mutation: deleting one word was rejected 180 of 180 times. Dropping one edge was rejected 136 of 136 when the deal's word stayed in the set (44 more lost the deal's word or were empty, so they prove nothing).
- 150 random sub-rows of 12-19 cards, compared with explicit search, including the 49 with a win: 150 agree.
- A real won deal is not covered: my test stopped at 34 piles (peak layer 238,723 nodes), so no claim about wins.

## 3. Lean draft

`Accordion.lean` holds the rules on piles, the reduction to top cards, the generator, the invariant theorem, the automaton checker, and the splitting theorem with `canSit`, the left-recursion that covers the leap over a short block. Proofs are mostly outlines with `sorry`. Least sure of:
- It has never been compiled; expect fixes.
- `closed_of_noCrossing` is left open. The slot arithmetic across block boundaries is the hard induction, and the short-block leap lives there.
- The kernel's deep recursion and memory on a big `all`-style check.

Since the automaton covers jammed deals too, I would prove the splitting theorem only for its explanatory value.

## 4. Trust

| Route | What you must trust, beyond the rules and generator text | Who else can check it | All losses [E] |
|---|---|---|---|
| Kernel, automaton certificate | Lean's kernel and GMP only | Lean's replay checker, and other kernels (`nanoda`, `lean4lean`) | 2-36 core-hours |
| `native_decide` | Also the compiler, C toolchain, runtime; recorded as an axiom | Nobody outside Lean | Minutes |
| Separate program | Its language and compiler; gives no theorem | Rerun or rewrite it, in any language | Minutes |
| Kernel, explicit enumeration | Lean's kernel and GMP only | As the first row | 70-700 core-days |

The first row is the middle route asked for. Cost [E]: one lookup is a table fetch, an edge scan and a pair test, about 100-200 kernel reductions at 1-10 us, built on the kernel's fast `Nat` operations (add, mul, div, mod, land, lor, xor, shifts, beq, ble). That is 0.1-2 ms a lookup, so:

| | Lookups [M] | Kernel [E] |
|---|---|---|
| The 19.4M deal | 41,651 | 4-80 s |
| Worst of the six | 762,846 | 1-25 min |
| About 1,040 losses, 26% near-wins at the mean of 240k | About 65M | 2-36 core-hours |

Packing explicit positions into `Nat`s without the automaton is only about 10x better than the old plan, around 3-38 hours for the 19.4M deal [E]. To keep the kernel's memory down, split each large deal into chunks of about 100k lookups, one theorem each, since caches are per declaration [E].

Also recheck every certificate in a second language. It costs almost nothing and catches any bug in one checker.

Remaining risks: a misread rule still passes through everything, and all kernel timing is unmeasured.

## 5. What next

**For the run, nothing changes.** Only the seeds matter, and `acc` already writes them in `hard-deals/proofs/`. The solver needs to save nothing extra, because a certificate rebuilds from a seed in seconds. The prove budget defaults to 10 billion moves per deal, about 90x the largest seen. If a giant ever exceeds it, the automaton can settle it offline with little memory. (The trillion-deal run that followed had 721 losses.)

**After the run:**
1. Stage 0 in Lean: rules, generator tested on the 23 seed/card pairs, checker, and seeds 3,417,058,778 and 112,249,515. The second is a 19.4M-position loss with a certificate of about 6K edges. This replaces my timing guesses with real ones; a day or two [E].
2. Generate certificates for all losses with `dawg.py`: the 23 took about 12 s [M], so about 10-20 minutes in Python [E]. Recheck each in a second language.
3. Kernel-check all of them. If a giant's automaton is too big (nothing near that yet; only 6 near-wins seen, so the tail is unknown), use `native_decide` for that deal alone.

**Changed from the earlier studies:** a certificate can be far cheaper than re-enumerating; "nothing suggests a compact invariant" was wrong; and no giant-loss memory plan is needed.

Files: the study's scripts and logs are in [automaton/](automaton/), where `Accordion.lean` has since grown into the checker.
