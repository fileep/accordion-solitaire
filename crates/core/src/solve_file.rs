//! Solve files, format `accordion-solve/1` (docs/FORMATS.md): one JSON object per game, and batches as
//! JSON Lines with one object per line.

use crate::card::Card;
use crate::deal::Deal;
use crate::game::{Game, Move, MoveError};
use crate::notation::CardMove;
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "accordion-solve/1";

/// The rule set a game was played under. Only the standard rules are supported today; `final_card` is the
/// hook for Accordion's Revenge (the named card must finish on top).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    pub deal: String,
    pub leaps: Vec<u8>,
    #[serde(rename = "match")]
    pub matching: String,
    pub win: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_card: Option<Card>,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            deal: "open".into(),
            leaps: vec![1, 3],
            matching: "suit-or-rank".into(),
            win: "one-pile".into(),
            final_card: None,
        }
    }
}

impl Rules {
    /// Checks these are rules this engine plays, ignoring `final_card`, which verification enforces.
    pub fn check_supported(&self) -> Result<(), String> {
        let standard = Rules { final_card: self.final_card, ..Rules::default() };
        if *self == standard {
            Ok(())
        } else {
            Err(format!(
                "unsupported rules (deal {:?}, leaps {:?}, match {:?}, win {:?}); only open, [1, 3], suit-or-rank, one-pile",
                self.deal, self.leaps, self.matching, self.win
            ))
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Win,
    Unknown,
    Loss,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DealInfo {
    /// Present when the deal came from a seed; the cards must then equal `Deal::from_seed(seed)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The 52 cards left to right, in deck-input notation.
    pub cards: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SolveRecord {
    pub format: String,
    pub rules: Rules,
    pub deal: DealInfo,
    pub result: Outcome,
    /// Moves in card notation (`7H>7C`). For `unknown` this is the best line found so far.
    #[serde(default)]
    pub moves: Vec<String>,
    /// The same moves in compact form; either field may be used, and if both are present they must agree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moves_b64: Option<String>,
    /// Free-form solver statistics, carried through unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<serde_json::Map<String, serde_json::Value>>,
}

impl SolveRecord {
    /// A record for `deal` under the standard rules, naming each move in card notation by replaying it.
    pub fn new(
        deal: &Deal,
        seed: Option<u64>,
        result: Outcome,
        moves: &[Move],
    ) -> Result<SolveRecord, MoveError> {
        let mut game = Game::new(deal);
        let mut text = Vec::with_capacity(moves.len());
        for &mv in moves {
            game.check(mv)?;
            let (mover, target) = game.cards_of(mv);
            text.push(CardMove { mover, target }.to_string());
            game.apply(mv)?;
        }
        Ok(SolveRecord {
            format: FORMAT.into(),
            rules: Rules::default(),
            deal: DealInfo { seed, cards: deal.to_string() },
            result,
            moves: text,
            moves_b64: None,
            stats: None,
        })
    }

    /// Pretty JSON, for single-game files.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("solve records always serialize")
    }

    /// One line, for JSON Lines batches.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("solve records always serialize")
    }
}

/// Parses a solve file: a single JSON object, or JSON Lines (one object per line; blank lines skipped).
/// Each record comes with its 1-based line number (1 for a single object).
pub fn parse_records(text: &str) -> Vec<(usize, Result<SolveRecord, String>)> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
        && value.is_object()
    {
        return vec![(1, serde_json::from_value(value).map_err(|e| e.to_string()))];
    }
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, line)| (i + 1, serde_json::from_str(line).map_err(|e| e.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_json_and_json_lines() {
        let deal = Deal::from_seed(3);
        let first = Game::new(&deal).legal_moves()[0];
        let rec = SolveRecord::new(&deal, Some(3), Outcome::Unknown, &[first]).unwrap();
        let single = parse_records(&rec.to_json());
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].1.as_ref().unwrap(), &rec);
        let batch = format!("{}\n\n{}\n", rec.to_json_line(), rec.to_json_line());
        let parsed = parse_records(&batch);
        assert!(parsed.iter().all(|(_, r)| r.as_ref() == Ok(&rec)));
        assert_eq!(parsed.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn standard_rules_serialize_with_match_key() {
        let json = serde_json::to_string(&Rules::default()).unwrap();
        assert_eq!(json, r#"{"deal":"open","leaps":[1,3],"match":"suit-or-rank","win":"one-pile"}"#);
        assert!(Rules::default().check_supported().is_ok());
        assert!(Rules { leaps: vec![1, 2], ..Rules::default() }.check_supported().is_err());
    }
}
