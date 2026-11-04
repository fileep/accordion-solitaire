// Accordion visualizer (PLAN.md Phase 3): play a deal by hand with undo, enter your own deck, and replay solve
// files with a slider and speeds. One deal at a time; every rule comes from the Rust engine via WebAssembly.
import './style.css';
import type { SolveResponse } from './solver-worker';
import {
  type Card,
  type LegalMove,
  type ReplayEntry,
  type State,
  cardFace,
  cardLabel,
  checkDeck,
  dealFromSeed,
  exportRecord,
  loadEngine,
  openSolveFile,
  stateAfter,
} from './engine';

type Mode = 'play' | 'replay';

/** A line of play on one deal: `moves[0..cursor]` are on the board, the rest can be redone (or replayed). */
interface Line {
  cards: string;
  seed?: bigint;
  label: string;
  moves: number[];
  texts: string[];
  cursor: number;
}

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const board = $('board');
const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');

let mode: Mode = 'play';
let playLine: Line;
let replayLine: Line | null = null;
let replayEntries: ReplayEntry[] = [];
let replayEntry: ReplayEntry | null = null;
let line: Line | null = null;
let state: State | null = null;
let selected: number | null = null;
let timer: number | undefined;

// ---- deals ---------------------------------------------------------------------------------------------------

function randomSeed(): bigint {
  const words = crypto.getRandomValues(new Uint32Array(2));
  return (BigInt(words[0]) << 32n) | BigInt(words[1]);
}

function newLine(cards: string, label: string, seed?: bigint): Line {
  return { cards, seed, label, moves: [], texts: [], cursor: 0 };
}

function playSeed(seed: bigint) {
  playLine = newLine(dealFromSeed(seed), `seed ${seed}`, seed);
  note('');
  showMode('play');
}

// ---- rendering -----------------------------------------------------------------------------------------------

const movesFrom = (i: number): LegalMove[] => state?.legal.filter((m) => m.pile === i) ?? [];

function targetsOf(i: number | null): Map<number, LegalMove> {
  const map = new Map<number, LegalMove>();
  if (i === null || mode !== 'play') return map;
  for (const m of movesFrom(i)) map.set(m.pile - (m.leap ? 3 : 1), m);
  return map;
}

function cardElement(card: Card): HTMLElement {
  const f = cardFace(card);
  const el = document.createElement('div');
  el.className = `card ${f.red ? 'red' : 'black'}`;
  el.innerHTML = `<span class="rank">${f.rank}</span><span class="suit">${f.suit}</span>`;
  return el;
}

function pileElement(pile: Card[], i: number): HTMLElement {
  const top = pile[pile.length - 1];
  const el = document.createElement('div');
  el.className = 'pile';
  el.dataset.index = String(i);
  el.dataset.top = top;
  el.tabIndex = 0;
  el.setAttribute('role', 'button');
  const depth = pile.length > 1 ? `, ${pile.length} cards` : '';
  el.setAttribute('aria-label', `Pile ${i + 1}: ${cardLabel(top)}${depth}`);
  const pos = document.createElement('span');
  pos.className = 'pos';
  pos.textContent = String(i + 1);
  el.append(pos, cardElement(top));
  if (pile.length > 1) {
    const badge = document.createElement('span');
    badge.className = 'depth';
    badge.textContent = String(pile.length);
    el.append(badge);
  }
  return el;
}

/** Hovering or focusing a pile shows its whole stack in the info line above the board. */
function showPileInfo(e: Event) {
  const el = (e.target as HTMLElement).closest<HTMLElement>('.pile');
  const pile = el && state?.piles[Number(el.dataset.index)];
  $('pile-info').textContent = pile
    ? `Pile ${Number(el.dataset.index) + 1}: ${pile.length} card${pile.length === 1 ? '' : 's'}, bottom to top: ${pile.map(cardLabel).join(' ')}`
    : '';
}
board.addEventListener('mouseover', showPileInfo);
board.addEventListener('focusin', showPileInfo);
board.addEventListener('mouseleave', () => ($('pile-info').textContent = ''));

/** Selection and legal-target classes, updated in place (rebuilding during a drag would cancel it). */
function updateHighlights() {
  const targets = targetsOf(selected);
  for (const el of board.querySelectorAll<HTMLElement>('.pile')) {
    const i = Number(el.dataset.index);
    const movable = mode === 'play' && movesFrom(i).length > 0;
    el.classList.toggle('movable', movable);
    el.draggable = movable;
    el.classList.toggle('selected', selected === i);
    el.classList.toggle('target', targets.has(i));
  }
}

/** The move that led to the position on screen, if any. */
function lastMove(): { mover: Card; target: Card; leap: boolean } | null {
  if (!line || line.cursor === 0) return null;
  const [mover, target] = line.texts[line.cursor - 1].split('>');
  return { mover, target, leap: (line.moves[line.cursor - 1] & 1) === 1 };
}

/**
 * Rebuilds the board. When animating a move it plays in two steps, so a reviewer can see exactly what happened:
 * the moving pile, raised and outlined, slides onto its target (a copy of the covered card stays in place until it
 * lands), then the piles to the right slide left to close the gap. The last pile moved keeps an outline either way.
 */
function renderBoard(animate: boolean, duration = 420) {
  const motion = animate && !reduceMotion.matches && duration > 0;
  const moved = lastMove();
  for (const old of board.parentElement!.querySelectorAll('.ghost')) old.remove(); // from a move still fading
  const before = new Map<string, DOMRect>();
  let ghost: HTMLElement | null = null;
  let ghostRect: DOMRect | null = null;
  if (motion) {
    for (const el of board.querySelectorAll<HTMLElement>('.pile')) before.set(el.dataset.top!, el.getBoundingClientRect());
    const covered = moved && board.querySelector<HTMLElement>(`.pile[data-top="${moved.target}"]`);
    if (covered) {
      ghost = covered.cloneNode(true) as HTMLElement;
      ghostRect = covered.getBoundingClientRect();
    }
  }
  if (!line || !state) {
    board.replaceChildren();
    return;
  }
  board.replaceChildren(...state.piles.map(pileElement));
  updateHighlights();
  if (moved) board.querySelector(`.pile[data-top="${moved.mover}"]`)?.classList.add('last-move');
  if (!motion) return;

  const slide = duration * 0.6;
  const close = duration - slide;
  if (ghost && ghostRect) {
    const wrap = board.parentElement!;
    const origin = wrap.getBoundingClientRect();
    ghost.className = 'pile ghost';
    ghost.removeAttribute('tabindex');
    ghost.removeAttribute('role');
    ghost.setAttribute('aria-hidden', 'true');
    ghost.style.left = `${ghostRect.left - origin.left}px`;
    ghost.style.top = `${ghostRect.top - origin.top}px`;
    ghost.style.width = `${ghostRect.width}px`;
    wrap.append(ghost);
    const fade = ghost.animate([{ opacity: 1 }, { opacity: 1, offset: 0.85 }, { opacity: 0 }], { duration: slide + 60 });
    fade.finished.then(
      () => ghost!.remove(),
      () => ghost!.remove(),
    );
  }
  for (const el of board.querySelectorAll<HTMLElement>('.pile')) {
    const old = before.get(el.dataset.top!);
    if (!old) continue;
    const now = el.getBoundingClientRect();
    const from = { transform: `translate(${old.left - now.left}px, ${old.top - now.top}px)` };
    if (el.dataset.top === moved?.mover) {
      el.classList.add('moving');
      const flight = el.animate([from, { transform: 'none' }], { duration: slide, easing: 'ease-in-out' });
      flight.finished.then(
        () => el.classList.remove('moving'),
        () => el.classList.remove('moving'),
      );
    } else if (old.left !== now.left || old.top !== now.top) {
      el.animate([from, { transform: 'none' }], { duration: close, delay: slide, easing: 'ease-out', fill: 'backwards' });
    }
  }
}

function renderHistory() {
  const list = $('history');
  list.replaceChildren(
    ...(line?.texts ?? []).map((text, k) => {
      const li = document.createElement('li');
      const b = document.createElement('button');
      b.textContent = text.replace(/T/g, '10');
      b.className = k < (line?.cursor ?? 0) ? 'played' : 'ahead';
      b.dataset.k = String(k + 1);
      b.title = `Go to the position after move ${k + 1}`;
      li.append(b);
      return li;
    }),
  );
  list.querySelector('.played:last-of-type')?.scrollIntoView({ block: 'nearest' });
}

function renderStatus() {
  const status = $('status');
  const label = $('deal-label');
  if (!line || !state) {
    status.textContent = mode === 'replay' ? 'Open a solve file to replay it.' : '';
    label.textContent = '';
    return;
  }
  const piles = state.piles.length;
  let text = `${piles} pile${piles === 1 ? '' : 's'} left · ${state.legal.length} move${state.legal.length === 1 ? '' : 's'} available`;
  if (state.won) text = `Won! One pile after ${line.cursor} moves.`;
  else if (state.legal.length === 0) text = `No moves left: ${piles} piles. ${mode === 'play' ? 'Undo to try another line.' : ''}`;
  if (mode === 'replay' && replayEntry) {
    const e = replayEntry;
    const verdict = e.verified
      ? `✓ verified ${e.result}, ${e.moves.length} moves`
      : `✗ fails verification, ${e.error}. ${e.errorMove ? 'The replay stops before the bad move.' : ''}`;
    text = `${verdict} · ${text}`;
  }
  status.textContent = text;
  status.classList.toggle('bad', mode === 'replay' && !!replayEntry && !replayEntry.verified);
  label.textContent = `Deal (${line.label}): ${line.cards}`;
  const moved = lastMove();
  $('last-move').textContent = moved
    ? `Move ${line.cursor}: ${cardLabel(moved.mover)} onto ${cardLabel(moved.target)} (${moved.leap ? 'leap, 3 piles left' : 'slide, 1 pile left'})`
    : 'Start of the deal';
}

function renderReplayControls() {
  const max = line?.moves.length ?? 0;
  const slider = $<HTMLInputElement>('r-slider');
  slider.max = String(max);
  slider.value = String(line?.cursor ?? 0);
  $('r-count').textContent = `${line?.cursor ?? 0} / ${max}`;
  $('r-play').textContent = timer === undefined ? '▶' : '⏸';
  $('r-play').setAttribute('aria-label', timer === undefined ? 'Play' : 'Pause');
}

function render(animate = false, duration = 420) {
  state = line ? stateAfter(line.cards, line.moves.slice(0, line.cursor)) : null;
  if (selected !== null && (!state || selected >= state.piles.length)) selected = null;
  renderBoard(animate, duration);
  renderHistory();
  renderStatus();
  renderReplayControls();
  $<HTMLButtonElement>('undo').disabled = !line || line.cursor === 0;
  $<HTMLButtonElement>('redo').disabled = !line || line.cursor >= line.moves.length;
  renderOutcome();
}

// ---- you win / game over ---------------------------------------------------------------------------------------

/** The position the outcome box was hidden on; it stays hidden until the position changes. */
let outcomeHiddenAt: string | null = null;
const positionKey = () => (line ? `${line.cards}|${line.moves.slice(0, line.cursor).join(',')}` : '');

function renderOutcome() {
  const box = $('outcome');
  const over = mode === 'play' && !!state && (state.won || state.legal.length === 0);
  if (!over || outcomeHiddenAt === positionKey()) {
    box.hidden = true;
    return;
  }
  const piles = state!.piles.length;
  $('outcome-title').textContent = state!.won ? 'You win!' : 'Game over';
  $('outcome-text').textContent = state!.won
    ? `All 52 cards in one pile in ${line!.cursor} moves.`
    : `No moves left with ${piles} piles. Undo or restart to try another line, or ask the solver for a way to win.`;
  $('o-solve').hidden = state!.won;
  $<HTMLButtonElement>('o-undo').disabled = line!.cursor === 0;
  box.hidden = false;
}

$('o-undo').addEventListener('click', () => seek((line?.cursor ?? 0) - 1));
$('o-restart').addEventListener('click', () => seek(0));
$('o-new').addEventListener('click', () => playSeed(randomSeed()));
$('o-solve').addEventListener('click', () => startSolve(true));
$('o-close').addEventListener('click', () => {
  outcomeHiddenAt = positionKey();
  renderOutcome();
});

// ---- solve & replay ---------------------------------------------------------------------------------------------

let worker: Worker | null = null;
let solveClock: number | undefined;
const SOLVE_LIMIT = 300_000_000; // positions; about half a minute in the browser at worst

function note(text: string) {
  $('note').textContent = text;
  $('note').hidden = text === '';
}

function setSolving(on: boolean) {
  $('solving').hidden = !on;
  $<HTMLButtonElement>('solve').hidden = on;
  $<HTMLButtonElement>('o-solve').disabled = on;
  clearInterval(solveClock);
  if (on) {
    const t0 = performance.now();
    const tickClock = () => ($('solving-time').textContent = `${((performance.now() - t0) / 1000).toFixed(1)} s`);
    tickClock();
    solveClock = window.setInterval(tickClock, 100);
  }
}

/** Solves the deal on screen from the current position (or from the start), verifies the line, and replays it. */
function startSolve(fromStart = false) {
  const game = playLine;
  if (!game) return;
  const prefix = fromStart ? [] : game.moves.slice(0, game.cursor);
  note(fromStart ? 'Solving from the start of the deal…' : 'Solving from this position…');
  setSolving(true);
  worker ??= new Worker(new URL('./solver-worker.ts', import.meta.url), { type: 'module' });
  worker.onmessage = (e: MessageEvent<SolveResponse>) => finishSolve(e.data, game, prefix, fromStart);
  worker.postMessage({ cards: game.cards, moves: prefix, maxNodes: SOLVE_LIMIT, seed: Math.floor(Math.random() * 2 ** 32) });
}

function finishSolve(r: SolveResponse, game: Line, prefix: number[], fromStart: boolean) {
  setSolving(false);
  const effort = `${(r.nodes ?? 0).toLocaleString()} positions searched in ${(r.ms / 1000).toFixed(1)} s`;
  if (r.error) return note(`The solver failed: ${r.error}`);
  if (r.result === 'win' && r.moves) {
    let record: string;
    try {
      record = exportRecord(game.cards, game.seed, [...prefix, ...r.moves]); // verified before it is shown
    } catch (err) {
      return note(`The solver's line failed verification and was discarded: ${String(err)}`);
    }
    showMode('replay');
    openText(record);
    seek(prefix.length);
    const from = prefix.length ? `from move ${prefix.length}` : 'from the start';
    note(`Solved ${from}: a verified win (${effort}). Replaying it now; use Continue from here to take over.`);
    togglePlayback();
  } else if (r.result === 'exhausted' && !fromStart && prefix.length > 0) {
    note(`No win exists from this position (${effort}). Solving from the start of the deal instead…`);
    startSolve(true);
  } else if (r.result === 'exhausted') {
    const where = fromStart || prefix.length === 0 ? 'from the start of the deal' : 'from here';
    note(`No win found: every line ${where} was searched (${effort}). This deal looks unwinnable.`);
  } else {
    note(`No win found within the search limit (${effort}). Try again: each attempt searches in a new random order.`);
  }
}

$('solve').addEventListener('click', () => startSolve(false));
$('solve-cancel').addEventListener('click', () => {
  worker?.terminate();
  worker = null;
  setSolving(false);
  note('Solve cancelled.');
});

// ---- play ----------------------------------------------------------------------------------------------------

function play(mv: LegalMove) {
  if (mode !== 'play' || !line) return;
  line.moves = [...line.moves.slice(0, line.cursor), mv.byte];
  line.texts = [...line.texts.slice(0, line.cursor), `${mv.mover}>${mv.target}`];
  line.cursor++;
  selected = null;
  note('');
  render(true);
}

function seek(k: number, animate = false, duration = 420) {
  if (!line) return;
  const next = Math.max(0, Math.min(line.moves.length, k));
  const forward = next === line.cursor + 1;
  line.cursor = next;
  selected = null;
  render(animate && forward, duration);
}

function onPile(i: number) {
  if (mode !== 'play') return;
  const mv = targetsOf(selected).get(i);
  if (mv) return play(mv);
  selected = selected === i || movesFrom(i).length === 0 ? null : i;
  updateHighlights();
}

board.addEventListener('click', (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>('.pile');
  if (el) onPile(Number(el.dataset.index));
  else if (selected !== null) {
    selected = null;
    updateHighlights();
  }
});
board.addEventListener('keydown', (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>('.pile');
  if (el && (e.key === 'Enter' || e.key === ' ')) {
    e.preventDefault();
    onPile(Number(el.dataset.index));
  }
});
board.addEventListener('dragstart', (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>('.pile');
  if (!el || mode !== 'play') return;
  selected = Number(el.dataset.index);
  e.dataTransfer?.setData('text/plain', el.dataset.index!);
  updateHighlights();
});
board.addEventListener('dragover', (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>('.pile');
  if (el && targetsOf(selected).has(Number(el.dataset.index))) e.preventDefault();
});
board.addEventListener('drop', (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>('.pile');
  const mv = el ? targetsOf(selected).get(Number(el.dataset.index)) : undefined;
  if (mv) {
    e.preventDefault();
    play(mv);
  }
});

$('undo').addEventListener('click', () => seek((line?.cursor ?? 0) - 1));
$('redo').addEventListener('click', () => seek((line?.cursor ?? 0) + 1, true));
$('restart').addEventListener('click', () => seek(0));
$('history').addEventListener('click', (e) => {
  const k = (e.target as HTMLElement).dataset.k;
  if (k) seek(Number(k));
});
$('new-random').addEventListener('click', () => playSeed(randomSeed()));
$('seed-input').addEventListener('keydown', (e) => {
  if (e.key !== 'Enter') return;
  const input = e.target as HTMLInputElement;
  try {
    const seed = BigInt(input.value.trim());
    if (input.value.trim() === '' || seed < 0n || seed >= 1n << 64n) throw new Error();
    input.setCustomValidity('');
    playSeed(seed);
  } catch {
    input.setCustomValidity('A whole number from 0 to 18446744073709551615');
    input.reportValidity();
  }
});
$('seed-input').addEventListener('input', (e) => (e.target as HTMLInputElement).setCustomValidity(''));
$('export').addEventListener('click', () => {
  if (!line) return;
  try {
    const json = exportRecord(line.cards, line.seed, line.moves.slice(0, line.cursor));
    const name = `accordion-${line.seed !== undefined ? `seed-${line.seed}` : 'deck'}-${line.cursor}-moves.json`;
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([json], { type: 'application/json' }));
    a.download = name;
    a.click();
    URL.revokeObjectURL(a.href);
  } catch (err) {
    alert(`Export failed: ${String(err)}`);
  }
});

// ---- replay --------------------------------------------------------------------------------------------------

function stopPlayback() {
  if (timer !== undefined) clearTimeout(timer);
  timer = undefined;
  renderReplayControls();
}

const interval = () => 1000 / Number($<HTMLSelectElement>('r-speed').value);

function tick() {
  if (!line || line.cursor >= line.moves.length) return stopPlayback();
  seek(line.cursor + 1, true, Math.min(700, interval() * 0.75)); // slower speeds get a slower, clearer slide
  timer = window.setTimeout(tick, interval());
  renderReplayControls();
}

function togglePlayback() {
  if (timer !== undefined) return stopPlayback();
  if (!line) return;
  if (line.cursor >= line.moves.length) seek(0);
  timer = window.setTimeout(tick, 0);
  renderReplayControls();
}

function loadEntry(index: number) {
  stopPlayback();
  replayEntry = replayEntries[index] ?? null;
  if (!replayEntry?.cards) {
    replayLine = null;
  } else {
    const seed = replayEntry.seed !== undefined ? BigInt(replayEntry.seed) : undefined;
    const label = `line ${replayEntry.line}${seed !== undefined ? `, seed ${seed}` : ''}`;
    replayLine = { ...newLine(replayEntry.cards, label, seed), moves: replayEntry.moves, texts: replayEntry.texts };
  }
  line = replayLine;
  render();
  if (replayEntry && !replayEntry.cards) $('status').textContent = `✗ ${replayEntry.error}`;
}

function openText(text: string) {
  replayEntries = openSolveFile(text);
  const select = $<HTMLSelectElement>('entry-select');
  select.replaceChildren(
    ...replayEntries.map((e, i) => {
      const o = document.createElement('option');
      o.value = String(i);
      const what = e.cards ? `${e.result} · ${e.moves.length} moves` : 'unreadable';
      o.textContent = `Line ${e.line}${e.seed ? ` · seed ${e.seed}` : ''} · ${what} · ${e.verified ? '✓' : '✗'}`;
      return o;
    }),
  );
  select.hidden = replayEntries.length < 2;
  if (replayEntries.length === 0) {
    replayEntry = null;
    line = replayLine = null;
    render();
    $('status').textContent = 'No solve records found in that file.';
    return;
  }
  loadEntry(0);
}

$('file-input').addEventListener('change', async (e) => {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (file) openText(await file.text());
});
$('paste-open').addEventListener('click', () => openText($<HTMLTextAreaElement>('paste-text').value));
$('entry-select').addEventListener('change', (e) => loadEntry(Number((e.target as HTMLSelectElement).value)));
$('r-play').addEventListener('click', togglePlayback);
$('r-first').addEventListener('click', () => (stopPlayback(), seek(0)));
$('r-last').addEventListener('click', () => (stopPlayback(), seek(line?.moves.length ?? 0)));
$('r-back').addEventListener('click', () => (stopPlayback(), seek((line?.cursor ?? 0) - 1)));
$('r-fwd').addEventListener('click', () => (stopPlayback(), seek((line?.cursor ?? 0) + 1, true)));
$('r-slider').addEventListener('input', (e) => {
  const k = Number((e.target as HTMLInputElement).value); // read before stopPlayback redraws the slider
  stopPlayback();
  seek(k);
});
$('r-continue').addEventListener('click', () => {
  if (!line) return;
  stopPlayback();
  playLine = {
    ...newLine(line.cards, `${line.label}, continued by hand`, line.seed),
    moves: line.moves.slice(0, line.cursor),
    texts: line.texts.slice(0, line.cursor),
    cursor: line.cursor,
  };
  showMode('play');
});

// ---- modes and keyboard ------------------------------------------------------------------------------------------

function showMode(next: Mode) {
  stopPlayback();
  mode = next;
  selected = null;
  for (const tab of document.querySelectorAll<HTMLElement>('[role=tab]')) {
    tab.setAttribute('aria-selected', String(tab.dataset.mode === mode));
  }
  $('play-bar').hidden = mode !== 'play';
  $('replay-bar').hidden = mode !== 'replay';
  line = mode === 'play' ? playLine : replayLine;
  render();
}

for (const tab of document.querySelectorAll<HTMLElement>('[role=tab]')) {
  tab.addEventListener('click', () => showMode(tab.dataset.mode as Mode));
}

document.addEventListener('keydown', (e) => {
  const t = e.target as HTMLElement;
  if (t.closest('input, textarea, select, dialog')) return;
  const ctrl = e.ctrlKey || e.metaKey;
  if (mode === 'play') {
    if (ctrl && e.key.toLowerCase() === 'z' && !e.shiftKey) (e.preventDefault(), seek((line?.cursor ?? 0) - 1));
    else if (ctrl && (e.key.toLowerCase() === 'y' || (e.key.toLowerCase() === 'z' && e.shiftKey)))
      (e.preventDefault(), seek((line?.cursor ?? 0) + 1, true));
    else if (e.key === 'Escape' && selected !== null) ((selected = null), updateHighlights());
  } else if (!t.closest('.pile')) {
    if (e.key === ' ') (e.preventDefault(), togglePlayback());
    else if (e.key === 'ArrowLeft') (stopPlayback(), seek((line?.cursor ?? 0) - 1));
    else if (e.key === 'ArrowRight') (stopPlayback(), seek((line?.cursor ?? 0) + 1, true));
    else if (e.key === 'Home') (stopPlayback(), seek(0));
    else if (e.key === 'End') (stopPlayback(), seek(line?.moves.length ?? 0));
  }
});

// ---- deck entry ----------------------------------------------------------------------------------------------

const ALL_CARDS: Card[] = [...'CDHS'].flatMap((s) => [...'A23456789TJQK'].map((r) => r + s));
const deckText = $<HTMLTextAreaElement>('deck-text');

function checkDeckInput() {
  const result = checkDeck(deckText.value);
  const box = $('deck-check');
  const missing = new Set<Card>(result.issues?.find((i) => i.kind === 'missing')?.cards ?? []);
  if (result.ok) {
    box.className = 'deck-check good';
    box.textContent = '✓ A valid deck of 52 cards.';
  } else {
    box.className = 'deck-check bad';
    box.textContent = deckText.value.trim() ? result.message ?? '' : '';
  }
  for (const b of $('picker').querySelectorAll<HTMLButtonElement>('button')) b.disabled = !missing.has(b.dataset.card!);
  $('picked-count').textContent = String(52 - missing.size);
  $<HTMLButtonElement>('deck-play').disabled = !result.ok;
  return result;
}

$('picker').replaceChildren(
  ...ALL_CARDS.map((card) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.dataset.card = card;
    b.className = `pick ${cardFace(card).red ? 'red' : 'black'}`;
    b.textContent = cardLabel(card);
    b.setAttribute('aria-label', `Add ${cardLabel(card)}`);
    return b;
  }),
);
$('picker').addEventListener('click', (e) => {
  const card = (e.target as HTMLElement).dataset.card;
  if (!card) return;
  deckText.value = `${deckText.value.trim()} ${card}`.trim();
  checkDeckInput();
});
$('pick-undo').addEventListener('click', () => {
  deckText.value = deckText.value.trim().replace(/[\s,]*[^\s,]+$/, '');
  checkDeckInput();
});
$('pick-clear').addEventListener('click', () => {
  deckText.value = '';
  checkDeckInput();
});
deckText.addEventListener('input', checkDeckInput);
$('enter-deck').addEventListener('click', () => {
  checkDeckInput();
  $<HTMLDialogElement>('deck-dialog').showModal();
  deckText.focus();
});
$('deck-play').addEventListener('click', () => {
  const result = checkDeckInput();
  if (!result.ok) return;
  playLine = newLine(result.ok, 'your deck');
  $<HTMLDialogElement>('deck-dialog').close();
  showMode('play');
});

// ---- start ---------------------------------------------------------------------------------------------------

try {
  $('engine').textContent = `engine ${await loadEngine()}`;
  playSeed(randomSeed());
} catch (err) {
  $('status').textContent = `The engine failed to load: ${String(err)}`;
}
