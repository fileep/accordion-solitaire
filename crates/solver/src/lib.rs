//! Accordion solitaire solver: first-win search with a dead-position table and gated dead-end tests (Phase 2),
//! randomized restarts under a Luby schedule (Phase 4), with random-prefix attempts, the endgame table and
//! cache, and the hard-deal pipeline to follow in PLAN.md Phases 5–7.

pub mod dfs;
pub mod solver;
pub mod table;

pub use dfs::{Gates, Search, SearchResult, Stats};
pub use solver::{Config, Report, Shared, Verdict, portfolio, solve};
