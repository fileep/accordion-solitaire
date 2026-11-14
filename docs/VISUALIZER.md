# The visualizer

A web page for playing Accordion by hand, having the solver win a deal from wherever you are, and replaying solve files
move by move. It runs in your browser on your own computer; the solver in it is acc's own, compiled to WebAssembly.

These steps weren't run while this was written, as they download packages; they are the project's usual ones. CI
installs the packages (`npm ci`) and passes the visualizer's tests (`npm test`) on Linux on every push.

## Starting it

You need, once:

1. **Rust**, as for acc ([GETTING_STARTED.md](GETTING_STARTED.md#2-install-rust)), and its WebAssembly target:

   ```
   rustup target add wasm32-unknown-unknown
   ```

2. **Node.js 22.18 or later**, from [nodejs.org](https://nodejs.org) (the LTS version). Check it in a new terminal with
   `node --version`, which should print `v22.18.0` or higher.

Then, from the project's folder, one line at a time:

```
cd viz
npm install
npm run dev
```

In Windows PowerShell, type `npm.cmd` for `npm` (`npm.cmd install`, then `npm.cmd run dev`). Plain `npm` there runs
`npm.ps1`, a script, which Windows' default settings refuse with `npm.ps1 cannot be loaded because running scripts is
disabled on this system`; `npm.cmd` is the same program without that. In Command Prompt, plain `npm` works.

`npm install` fetches what the page is built with (Vite, TypeScript and wasm-pack) into `viz/node_modules`; it needs the
internet the first time. `npm run dev` compiles the solver to WebAssembly, then starts a small web server on your computer
and prints its address, normally `http://localhost:5173/`. Open that address in a browser. The page works while the terminal
stays open; Ctrl+C in the terminal stops it. Next time, `cd viz` and `npm run dev` (in PowerShell, `npm.cmd run dev`) are
enough.

## Playing

The **Play** tab shows a deal as a row of piles.

- **New random deal** deals one at random. Type a number in **Seed** and press Enter for that deal: the same deal as
  `acc show --seed` gives. **Enter a deck…** takes 52 cards typed or pasted (as for `acc deal --cards`), or picked in order.
- To move, click a pile, then one of the highlighted piles it can go onto (one or three to its left, with a top card of
  the same suit or rank). Dragging a pile onto its target works too, and so do Tab and Enter from the keyboard.
- **Undo** (Ctrl+Z), **Redo** (Ctrl+Y) and **Restart** go back and forth; the list of moves can be clicked to jump to one.
- **Solve & replay** has the solver find a winning line from the position on the screen, checks it, and replays it in the
  Replay tab. It searches up to 300 million positions, which takes about half a minute at worst; **Cancel** stops it. If
  no win is left from where you are, it says so and solves from the start of the deal instead.
- **Export** downloads the game so far as a solve file, which `acc verify` checks and `acc show` shows.
- When the game ends, a box offers **Undo**, **Restart**, **Show a solution** and **New random deal**.

## Replaying a solve file

The **Replay a solve file** tab opens a solve file ([FORMATS.md](FORMATS.md#solve-files)): choose one with **Open file**,
or paste its text and press **Open pasted text**. A file of many lines (`.jsonl`) gives a list to choose from. Each line is
checked under the rules as it is opened, and marked ✓ or ✗.

The buttons go to the start, a move back, play or pause, a move forward and the end (also Home, ←, Space, → and End), with a
speed setting. **Continue from here** takes the position into the Play tab to go on by hand.

Files worth opening:

- the hardest deals of the project's runs, in [hard-deals/gallery/](../hard-deals/gallery/), listed in
  [hard-deals/INDEX.md](../hard-deals/INDEX.md);
- a deal's line from `acc solve --seed N --out win.json`, or every line of a run from `acc run --save-wins wins.jsonl`;
- the loss records in `hard-deals/proofs/` or `output/hard-deals/proofs/`, which hold the longest line found in a deal that
  can't be won.

A dossier (`seed-<n>.json` in `hard-deals/`) is not a solve file; its best line is in its `best` field, and `acc show` on it
shows its deal.

## Building it as files

`npm run build` writes the page as plain files into `viz/dist`, which any web server can serve, so that others can use it
without installing anything. That hasn't been done for this project yet.
