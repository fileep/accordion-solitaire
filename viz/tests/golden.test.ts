// Golden tests for the engine as the visualizer sees it (WebAssembly), from the same testdata/ files the Rust
// tests use (PLAN.md §6.4). Run with `npm test`, which rebuilds the WASM package first.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  initSync,
  dealFromSeed,
  exportRecord,
  openSolveFile,
  parseDeck,
  replay,
  verifySolveFile,
} from '../src/wasm/accordion.js';

initSync({ module: readFileSync(new URL('../src/wasm/accordion_bg.wasm', import.meta.url)) });

const testdata = (path: string) => readFileSync(new URL(`../../testdata/${path}`, import.meta.url), 'utf8');

// Seeds go up to 2^64 - 1, beyond a JS number, so read them from the JSON source text as BigInt.
const parseWithBigSeeds = (text: string) =>
  JSON.parse(text, (key, value, context?: { source: string }) =>
    key === 'seed' && context ? BigInt(context.source) : value,
  );

test('seeded deals match the spec, including seeds beyond 2^53', () => {
  const { deals } = parseWithBigSeeds(testdata('deals_by_seed.json'));
  assert.ok(deals.some((d: { seed: bigint }) => d.seed > BigInt(Number.MAX_SAFE_INTEGER)));
  for (const d of deals) assert.equal(dealFromSeed(d.seed), d.cards, `seed ${d.seed}`);
});

test('every golden game verifies exactly as the manifest expects', () => {
  const { games } = JSON.parse(testdata('games/manifest.json'));
  let records = 0;
  for (const game of games) {
    const results = JSON.parse(verifySolveFile(testdata(`games/${game.file}`)));
    assert.equal(results.length, game.expect.length, `${game.file}: record count`);
    results.forEach((r: Record<string, unknown>, i: number) => {
      const e = game.expect[i];
      const label = `${game.file} line ${r.line}`;
      assert.equal(r.ok, e.ok, `${label}: ${r.error ?? ''}`);
      if (e.ok) {
        assert.deepEqual([r.result, r.moves, r.piles_left], [e.result, e.moves, e.piles_left], label);
      } else {
        assert.equal(r.move ?? null, e.move ?? null, label);
      }
      records++;
    });
  }
  assert.ok(records > 300, `${records} records`);
});

test('deck input matches the spec', () => {
  const { decks } = JSON.parse(testdata('deck_input.json'));
  for (const c of decks) {
    const r = JSON.parse(parseDeck(c.input));
    if (c.ok) assert.equal(r.ok, c.ok, c.name);
    else assert.deepEqual(r.issues, c.issues, c.name);
  }
});

test('replay stops before an illegal move and names it', () => {
  const [entry] = JSON.parse(openSolveFile(testdata('games/bad_wrong_distance.json')));
  assert.equal(entry.verified, false);
  assert.equal(entry.errorMove, 11);
  assert.equal(entry.moves.length, 10, 'only the legal prefix is replayable');
  const bad = JSON.parse(replay(entry.cards, Uint8Array.from([...entry.moves, 0])));
  assert.equal(bad.move, 11);
});

test('a game exported from the page verifies as a win', () => {
  const [entry] = JSON.parse(openSolveFile(testdata('games/seed_42_win.json')));
  assert.equal(entry.verified, true);
  const state = JSON.parse(replay(entry.cards, Uint8Array.from(entry.moves))).ok;
  assert.equal(state.won, true);
  const exported = JSON.parse(exportRecord(entry.cards, 42n, Uint8Array.from(entry.moves)));
  const [check] = JSON.parse(verifySolveFile(exported.ok));
  assert.deepEqual([check.ok, check.result, check.moves], [true, 'win', 51]);
  const partial = JSON.parse(exportRecord(entry.cards, undefined, Uint8Array.from(entry.moves.slice(0, 20))));
  assert.equal(JSON.parse(verifySolveFile(partial.ok))[0].result, 'unknown');
});
