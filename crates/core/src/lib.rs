//! Accordion solitaire core: cards, deals, rules, the reference and fast engines, and file formats.
//!
//! Shared by the native solver and, through `accordion-wasm`, by the visualizer, so both always play
//! by exactly the same rules. Contents land in PLAN.md Phases 1–2.

/// Engine version, recorded in every solve file so results can be traced to the code that made them.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
