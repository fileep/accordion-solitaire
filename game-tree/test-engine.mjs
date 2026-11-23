// Checks the game tree's engine against the project's records: for lost deals small enough to search whole, the number of
// positions must equal the proof's (research/lean/automaton/certificates.csv), the number of moves the plain search's
// count (hard-deals/proofs/seed-*-loss.json), and the fewest piles reached the record's. Then it checks that a hard
// deal's known winning line plays and wins, and that a big deal's sampled tree is consistent.
//
//   node game-tree/test-engine.mjs [most positions, default 700000]
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..");
const html = readFileSync(join(here, "game-tree.html"), "utf8");
const part = (tag) => {
  const m = html.match(new RegExp(`// <${tag}>([\\s\\S]*?)// </${tag}>`));
  if (!m) throw new Error(`no <${tag}> section in game-tree.html`);
  return m[1];
};
const lib = new Function(`${part("deal")}\n${part("engine")}\nreturn { dealFromSeed, workerMain };`)();

function build(q) {
  let out = null;
  const self = { postMessage(m) { if (m.type !== "progress") out = m; } };
  lib.workerMain(self);
  self.onmessage({ data: { cmd: "build", ...q } });
  if (!out || out.type !== "graph") throw new Error(`build failed: ${out && out.message}`);
  return out;
}

const max = Number(process.argv[2] || 700000);
const certs = readFileSync(join(repo, "research/lean/automaton/certificates.csv"), "utf8").trim().split(/\r?\n/).slice(1)
  .map((l) => l.split(",")).map((r) => ({ seed: r[0], positions: Number(r[1]) }))
  .filter((r) => r.positions <= max).sort((a, b) => a.positions - b.positions);
// Every size class: the smallest ten, then about one in fifteen of the rest.
const chosen = certs.filter((r, i) => i < 10 || i % 15 === 0 || i === certs.length - 1);
let failures = 0;
const fail = (msg) => { failures++; console.log(`FAIL ${msg}`); };

// The page's list of the 721 losses, with their numbers of positions, must be the proofs' list.
const listed = new Map(html.match(/var LOSSES='([^']*)'/)[1].split(",").map((p) => p.split(".").map((x) => parseInt(x, 36)))
  .map(([s, n]) => [String(s), n]));
const allCerts = readFileSync(join(repo, "research/lean/automaton/certificates.csv"), "utf8").trim().split(/\r?\n/).slice(1).map((l) => l.split(","));
if (listed.size !== 721 || allCerts.length !== 721 || allCerts.some((r) => listed.get(r[0]) !== Number(r[1])))
  fail(`the page's list of losses differs from certificates.csv (${listed.size} listed)`);
else console.log("ok   the page lists the 721 losses with their numbers of positions");

const index = readFileSync(join(repo, "hard-deals/INDEX.md"), "utf8");
for (const r of chosen) {
  const t = Date.now();
  // The page tells the engine the size of a known loss, as here; a tree of unknown size is only tried whole up to 4 million.
  const g = build({ cards: lib.dealFromSeed(r.seed), budget: r.positions + 10, top: 0.15, endPiles: 15, total: r.positions });
  const proof = JSON.parse(readFileSync(join(repo, `hard-deals/proofs/seed-${r.seed}-loss.json`), "utf8"));
  const moves = proof.stats.proof.plain.positions;
  const row = index.split("\n").find((l) => l.startsWith(`| [seed-${r.seed}]`));
  const best = row ? Number(row.split("|")[4]) : NaN;
  const ok = g.complete && g.n === r.positions && g.status[0] === 2 && g.counts.unknown === 0;
  const movesOk = g.ne === moves || g.ne + 1 === moves;
  const bestOk = g.best[0] === best;
  console.log(`${ok && movesOk && bestOk ? "ok  " : "FAIL"} seed ${r.seed}: ${g.n} positions (record ${r.positions}), ${g.ne} moves (plain search ${moves}), ` +
    `best ${g.best[0]} piles (record ${best}), ${Date.now() - t} ms`);
  if (!(ok && movesOk && bestOk)) failures++;
}

// A hard deal: the known winning line must play legally and end in one pile, which must show as a win from the deal.
const RANKS = "A23456789TJQK", SUITS = "CDHS";
const byte = (s) => (SUITS.indexOf(s[1]) << 4) | RANKS.indexOf(s[0]);
const hardSeed = "56669828180";
const dossier = JSON.parse(readFileSync(join(repo, `hard-deals/seed-${hardSeed}.json`), "utf8"));
const cards = lib.dealFromSeed(hardSeed);
if (cards.map((b) => RANKS[b & 15] + SUITS[b >> 4]).join(" ") !== dossier.deal.cards) fail(`seed ${hardSeed} deals different cards from its dossier`);
const tops = cards.slice(), line = [];
for (const mv of dossier.solution) {
  const a = byte(mv.slice(0, 2)), b = byte(mv.slice(3, 5)), i = tops.indexOf(a), t = tops.indexOf(b);
  if (i - t !== 1 && i - t !== 3) { fail(`bad move ${mv}`); break; }
  line.push((i << 1) | (i - t === 3 ? 1 : 0)); tops[t] = a; tops.splice(i, 1);
}
let t = Date.now();
const h = build({ cards, budget: 200000, top: 0.15, endPiles: 15, line, total: 1e18, rng: 7 });
// A small lost deal built as if it were huge (as a typed seed once was): it is still found whole, with its moves.
{ const g = build({ cards: lib.dealFromSeed("587527261691"), budget: 1000000, top: 0.15, endPiles: 15, total: 1e18 });
  if (!g.complete || g.n !== 1512 || g.ne !== 5940 || g.best[0] !== 39) fail(`seed 587527261691 built as huge: ${g.n} positions, ${g.ne} moves, best ${g.best[0]}`);
  else console.log("ok   seed 587527261691 built as if huge: still whole, 1512 positions, 5940 moves, best 39"); }
const lp = h.linePath;
if (!lp || lp.length !== 52 || h.len[lp[51]] !== 1) fail("the winning line did not play to one pile");
else if (h.status[0] !== 1) fail("the deal does not show as winnable with its winning line drawn");
else console.log(`ok   seed ${hardSeed}: winning line drawn, ${h.n} positions, ${h.counts.won} winning positions, ${h.dives} dives (${h.divesWon} won), ${Date.now() - t} ms`);

// A game being played: its moves must be drawn, each joined to the next by a move, with every position a few moves
// below where it has got to.
{
  const cards185 = lib.dealFromSeed("185"), row = cards185.slice(), played = [];
  for (let s = 0; s < 20; s++) { // the first legal move each time, leaps first
    let m = -1;
    for (let i = 1; i < row.length && m < 0; i++) for (const d of [3, 1]) {
      if (i < d || m >= 0) continue; const c = row[i], t = row[i - d];
      if ((c >> 4) === (t >> 4) || (c & 15) === (t & 15)) m = (i << 1) | (d === 3 ? 1 : 0);
    }
    if (m < 0) break; played.push(m); const i = m >> 1; row[i - (m & 1 ? 3 : 1)] = row[i]; row.splice(i, 1);
  }
  const t1 = Date.now(), g = build({ cards: cards185, budget: 200000, top: 0.15, endPiles: 15, total: 1e18, play: played, rng: 3 });
  const pp = g.playPath, edge = (e) => g.et[e >>> 22][e & 4194303];
  let okPath = pp && pp.length === played.length + 1;
  for (let k = 0; okPath && k < played.length; k++) {
    let joined = false; for (let e = g.es[pp[k]]; e < g.es[pp[k] + 1]; e++) if (edge(e) === pp[k + 1]) joined = true;
    if (!joined || g.len[pp[k + 1]] !== g.len[pp[k]] - 1) okPath = false;
  }
  const rowAt = (u) => { const mv = []; for (let v = u; v > 0; v = g.parent[v]) mv.push(g.pmove[v]); const r = g.root.slice();
    for (const m of mv.reverse()) { const i = m >> 1; r[i - (m & 1 ? 3 : 1)] = r[i]; r.splice(i, 1); } return r; };
  const end = pp && pp[pp.length - 1], tops = end != null ? rowAt(end) : [];
  if (!okPath) fail("the moves played are not drawn as a line");
  else if (tops.join() !== row.join()) fail("the position played to has the wrong cards");
  else if (g.es[end + 1] - g.es[end] < 1) fail("nothing is drawn below the position played to");
  else console.log(`ok   seed 185 played ${played.length} moves: drawn as a line, ${g.es[end + 1] - g.es[end]} moves drawn from where it got to, ${g.n} positions, ${Date.now() - t1} ms`);
}

// A sampled tree: statuses must agree with the moves drawn.
t = Date.now();
const s = build({ cards: lib.dealFromSeed("0"), budget: 300000, top: 0.15, endPiles: 15, rng: 1 });
let bad = 0;
for (let u = 0; u < s.n; u++) {
  let anyWin = false, allDead = true, deg = 0;
  for (let e = s.es[u]; e < s.es[u + 1]; e++) { const c = s.et[e >>> 22][e & 4194303]; deg++; if (s.status[c] === 1) anyWin = true; if (s.status[c] !== 2) allDead = false;
    if (s.len[c] !== s.len[u] - 1) bad++; }
  const want = s.len[u] === 1 ? 1 : anyWin ? 1 : ((s.expanded[u] & 1) && allDead) || (s.expanded[u] & 2) ? 2 : 0;
  if (s.status[u] !== want) bad++;
  if ((s.expanded[u] & 1) && deg === 0 && s.len[u] > 1 && s.status[u] !== 2) bad++;
}
// "Exact" (8 in expanded) promises that everything below is drawn, so the fewest piles shown is the true one. Check it
// with a separate exhaustive search (its own move generator, on strings) for exact positions of up to 16 piles, and
// that a position marked lost without being exact really cannot win.
const bestMemo = new Map();
function trueBest(tops) {
  if (tops.length === 1) return 1;
  const key = tops.join(","); if (bestMemo.has(key)) return bestMemo.get(key);
  let b = tops.length;
  for (let i = 1; i < tops.length; i++) for (const d of [1, 3]) {
    if (i < d) continue; const c = tops[i], t = tops[i - d];
    if ((c >> 4) !== (t >> 4) && (c & 15) !== (t & 15)) continue;
    const next = tops.slice(); next[i - d] = c; next.splice(i, 1);
    b = Math.min(b, trueBest(next)); if (b === 1) break;
  }
  bestMemo.set(key, b); return b;
}
// A position's row of top cards, played again from the deal along the moves that first reached it.
const topsOf = (g, u) => {
  const moves = [];
  for (let v = u; v > 0; v = g.parent[v]) moves.push(g.pmove[v]);
  const row = g.root.slice();
  for (const m of moves.reverse()) { const i = m >> 1; row[i - (m & 1 ? 3 : 1)] = row[i]; row.splice(i, 1); }
  return row;
};
let exactChecked = 0, lostChecked = 0;
for (let u = 0; u < s.n && (exactChecked < 60 || lostChecked < 60); u++) {
  if (s.len[u] > 16 || s.len[u] < 2) continue;
  if ((s.expanded[u] & 8) && exactChecked < 60) { exactChecked++; if (trueBest(topsOf(s, u)) !== s.best[u]) bad++; }
  else if (s.status[u] === 2 && lostChecked < 60) { lostChecked++; if (trueBest(topsOf(s, u)) === 1) bad++; }
}
if (bad) fail(`seed 0 sampled tree: ${bad} inconsistencies`);
else console.log(`ok   seed 0 sampled (and ${exactChecked} exact, ${lostChecked} lost positions checked by a separate search): ${s.n} positions, ${s.ne} moves, ${s.maxDepth} deep, ${s.dives} dives (${s.divesWon} won), ${s.counts.won} winning positions, ${Date.now() - t} ms`);

console.log(failures ? `${failures} failed` : "all passed");
process.exit(failures ? 1 : 0);
