// Runs the Rust solver (via WebAssembly) off the main thread, so the page stays responsive while it searches.
// Cancelling means terminating this worker; the page starts a new one for the next solve.
import init, { solve } from './wasm/accordion';

export interface SolveRequest {
  cards: string;
  moves: number[];
  maxNodes: number;
  seed: number;
}

export interface SolveResponse {
  result?: 'win' | 'exhausted' | 'unknown';
  moves?: number[];
  nodes?: number;
  attempts?: number;
  bestPiles?: number;
  error?: string;
  ms: number;
}

const ready = init();

self.onmessage = async (e: MessageEvent<SolveRequest>) => {
  await ready;
  const t0 = performance.now();
  const { cards, moves, maxNodes, seed } = e.data;
  const out = JSON.parse(solve(cards, Uint8Array.from(moves), maxNodes, seed));
  self.postMessage({ ...out, ms: performance.now() - t0 } satisfies SolveResponse);
};
