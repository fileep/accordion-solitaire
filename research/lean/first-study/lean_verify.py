p = r'C:/Users/Filip/Documents/Accordion Solitaire/crates/cli/src/runner.rs'
s = open(p, encoding='utf-8').read()


def rep(a, b):
    global s
    assert s.count(a) == 1, a[:70]
    s = s.replace(a, b)


rep('''use accordion_core::{Deal, Move, Outcome, SolveRecord};''', '''use accordion_core::{Deal, Game, Move, Outcome, SolveRecord};''')
rep('''/// Verifies a deal's line with the reference engine and serializes its record.
fn finish(index: usize, seed: u64, deal: &Deal, report: Report, us: u64, records: bool) -> Solved {
    let (outcome, line) = match &report.verdict {
        Verdict::Win(line) => (Outcome::Win, line),
        _ => (Outcome::Unknown, &report.best_line),
    };
    // Nothing counts as a win until the reference engine has replayed it.
    let (verified, record) = match SolveRecord::new(deal, Some(seed), outcome, line) {''', '''/// Verifies a deal's line with the reference engine and serializes its record. Without records, the reference
/// engine replays the line directly ([`replay`]): the same check `verify` makes of a record, without writing the
/// record's card notation and parsing it back, which took a fifth of the CPU workers' time next to the GPU.
fn finish(index: usize, seed: u64, deal: &Deal, report: Report, us: u64, records: bool) -> Solved {
    let (outcome, line) = match &report.verdict {
        Verdict::Win(line) => (Outcome::Win, line),
        _ => (Outcome::Unknown, &report.best_line),
    };
    // Nothing counts as a win until the reference engine has replayed it.
    if !records {
        let verified = replay(deal, line, outcome == Outcome::Win);
        return Solved { index, seed, report, us, verified, record: None, fingerprint: fingerprint(deal) };
    }
    let (verified, record) = match SolveRecord::new(deal, Some(seed), outcome, line) {'''),
rep('''/// A 64-bit fingerprint of the deal with suits and ranks renamed''', '''/// The reference engine's replay of `line` from `deal`: every move legal, and one pile at the end when `won` is
/// claimed (after exactly 51 moves, since each move takes one pile away).
fn replay(deal: &Deal, line: &[Move], won: bool) -> Result<(), String> {
    let mut game = Game::new(deal);
    for (i, &mv) in line.iter().enumerate() {
        game.apply(mv).map_err(|e| format!("move {}: {e}", i + 1))?;
    }
    if won && !game.is_won() {
        return Err(format!("the line ends with {} piles, not one", game.pile_count()));
    }
    Ok(())
}

/// A 64-bit fingerprint of the deal with suits and ranks renamed''')
open(p, 'w', encoding='utf-8').write(s)
print('ok')
