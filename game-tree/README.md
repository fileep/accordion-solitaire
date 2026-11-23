# Game trees

`game-tree.html` draws the game tree of any deal in 3D, in the browser, and lets you explore it: each dot is a position
and each line a move, and a position reached by different orders of moves is drawn once. It is one file with nothing to
install, and the part between `<!-- acc-tree` and `<!-- end of acc-tree -->` can be pasted into a blog post as it is.

![The easy example, seed 185, seen from above](screenshots/easy-seed-185-top.jpg)

## Opening it

Open `game-tree.html` in Chrome, Edge or Firefox. If the browser won't run it from a file, serve the folder:

```
python game-tree/serve.py
```

and open `http://127.0.0.1:8781/game-tree.html`. The server listens on this computer only; it also saves pictures sent
to `/save/<name>.png` into `screenshots/full/`, which is how the pictures here were made.

The address can choose what to show: `?ex=l202346720` (an example, by the key in the page's list), or `?seed=123`, with
`&size=1000000` (any number of positions from 1,000 to 40 million, such as `2.5m`), `&view=top` (`3d`, `top`, `side`),
`&shape=spindle`, `&color=best` (`outcome`, `best`, `depth`) and `&edges=all` (`tree`, `all`, `none`).

## What it shows

- **Colours**: green positions can still win and gold ones are won (one pile). Orange to dark red ones are lost with
  everything below them drawn: orange gets as close as the deal allows (2 piles if it can be won, else the deal's own
  best), dark red falls further short. Red ones are lost without the fewest piles worked out (the short searches below
  skip moves the dead-end test rules out), and blue-grey ones aren't settled either way. The positions that can win, and
  the moves between them, are drawn again on top so that the few winning lines in a hard deal stand out.
- **Shape: rings** puts the deal at the centre (the top, in 3D) and each move one ring further out, each position near the
  positions it comes from. **Shape: spindle** places each position by which cards are covered: each covered card pushes it
  towards that card's place in the deal, so moves that don't interfere draw as parallelograms.
- **Small trees are complete.** The lost deals of the trillion run have between 2 and 22,066,800 positions; a tree that
  fits the chosen size (200,000, 1 million, 4 million, 40 million, or a number typed in) is drawn with every position and
  every move, and its counts are exact. Of the 721, 701 fit in 4 million and all of them in 40 million: the biggest,
  22,066,800 positions joined by 134,505,640 moves, takes about 45 seconds and 1.5 GB.
- **How 40 million fit.** A position keeps only the move that first reached it, the card that move covered, a few flags
  and two 32-bit hashes of its row of top cards; its cards are played again from the deal when they are needed. Two
  positions of the same size share both hashes with odds of 1 in 2<sup>64</sup>, so even among 40 million the odds that
  any two are mixed up are under 1 in 10,000. A sample of 40 million positions of a deal that can be won takes about a
  minute and a half and 2 GB.
- **Known deals.** The page knows the 721 lost deals of seeds 0 to 999,999,999,999 and their numbers of positions, so
  every other seed in that range is known to be winnable and goes straight to a sample, and a lost one is drawn whole
  when it fits.
- **Big trees are samples.** A deal that can be won usually has something like 10<sup>17</sup> positions. Then the first
  15% of the size is every position of the first few moves, and the rest comes from short searches like the solver's:
  depth first, a few thousand positions each, moves in a random order that favours leaps, never into a position the
  solver's dead-end test shows lost. Each group of searches starts from the deal, then from positions it reached, so they
  branch like a tree. In an easy deal most of them find a win; in a hard one almost none do.
- **Click a dot** to see its position as cards, the way in from the deal (white) and the best line from it (gold to a
  win, red to the fewest piles drawn). **Explore from here** draws the tree from that position, which is complete once
  it is small enough: from the 20-pile position on seed 185's way to a win, for example, all 89,801 positions, where
  4,974,472 of the 1,198,856,684 ways to play on win.
- **In a blog post** the first tree is drawn when the block comes near the screen. One finger scrolls the page up and
  down, two fingers turn and zoom the tree, and the wheel scrolls the page until the tree is clicked (or with Ctrl held).
  On its own page, or full screen, every touch and the wheel drive the tree.

## The examples

- **Easy to win**: seed 185, the one of the first 300 deals whose short searches win most often (about 9 in 10 at 1
  million positions), and seed 0, the first deal.
- **Hard to win**: seed 3, the one of the first 300 whose short searches win least often (about 1 in 5 at 1 million
  positions), and the five deals of the
  trillion run that only the exhaustive search won (after 9,983 failed tries each), with their known winning lines drawn
  in.
- **Lost**: eleven of the 721 proven losses, from the one with a single legal move (2 positions) to the one with the most
  positions (22,066,800, drawn whole at the 40 million size).

## Checking it

```
node game-tree/test-engine.mjs
```

runs the page's engine on the 51 lost deals of every size up to 700,000 positions and checks, for each, that it finds as
many positions as the proof (`research/lean/automaton/certificates.csv`), as many moves as the plain proof search
(`hard-deals/proofs/`), and the same fewest piles as the record (`hard-deals/INDEX.md`); then that the page's list of
losses is the proofs' list, that a hard deal's winning line plays and wins, and that a sampled tree's colours agree with
its moves, with a separate exhaustive search confirming the fewest piles of positions marked exact and that positions
marked lost can't win. All pass. `node game-tree/test-engine.mjs 23000000` adds bigger losses, up to the biggest (it
matches its proof's 22,066,800 positions, 134,505,640 moves and 22 piles; a few minutes and 2 GB).

## Pictures

`screenshots/` has stills of the examples, saved from the page through `serve.py` (with `host.accTree.save`, which draws
the tree at twice the size of the screen) and made into 1,600-pixel JPEGs. The full-size originals (2,304 by 1,728) stay
in `screenshots/full/`, which isn't committed.
