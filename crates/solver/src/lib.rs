//! Accordion solitaire solver: first-win search with a dead-position table and gated dead-end tests (Phase 2),
//! randomized restarts (Phase 4), and the Phase 5 strategies: a random-prefix ladder in front of the thorough
//! search, move-order policies, sleep sets, and complete endgame tables keyed by suit/rank pattern.

pub mod dfs;
pub mod endgame;
pub mod solver;
pub mod table;

pub use dfs::{Gates, Order, Search, SearchOptions, SearchResult, Stats};
pub use endgame::Endgame;
pub use solver::{Config, Ladder, Report, Restarts, Shared, Stage, Verdict, portfolio, solve};
