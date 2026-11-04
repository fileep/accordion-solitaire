// Accordion visualizer (PLAN.md Phase 3): play a deal by hand with undo, enter your own deck, and replay solve
// files with a slider and speeds. One deal at a time; every rule comes from the Rust engine via WebAssembly.
import './style.css';
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

/** FLIP: remember where each top card was, rebuild, then animate every pile from its old place to its new one. */
function renderBoard(animate: boolean, duration = 220) {
  const before = new Map<string, DOMRect>();
  if (animate && !reduceMotion.matches && duration > 0) {
    for (const el of board.querySelectorAll<HTMLElement>('.pile')) before.set(el.dataset.top!, el.getBoundingClientRect());
  }
  if (!line || !state) {
    board.replaceChildren();
    return;
  }
  board.replaceChildren(...state.piles.map(pileElement));
  updateHighlights();
  for (const el of board.querySelectorAll<HTMLElement>('.pile')) {
    const old = before.get(el.dataset.top!);
    if (!old) continue;
    const now = el.getBoundingClientRect();
    const dx = old.left - now.left;
    const dy = old.top - now.top;
    if (dx || dy) {
      el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }], { duration, easing: 'ease-out' });
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

function render(animate = false, duration = 220) {
  state = line ? stateAfter(line.cards, line.moves.slice(0, line.cursor)) : null;
  if (selected !== null && (!state || selected >= state.piles.length)) selected = null;
  renderBoard(animate, duration);
  renderHistory();
  renderStatus();
  renderReplayControls();
  $<HTMLButtonElement>('undo').disabled = !line || line.cursor === 0;
  $<HTMLButtonElement>('redo').disabled = !line || line.cursor >= line.moves.length;
}

// ---- play ----------------------------------------------------------------------------------------------------

function play(mv: LegalMove) {
  if (mode !== 'play' || !line) return;
  line.moves = [...line.moves.slice(0, line.cursor), mv.byte];
  line.texts = [...line.texts.slice(0, line.cursor), `${mv.mover}>${mv.target}`];
  line.cursor++;
  selected = null;
  render(true);
}

function seek(k: number, animate = false, duration = 220) {
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
$('new-seed').addEventListener('click', () => {
  const input = $<HTMLInputElement>('seed-input');
  try {
    const seed = BigInt(input.value.trim());
    if (seed < 0n || seed >= 1n << 64n) throw new Error();
    input.setCustomValidity('');
    playSeed(seed);
  } catch {
    input.setCustomValidity('A whole number from 0 to 18446744073709551615');
    input.reportValidity();
  }
});
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
  seek(line.cursor + 1, true, Math.min(220, interval() * 0.8));
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
