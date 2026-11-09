//! Deals: the 52 cards in one row, left to right as dealt.

use crate::card::Card;
use crate::deck_input::{self, DeckError};
#[cfg(feature = "second-generator")]
use crate::rng::ChaCha20 as DealRng;
#[cfg(not(feature = "second-generator"))]
use crate::rng::Xoshiro256StarStar as DealRng;
use std::fmt;

pub const DECK_SIZE: usize = 52;

/// The generator behind [`Deal::from_seed`]. Deals of the same seed differ between the two, so their results mustn't be mixed.
pub const DEAL_GENERATOR: &str = if cfg!(feature = "second-generator") { "ChaCha20" } else { "xoshiro256**" };

/// A full deck in deal order. Always exactly the 52 distinct cards.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Deal([Card; DECK_SIZE]);

impl Deal {
    /// The deal for `seed`: canonical order (see [`Card::from_index`]) shuffled by Fisher–Yates, from the last
    /// position down, with xoshiro256** seeded through SplitMix64 (docs/FORMATS.md). The `second-generator` feature
    /// deals from ChaCha20 instead, to test that the win rate doesn't depend on the generator.
    pub fn from_seed(seed: u64) -> Deal {
        let mut rng = DealRng::from_seed(seed);
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

    /// This deal with suits and ranks renamed in order of first appearance, left to right: the first card becomes
    /// the ace of clubs, the first new suit after it diamonds, the first new rank a two, and so on. Matching only
    /// compares suits and ranks, so two deals play alike, move for move, exactly when these are equal.
    pub fn relabelled(&self) -> Deal {
        let (mut suits, mut ranks) = ([u8::MAX; 4], [u8::MAX; 13]);
        let (mut next_suit, mut next_rank) = (0, 0);
        Deal(self.0.map(|card| {
            let suit = &mut suits[card.suit() as usize];
            if *suit == u8::MAX {
                (*suit, next_suit) = (next_suit, next_suit + 1);
            }
            let rank = &mut ranks[card.rank() as usize];
            if *rank == u8::MAX {
                (*rank, next_rank) = (next_rank, next_rank + 1);
            }
            Card::new(*suit, *rank)
        }))
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

    #[test]
    fn relabelling_ignores_names_but_not_play() {
        let d = Deal::from_seed(7);
        let r = d.relabelled();
        assert_eq!(r.cards()[0], Card::new(0, 0));
        assert_eq!(r.relabelled(), r);
        assert!(deck_input::check_full_deck(r.cards()).is_ok());
        // Same pattern of matches, card by card.
        for (i, j) in (0..52).flat_map(|i| (0..52).map(move |j| (i, j))) {
            assert_eq!(d.cards()[i].matches(d.cards()[j]), r.cards()[i].matches(r.cards()[j]));
        }
        // Renaming suits (hearts and spades swapped) and ranks (reversed) gives the same game.
        let renamed = Deal(d.0.map(|c| Card::new([0, 1, 3, 2][c.suit() as usize], 12 - c.rank())));
        assert_ne!(renamed, d);
        assert_eq!(renamed.relabelled(), r);
        assert_ne!(Deal::from_seed(8).relabelled(), r);
    }
}
