// Accordion visualizer entry point. Phase 0: proves the Rust engine loads in the browser via WASM.
// Play, replay and deck entry land in PLAN.md Phase 3.
import './style.css';
import init, { engineVersion } from './wasm/accordion';

const app = document.querySelector<HTMLElement>('#app')!;

try {
  await init();
  app.innerHTML = `
    <h1>Accordion</h1>
    <p>Engine <code>${engineVersion()}</code> loaded from WebAssembly.</p>`;
} catch (err) {
  app.innerHTML = `<h1>Accordion</h1><p class="error">Engine failed to load: ${String(err)}</p>`;
}
