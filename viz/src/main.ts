// Accordion visualizer entry point. Until Phase 3 builds the real UI, this page just proves the Rust
// engine runs in the browser: version, a seeded deal, and the deck parser rejecting a bad deck.
import './style.css';
import init, { dealFromSeed, engineVersion, parseDeck } from './wasm/accordion';

const app = document.querySelector<HTMLElement>('#app')!;

try {
  await init();
  const deal = dealFromSeed(42n);
  const roundTrip = JSON.parse(parseDeck(deal.toLowerCase().replaceAll(' ', ', ')));
  const bad = JSON.parse(parseDeck('AC 2C 2C ZZ'));
  app.innerHTML = `
    <h1>Accordion</h1>
    <p>Engine <code>${engineVersion()}</code> loaded from WebAssembly.</p>
    <h2>Seed 42</h2>
    <p><code>${deal}</code></p>
    <p>Parsed back from lowercase, comma-separated text: ${roundTrip.ok === deal ? 'identical' : 'MISMATCH'}.</p>
    <h2>Deck check</h2>
    <pre>${bad.message}</pre>`;
} catch (err) {
  app.innerHTML = `<h1>Accordion</h1><p class="error">Engine failed to load: ${String(err)}</p>`;
}
