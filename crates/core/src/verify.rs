//! Verification: replay a solve record's moves with the reference engine and check its claimed result.
//! Nothing counts as a win until it passes here.

use crate::deal::Deal;
use crate::game::{Game, Move};
use crate::notation::{CardMove, decode_compact};
use crate::solve_file::{FORMAT, Outcome, SolveRecord};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub result: Outcome,
    pub moves: usize,
    pub piles_left: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyError {
    /// 1-based number of the offending move, when a move is at fault.
    pub move_number: Option<usize>,
    pub message: String,
}

impl VerifyError {
    fn record(message: impl Into<String>) -> VerifyError {
        VerifyError { move_number: None, message: message.into() }
    }

    fn at_move(index: usize, message: impl Into<String>) -> VerifyError {
        VerifyError { move_number: Some(index + 1), message: message.into() }
    }
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.move_number {
            Some(n) => write!(f, "move {n}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Checks the format and rules, that the cards are a valid deal (matching the seed if there is one), that every
/// move is legal, and that a claimed win ends in one pile (with the required final card, if the rules name one).
/// A claimed loss can't be confirmed by replay; only its moves are checked.
pub fn verify(record: &SolveRecord) -> Result<Verified, VerifyError> {
    if record.format != FORMAT {
        return Err(VerifyError::record(format!("unknown format {:?}; expected {FORMAT:?}", record.format)));
    }
    record.rules.check_supported().map_err(VerifyError::record)?;
    let deal = Deal::parse(&record.deal.cards).map_err(|e| VerifyError::record(format!("deal: {e}")))?;
    if let Some(seed) = record.deal.seed
        && Deal::from_seed(seed) != deal
    {
        return Err(VerifyError::record(format!("deal cards are not the deal for seed {seed}")));
    }

    let compact = record.moves_b64.as_deref().map(decode_compact).transpose().map_err(VerifyError::record)?;
    let mut game = Game::new(&deal);
    let mut played: Vec<Move> = Vec::new();
    if !record.moves.is_empty() || compact.is_none() {
        for (i, text) in record.moves.iter().enumerate() {
            let cm = CardMove::parse(text).map_err(|e| VerifyError::at_move(i, e))?;
            let mv = game
                .move_between(cm.mover, cm.target)
                .map_err(|e| VerifyError::at_move(i, format!("{text}: {e}")))?;
            game.apply(mv).expect("move_between returns legal moves");
            played.push(mv);
        }
        if let Some(compact) = &compact
            && *compact != played
        {
            let i = played.iter().zip(compact).take_while(|(a, b)| a == b).count();
            return Err(VerifyError::at_move(i, "moves and moves_b64 disagree"));
        }
    } else if let Some(compact) = compact {
        for (i, mv) in compact.into_iter().enumerate() {
            game.apply(mv).map_err(|e| VerifyError::at_move(i, e.to_string()))?;
            played.push(mv);
        }
    }

    check_conservation(&game)?;
    if record.result == Outcome::Win {
        if !game.is_won() {
            return Err(VerifyError::record(format!(
                "claims a win, but {} piles are left after {} moves",
                game.pile_count(),
                played.len()
            )));
        }
        // Every legal move removes exactly one pile, so a real win is 51 moves. Guards the engine itself.
        if played.len() != 51 {
            return Err(VerifyError::record(format!(
                "internal error: one pile after {} moves, not 51",
                played.len()
            )));
        }
        if let Some(required) = record.rules.final_card
            && game.top(0) != required
        {
            return Err(VerifyError::record(format!(
                "{} finished on top; the rules require {required}",
                game.top(0)
            )));
        }
    }
    Ok(Verified { result: record.result, moves: played.len(), piles_left: game.pile_count() })
}

/// Defense in depth against a bug in the reference engine itself: after replay the piles must still hold
/// all 52 cards, each exactly once (moves only stack piles; they never create, drop or copy cards).
fn check_conservation(game: &Game) -> Result<(), VerifyError> {
    let mut seen = [false; 52];
    let mut count = 0;
    for card in game.piles().iter().flatten() {
        if std::mem::replace(&mut seen[card.index() as usize], true) {
            return Err(VerifyError::record(format!("internal error: {card} appears twice after replay")));
        }
        count += 1;
    }
    if count != 52 {
        return Err(VerifyError::record(format!("internal error: {count} cards after replay, not 52")));
    }
    Ok(())
}
