//! Accordion solitaire core: cards, deals, rules, the reference and fast engines, and file formats.
//!
//! Shared by the native solver and, through `accordion-wasm`, by the visualizer, so both always play
//! by exactly the same rules. Formats are specified in docs/FORMATS.md.

pub mod card;
pub mod deal;
pub mod deck_input;
pub mod fast;
pub mod game;
pub mod notation;
pub mod quick;
pub mod rng;
pub mod selfcheck;
pub mod solve_file;
pub mod verify;

pub use card::Card;
pub use deal::Deal;
pub use game::{Game, Move};
pub use solve_file::{Outcome, SolveRecord};

/// Engine version, recorded in every solve file so results can be traced to the code that made them.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
