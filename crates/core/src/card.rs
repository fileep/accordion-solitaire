//! Cards. A card is one byte, `suit << 4 | rank`: rank 0–12 (A…K) in the low nibble and suit 0–3
//! (clubs, diamonds, hearts, spades) in bits 4–5. Two cards match when they share a suit or a rank,
//! which is two nibble tests on `a ^ b`. The text form is rank then suit, e.g. `TH` for the ten of hearts.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Rank characters, A (rank 0) through K (rank 12).
pub const RANKS: [char; 13] = ['A', '2', '3', '4', '5', '6', '7', '8', '9', 'T', 'J', 'Q', 'K'];
/// Suit characters: clubs, diamonds, hearts, spades (suits 0–3).
pub const SUITS: [char; 4] = ['C', 'D', 'H', 'S'];

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Card(u8);

impl Card {
    /// The card with suit 0–3 (C, D, H, S) and rank 0–12 (A…K).
    pub const fn new(suit: u8, rank: u8) -> Card {
        assert!(suit < 4 && rank < 13, "suit or rank out of range");
        Card(suit << 4 | rank)
    }

    /// Card number 0–51 in canonical order: clubs A…K, then diamonds, hearts, spades.
    pub const fn from_index(index: u8) -> Card {
        Card::new(index / 13, index % 13)
    }

    /// The card with this engine byte, if it is one (inverse of [`Card::code`]).
    pub const fn from_code(code: u8) -> Option<Card> {
        if code >> 4 < 4 && code & 0x0F < 13 { Some(Card(code)) } else { None }
    }

    /// The one-byte engine code, `suit << 4 | rank`.
    pub const fn code(self) -> u8 {
        self.0
    }

    pub const fn suit(self) -> u8 {
        self.0 >> 4
    }

    pub const fn rank(self) -> u8 {
        self.0 & 0x0F
    }

    /// Position 0–51 in canonical order (inverse of [`Card::from_index`]).
    pub const fn index(self) -> u8 {
        self.suit() * 13 + self.rank()
    }

    /// True when the cards share a suit or a rank.
    pub const fn matches(self, other: Card) -> bool {
        let x = self.0 ^ other.0;
        x & 0x0F == 0 || x & 0x70 == 0
    }

    /// All 52 cards in canonical order.
    pub fn all() -> impl Iterator<Item = Card> {
        (0..52).map(Card::from_index)
    }

    /// Parses one card leniently: case-insensitive, `10` for the ten, and suits as letters or symbols
    /// (`♣♦♥♠`, `♧♢♡♤`, with or without an emoji variation selector).
    pub fn parse(token: &str) -> Option<Card> {
        let mut chars: Vec<char> = token.trim().chars().filter(|&c| c != '\u{FE0F}').collect();
        let suit = match chars.pop()?.to_ascii_uppercase() {
            'C' | '♣' | '♧' => 0,
            'D' | '♦' | '♢' => 1,
            'H' | '♥' | '♡' => 2,
            'S' | '♠' | '♤' => 3,
            _ => return None,
        };
        let rank = match chars.as_slice() {
            ['1', '0'] => 9,
            [c] => RANKS.iter().position(|&r| r == c.to_ascii_uppercase())? as u8,
            _ => return None,
        };
        Some(Card::new(suit, rank))
    }
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", RANKS[self.rank() as usize], SUITS[self.suit() as usize])
    }
}

impl fmt::Debug for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseCardError(pub String);

impl fmt::Display for ParseCardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\"{}\" is not a card (expected a rank A, 2–9, T or 10, J, Q, K, then a suit C, D, H, S)",
            self.0
        )
    }
}

impl std::error::Error for ParseCardError {}

impl FromStr for Card {
    type Err = ParseCardError;

    fn from_str(s: &str) -> Result<Card, ParseCardError> {
        Card::parse(s).ok_or_else(|| ParseCardError(s.to_string()))
    }
}

impl Serialize for Card {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Card {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Card, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_for_every_card() {
        for card in Card::all() {
            assert_eq!(card.to_string().parse::<Card>(), Ok(card));
            assert_eq!(Card::from_index(card.index()), card);
        }
    }

    #[test]
    fn lenient_forms_parse_to_the_same_card() {
        let ten_of_hearts = Card::new(2, 9);
        for text in ["TH", "th", "10h", "10H", "T♥", "10♥", "t♡", "10♥\u{FE0F}", " TH "] {
            assert_eq!(Card::parse(text), Some(ten_of_hearts), "{text}");
        }
        for bad in ["", "H", "1H", "11H", "TX", "THH", "♥T", "ZZ"] {
            assert_eq!(Card::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn each_card_matches_exactly_fifteen_others() {
        for a in Card::all() {
            let n = Card::all().filter(|&b| b != a && a.matches(b)).count();
            assert_eq!(n, 15, "{a}");
        }
    }

    #[test]
    fn match_means_same_suit_or_rank() {
        for a in Card::all() {
            for b in Card::all() {
                assert_eq!(a.matches(b), a.suit() == b.suit() || a.rank() == b.rank());
            }
        }
    }
}
