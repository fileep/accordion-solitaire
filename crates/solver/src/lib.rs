//! Accordion solitaire solver: first-win search with a dead-position table and gated dead-end tests (Phase 2),
//! with random-prefix attempts, restart schedules, the endgame table and cache, the batch runner and the
//! hard-deal pipeline to follow in PLAN.md Phases 4–7.

pub mod dfs;
pub mod table;

pub use dfs::{Gates, Search, SearchResult, Stats};
