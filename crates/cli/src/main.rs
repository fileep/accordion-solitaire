//! `acc`: deal, verify, solve and batch-run Accordion games. Commands land in PLAN.md Phases 1 and 4.

fn main() {
    println!("acc {} (accordion-core {})", env!("CARGO_PKG_VERSION"), accordion_core::VERSION);
}
