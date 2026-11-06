//! Reference engine: piles as plain vectors, written to be obviously correct rather than fast.
//! It replays solve files for verification and is the oracle the fast engine (Phase 2) is tested against.

use crate::card::Card;
use crate::deal::Deal;
use std::fmt;

/// The pile at index `pile` (0-based, left to right) moves onto the pile 1 to its left (a slide)
/// or 3 to its left (a leap).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Move {
    pub pile: u8,
    pub leap: bool,
}

impl Move {
    pub const fn slide(pile: u8) -> Move {
        Move { pile, leap: false }
    }

    pub const fn leap(pile: u8) -> Move {
        Move { pile, leap: true }
    }

    pub const fn distance(self) -> usize {
        if self.leap { 3 } else { 1 }
    }

    /// Compact form: `pile << 1 | leap`.
    pub const fn to_byte(self) -> u8 {
        self.pile << 1 | self.leap as u8
    }

    pub const fn from_byte(byte: u8) -> Move {
        Move { pile: byte >> 1, leap: byte & 1 == 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveError {
    NoSuchPile { pile: usize, piles: usize },
    NothingToTheLeft { pile: usize, distance: usize },
    NoMatch { mover: Card, target: Card },
    NotATopCard(Card),
    WrongDistance { mover: Card, target: Card, distance: isize },
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveError::NoSuchPile { pile, piles } => {
                write!(f, "there is no pile {pile} (only {piles} piles)")
            }
            MoveError::NothingToTheLeft { pile, distance } => {
                write!(f, "pile {pile} has no pile {distance} to its left")
            }
            MoveError::NoMatch { mover, target } => {
                write!(f, "{mover} and {target} share neither suit nor rank")
            }
            MoveError::NotATopCard(card) => write!(f, "{card} is not the top card of any pile"),
            MoveError::WrongDistance { mover, target, distance } => match *distance {
                0 => write!(f, "{mover} can't move onto itself"),
                d if d < 0 => write!(f, "{target} is {} piles right of {mover}; piles only move left", -d),
                d => write!(f, "{target} is {d} piles left of {mover}; moves go exactly 1 or 3 piles left"),
            },
        }
    }
}

impl std::error::Error for MoveError {}

/// A game position: piles left to right, each listed bottom to top. Piles are never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    piles: Vec<Vec<Card>>,
}

impl Game {
    /// The starting position: one pile per card, in deal order.
    pub fn new(deal: &Deal) -> Game {
        Game { piles: deal.cards().iter().map(|&c| vec![c]).collect() }
    }

    /// A position from piles listed bottom to top. The caller guarantees 52 distinct cards and no empty
    /// pile, as [`crate::deck_input::parse_position`] does.
    pub fn from_piles(piles: Vec<Vec<Card>>) -> Game {
        assert!(piles.iter().all(|p| !p.is_empty()), "empty pile");
        Game { piles }
    }

    pub fn piles(&self) -> &[Vec<Card>] {
        &self.piles
    }

    pub fn pile_count(&self) -> usize {
        self.piles.len()
    }

    pub fn top(&self, pile: usize) -> Card {
        *self.piles[pile].last().expect("piles are never empty")
    }

    pub fn tops(&self) -> Vec<Card> {
        (0..self.piles.len()).map(|i| self.top(i)).collect()
    }

    pub fn is_won(&self) -> bool {
        self.piles.len() == 1
    }

    /// Whether `mv` is legal here, and why not if it isn't.
    pub fn check(&self, mv: Move) -> Result<(), MoveError> {
        let (pile, distance) = (mv.pile as usize, mv.distance());
        if pile >= self.piles.len() {
            return Err(MoveError::NoSuchPile { pile, piles: self.piles.len() });
        }
        if pile < distance {
            return Err(MoveError::NothingToTheLeft { pile, distance });
        }
        let (mover, target) = (self.top(pile), self.top(pile - distance));
        if !mover.matches(target) {
            return Err(MoveError::NoMatch { mover, target });
        }
        Ok(())
    }

    /// Every legal move, left to right, slide before leap.
    pub fn legal_moves(&self) -> Vec<Move> {
        (1..self.piles.len() as u8)
            .flat_map(|p| [Move::slide(p), Move::leap(p)])
            .filter(|&mv| self.check(mv).is_ok())
            .collect()
    }

    /// Plays `mv`: the whole pile goes on top of its target and the gap closes.
    pub fn apply(&mut self, mv: Move) -> Result<(), MoveError> {
        self.check(mv)?;
        let pile = mv.pile as usize;
        let moving = self.piles.remove(pile);
        self.piles[pile - mv.distance()].extend(moving);
        Ok(())
    }

    /// Index of the pile whose top card is `card`.
    pub fn pile_of(&self, card: Card) -> Option<usize> {
        self.piles.iter().position(|p| p.last() == Some(&card))
    }

    /// The legal move written `mover>target` in card notation.
    pub fn move_between(&self, mover: Card, target: Card) -> Result<Move, MoveError> {
        let from = self.pile_of(mover).ok_or(MoveError::NotATopCard(mover))?;
        let to = self.pile_of(target).ok_or(MoveError::NotATopCard(target))?;
        let mv = match from as isize - to as isize {
            1 => Move::slide(from as u8),
            3 => Move::leap(from as u8),
            distance => return Err(MoveError::WrongDistance { mover, target, distance }),
        };
        self.check(mv)?;
        Ok(mv)
    }

    /// Card notation for a legal move: the mover's and the target's top cards.
    pub fn cards_of(&self, mv: Move) -> (Card, Card) {
        let pile = mv.pile as usize;
        (self.top(pile), self.top(pile - mv.distance()))
    }
}

/// Plays `line` from `deal` by the rules of [`Game::check`] and [`Game::apply`] on the top cards alone, which is all
/// those rules look at, and returns the piles left; or the index of the first illegal move and why. Allocates nothing
/// where a [`Game`] allocates once per pile and again per move: `acc batch` replays every GPU line with this, up to
/// half a million a second. A test checks it against [`Game`], on legal moves and illegal ones.
pub fn replay_tops(deal: &Deal, line: &[Move]) -> Result<usize, (usize, MoveError)> {
    let mut tops = *deal.cards();
    let mut piles = tops.len();
    for (i, &mv) in line.iter().enumerate() {
        let (pile, distance) = (mv.pile as usize, mv.distance());
        let error = if pile >= piles {
            MoveError::NoSuchPile { pile, piles }
        } else if pile < distance {
            MoveError::NothingToTheLeft { pile, distance }
        } else if !tops[pile].matches(tops[pile - distance]) {
            MoveError::NoMatch { mover: tops[pile], target: tops[pile - distance] }
        } else {
            tops[pile - distance] = tops[pile];
            tops.copy_within(pile + 1..piles, pile);
            piles -= 1;
            continue;
        };
        return Err((i, error));
    }
    Ok(piles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards(text: &str) -> Vec<Card> {
        text.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }

    #[test]
    fn slides_and_leaps_move_whole_piles() {
        // AC 2C 3D 4C: 4C may leap onto AC (same suit), 2C slides onto AC.
        let mut g = Game::from_piles(cards("AC 2C 3D 4C").into_iter().map(|c| vec![c]).collect());
        assert_eq!(g.legal_moves(), vec![Move::slide(1), Move::leap(3)]);
        g.apply(Move::leap(3)).unwrap();
        assert_eq!(g.tops(), cards("4C 2C 3D"));
        g.apply(Move::slide(1)).unwrap();
        assert_eq!(g.piles()[0], cards("AC 4C 2C"));
        assert_eq!(g.tops(), cards("2C 3D"));
    }

    #[test]
    fn illegal_moves_are_explained() {
        let g = Game::from_piles(cards("AC 2D 3H 4S").into_iter().map(|c| vec![c]).collect());
        assert_eq!(g.check(Move::slide(0)), Err(MoveError::NothingToTheLeft { pile: 0, distance: 1 }));
        assert_eq!(g.check(Move::leap(2)), Err(MoveError::NothingToTheLeft { pile: 2, distance: 3 }));
        assert_eq!(g.check(Move::slide(9)), Err(MoveError::NoSuchPile { pile: 9, piles: 4 }));
        let (ac, d2, h3) = ("AC".parse().unwrap(), "2D".parse().unwrap(), "3H".parse().unwrap());
        assert_eq!(g.check(Move::slide(1)), Err(MoveError::NoMatch { mover: d2, target: ac }));
        assert_eq!(
            g.move_between(h3, ac),
            Err(MoveError::WrongDistance { mover: h3, target: ac, distance: 2 })
        );
    }

    #[test]
    fn replaying_top_cards_agrees_with_the_game() {
        // Random lines, most moves legal and some not: the same piles left, or the same first illegal move and reason.
        let mut rng = crate::rng::Xoshiro256StarStar::from_seed(1);
        let mut illegal = 0;
        for seed in 0..2000 {
            let deal = Deal::from_seed(seed);
            let mut game = Game::new(&deal);
            let mut line = Vec::new();
            loop {
                let legal = game.legal_moves();
                let mv = if legal.is_empty() || rng.below(25) == 0 {
                    Move { pile: rng.below(53) as u8, leap: rng.below(2) == 1 }
                } else {
                    legal[rng.below(legal.len() as u64) as usize]
                };
                line.push(mv);
                match game.apply(mv) {
                    Ok(()) => assert_eq!(replay_tops(&deal, &line), Ok(game.pile_count())),
                    Err(e) => {
                        assert_eq!(replay_tops(&deal, &line), Err((line.len() - 1, e)));
                        illegal += 1;
                        break;
                    }
                }
                if game.is_won() {
                    break;
                }
            }
        }
        assert!(illegal > 1000, "only {illegal} lines ended on an illegal move");
    }

    #[test]
    fn compact_bytes_round_trip() {
        for pile in 0..52 {
            for mv in [Move::slide(pile), Move::leap(pile)] {
                assert_eq!(Move::from_byte(mv.to_byte()), mv);
            }
        }
    }
}
