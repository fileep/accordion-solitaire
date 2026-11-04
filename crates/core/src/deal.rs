//! Deals: the 52 cards in one row, left to right as dealt.

use crate::card::Card;
use crate::deck_input::{self, DeckError};
use crate::rng::Xoshiro256StarStar;
use std::fmt;

pub const DECK_SIZE: usize = 52;

/// A full deck in deal order. Always exactly the 52 distinct cards.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Deal([Card; DECK_SIZE]);

impl Deal {
    /// The deal for `seed`: canonical order (see [`Card::from_index`]) shuffled by Fisher–Yates, from the last
    /// position down, with xoshiro256** seeded through SplitMix64 (docs/FORMATS.md).
    pub fn from_seed(seed: u64) -> Deal {
        let mut rng = Xoshiro256StarStar::from_seed(seed);
        let mut cards: [Card; DECK_SIZE] = std::array::from_fn(|i| Card::from_index(i as u8));
        for i in (1..DECK_SIZE).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            cards.swap(i, j);
        }
        Deal(cards)
    }

    /// A deal from cards in order, rejecting anything but the 52 distinct cards.
    pub fn from_cards(cards: &[Card]) -> Result<Deal, DeckError> {
        deck_input::check_full_deck(cards)
    }

    /// A deal from typed or pasted text (docs/FORMATS.md "Deck input").
    pub fn parse(text: &str) -> Result<Deal, DeckError> {
        deck_input::parse_deck(text)
    }

    /// Caller guarantees `cards` holds each card exactly once.
    pub(crate) fn from_valid(cards: [Card; DECK_SIZE]) -> Deal {
        Deal(cards)
    }

    pub fn cards(&self) -> &[Card; DECK_SIZE] {
        &self.0
    }
}

impl fmt::Display for Deal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, card) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{card}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Deal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Deal({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_deals_are_permutations_and_differ() {
        let a = Deal::from_seed(1);
        let mut sorted: Vec<u8> = a.cards().iter().map(|c| c.index()).collect();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..52).collect::<Vec<u8>>());
        assert_eq!(a, Deal::from_seed(1));
        assert_ne!(a, Deal::from_seed(2));
    }

    #[test]
    fn display_parses_back() {
        let d = Deal::from_seed(99);
        assert_eq!(Deal::parse(&d.to_string()), Ok(d));
    }
}
