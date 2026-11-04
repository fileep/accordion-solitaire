//! WebAssembly bindings for the visualizer. They expose the same `accordion-core` code the solver uses,
//! so the browser never carries its own copy of the rules.

use wasm_bindgen::prelude::*;

/// Engine version, shown in the visualizer so it's clear which rules build is running.
#[wasm_bindgen(js_name = engineVersion)]
pub fn engine_version() -> String {
    accordion_core::VERSION.to_string()
}
