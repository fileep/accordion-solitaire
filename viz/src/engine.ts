// Typed wrappers around the Rust engine compiled to WebAssembly. The page never implements the rules
// itself: it keeps a deal and a list of moves (compact bytes, pile << 1 | leap) and asks the engine for
// everything else.
import init, * as wasm from './wasm/accordion';

export type Card = string;

export interface LegalMove {
  pile: number;
  leap: boolean;
  byte: number;
  mover: Card;
  target: Card;
}

export interface State {
  /** Piles left to right, each bottom to top. */
  piles: Card[][];
  legal: LegalMove[];
  won: boolean;
}

export interface DeckCheck {
  /** The deck in canonical text, when valid. */
  ok?: string;
  /** Every problem found (docs/FORMATS.md "Deck input"), when not. */
  issues?: { kind: 'unknown_token' | 'duplicate' | 'missing' | 'empty_pile'; cards?: Card[] }[];
  message?: string;
}

export interface ReplayEntry {
  line: number;
  seed?: string;
  cards?: string;
  result?: 'win' | 'unknown' | 'loss';
  /** The playable prefix: every move before the first illegal one. */
  moves: number[];
  texts: string[];
  verified: boolean;
  error?: string;
  errorMove?: number | null;
}

export async function loadEngine(): Promise<string> {
  await init();
  return wasm.engineVersion();
}

export const dealFromSeed = (seed: bigint): string => wasm.dealFromSeed(seed);

export const checkDeck = (text: string): DeckCheck => JSON.parse(wasm.parseDeck(text));

export function stateAfter(cards: string, moves: number[]): State {
  const r = JSON.parse(wasm.replay(cards, Uint8Array.from(moves)));
  if (r.error) throw new Error(r.move ? `move ${r.move}: ${r.error}` : r.error);
  return r.ok as State;
}

export const openSolveFile = (text: string): ReplayEntry[] => JSON.parse(wasm.openSolveFile(text));

export function exportRecord(cards: string, seed: bigint | undefined, moves: number[]): string {
  const r = JSON.parse(wasm.exportRecord(cards, seed, Uint8Array.from(moves)));
  if (r.error) throw new Error(r.error);
  return r.ok as string;
}

const SUIT_SYMBOL: Record<string, string> = { C: '♣', D: '♦', H: '♥', S: '♠' };

/** Display parts of a card code like "TH": rank "10", suit "♥", red. */
export function cardFace(card: Card): { rank: string; suit: string; red: boolean } {
  const r = card[0] === 'T' ? '10' : card[0];
  return { rank: r, suit: SUIT_SYMBOL[card[1]], red: card[1] === 'D' || card[1] === 'H' };
}

export const cardLabel = (card: Card): string => {
  const f = cardFace(card);
  return f.rank + f.suit;
};
