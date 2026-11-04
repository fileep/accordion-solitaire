//! Deck input: lenient tokens, strict validation (docs/FORMATS.md "Deck input").
//!
//! Cards are separated by whitespace or commas and parsed with [`Card::parse`]. Validation demands exactly
//! the 52 distinct cards and reports every problem at once, with 1-based card positions, so a user can fix
//! a typed deck in one pass. Positions (games in progress) use the same notation with piles separated by `|`.

use crate::card::Card;
use crate::deal::{DECK_SIZE, Deal};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeckIssue {
    /// The token at this 1-based position is not a card.
    UnknownToken { position: usize, token: String },
    /// The same card at two positions.
    Duplicate { card: Card, first: usize, second: usize },
    /// Cards that never appear.
    Missing { cards: Vec<Card> },
    /// Position input only: a pile with no cards (1-based pile number).
    EmptyPile { pile: usize },
}

impl fmt::Display for DeckIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeckIssue::UnknownToken { position, token } => write!(
                f,
                "card {position}: \"{token}\" is not a card (expected a rank A, 2–9, T or 10, J, Q, K, then a suit C, D, H, S)"
            ),
            DeckIssue::Duplicate { card, first, second } => {
                write!(f, "{card} appears twice, as card {first} and card {second}")
            }
            DeckIssue::Missing { cards } => {
                let list: Vec<String> = cards.iter().map(Card::to_string).collect();
                write!(
                    f,
                    "missing {} card{}: {}",
                    cards.len(),
                    if cards.len() == 1 { "" } else { "s" },
                    list.join(" ")
                )
            }
            DeckIssue::EmptyPile { pile } => write!(f, "pile {pile} is empty"),
        }
    }
}

/// Every problem found in a deck or position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckError {
    pub issues: Vec<DeckIssue>,
}

impl fmt::Display for DeckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, issue) in self.issues.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DeckError {}

fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty())
}

/// Parses tokens into (position, card) pairs, numbering every token from `next_position`.
/// Returns the position after the last token.
fn parse_tokens<'a>(
    toks: impl Iterator<Item = &'a str>,
    mut next_position: usize,
    cards: &mut Vec<(usize, Card)>,
    issues: &mut Vec<DeckIssue>,
) -> usize {
    for token in toks {
        match Card::parse(token) {
            Some(card) => cards.push((next_position, card)),
            None => {
                issues.push(DeckIssue::UnknownToken { position: next_position, token: token.to_string() })
            }
        }
        next_position += 1;
    }
    next_position
}

/// Reports duplicates (by first and second position) and missing cards.
fn check_complete(cards: &[(usize, Card)], issues: &mut Vec<DeckIssue>) {
    let mut seen: [Option<usize>; DECK_SIZE] = [None; DECK_SIZE];
    for &(position, card) in cards {
        match seen[card.index() as usize] {
            Some(first) => issues.push(DeckIssue::Duplicate { card, first, second: position }),
            None => seen[card.index() as usize] = Some(position),
        }
    }
    let missing: Vec<Card> = Card::all().filter(|c| seen[c.index() as usize].is_none()).collect();
    if !missing.is_empty() {
        issues.push(DeckIssue::Missing { cards: missing });
    }
}

fn into_deal(cards: &[(usize, Card)], issues: Vec<DeckIssue>) -> Result<Deal, DeckError> {
    if !issues.is_empty() {
        return Err(DeckError { issues });
    }
    // No duplicates and nothing missing means exactly 52 cards.
    Ok(Deal::from_valid(std::array::from_fn(|i| cards[i].1)))
}

/// A deal from typed or pasted text.
pub fn parse_deck(text: &str) -> Result<Deal, DeckError> {
    let mut cards = Vec::with_capacity(DECK_SIZE);
    let mut issues = Vec::new();
    parse_tokens(tokens(text), 1, &mut cards, &mut issues);
    check_complete(&cards, &mut issues);
    into_deal(&cards, issues)
}

pub(crate) fn check_full_deck(cards: &[Card]) -> Result<Deal, DeckError> {
    let numbered: Vec<(usize, Card)> = cards.iter().enumerate().map(|(i, &c)| (i + 1, c)).collect();
    let mut issues = Vec::new();
    check_complete(&numbered, &mut issues);
    into_deal(&numbered, issues)
}

/// A game in progress: piles left to right, separated by `|`, each listed bottom to top.
/// All 52 cards must appear exactly once and no pile may be empty. Card positions count across piles.
pub fn parse_position(text: &str) -> Result<Vec<Vec<Card>>, DeckError> {
    let mut all = Vec::with_capacity(DECK_SIZE);
    let mut issues = Vec::new();
    let mut piles = Vec::new();
    let mut next_position = 1;
    for (p, segment) in text.split('|').enumerate() {
        let start = all.len();
        let before = next_position;
        next_position = parse_tokens(tokens(segment), next_position, &mut all, &mut issues);
        if next_position == before {
            issues.push(DeckIssue::EmptyPile { pile: p + 1 });
        }
        piles.push(all[start..].iter().map(|&(_, c)| c).collect::<Vec<Card>>());
    }
    check_complete(&all, &mut issues);
    if issues.is_empty() { Ok(piles) } else { Err(DeckError { issues }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical() -> String {
        Card::all().map(|c| c.to_string()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn accepts_mixed_separators_and_notations() {
        let messy = canonical().replace("TH", "10h").replace("KS", "K♠").replace(' ', ",\n ").to_lowercase();
        let deal = parse_deck(&messy).expect("valid");
        assert_eq!(deal.to_string(), canonical());
    }

    #[test]
    fn reports_every_problem_with_positions() {
        let mut cards: Vec<String> = Card::all().map(|c| c.to_string()).collect();
        cards[4] = "1H".into(); // unknown token at position 5 (5C now missing)
        cards[39] = cards[2].clone(); // 3C again at position 40 (AS now missing)
        let err = parse_deck(&cards.join(" ")).unwrap_err();
        assert_eq!(
            err.issues,
            vec![
                DeckIssue::UnknownToken { position: 5, token: "1H".into() },
                DeckIssue::Duplicate { card: "3C".parse().unwrap(), first: 3, second: 40 },
                DeckIssue::Missing { cards: vec!["5C".parse().unwrap(), "AS".parse().unwrap()] },
            ]
        );
    }

    #[test]
    fn short_deck_lists_missing_cards() {
        let text = canonical();
        let short = text.rsplit_once(' ').unwrap().0;
        let err = parse_deck(short).unwrap_err();
        assert_eq!(err.issues, vec![DeckIssue::Missing { cards: vec!["KS".parse().unwrap()] }]);
        assert_eq!(err.to_string(), "missing 1 card: KS");
    }

    #[test]
    fn positions_split_on_bars() {
        let mut text = canonical();
        text.insert_str(2, " |"); // "AC | 2C ..." : two piles
        let piles = parse_position(&text).unwrap();
        assert_eq!(piles.len(), 2);
        assert_eq!(piles[0], vec!["AC".parse::<Card>().unwrap()]);
        assert_eq!(piles[1].len(), 51);
        let err = parse_position(&format!("{text} |")).unwrap_err();
        assert_eq!(err.issues, vec![DeckIssue::EmptyPile { pile: 3 }]);
    }
}
