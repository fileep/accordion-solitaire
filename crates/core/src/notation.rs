//! Move notation. Text form `7H>7C`: the pile whose top card is 7H moves onto the pile whose top card is 7C.
//! Top cards are unique, so this names a move without any pile numbering. Compact form for batch files:
//! one byte per move (`pile << 1 | leap`), encoded as standard padded base64 (RFC 4648 §4).

use crate::card::Card;
use crate::game::Move;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CardMove {
    pub mover: Card,
    pub target: Card,
}

impl CardMove {
    pub fn parse(text: &str) -> Result<CardMove, String> {
        let (mover, target) = text
            .split_once('>')
            .ok_or_else(|| format!("\"{text}\" is not a move (expected mover>target, e.g. 7H>7C)"))?;
        let card = |t: &str| t.parse::<Card>().map_err(|e| format!("in move \"{text}\": {e}"));
        Ok(CardMove { mover: card(mover)?, target: card(target)? })
    }
}

impl fmt::Display for CardMove {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}>{}", self.mover, self.target)
    }
}

pub fn encode_compact(moves: &[Move]) -> String {
    STANDARD.encode(moves.iter().map(|m| m.to_byte()).collect::<Vec<u8>>())
}

pub fn decode_compact(text: &str) -> Result<Vec<Move>, String> {
    let bytes = STANDARD.decode(text.trim()).map_err(|e| format!("moves_b64 is not valid base64: {e}"))?;
    bytes
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let mv = Move::from_byte(b);
            if (mv.pile as usize) < 52 {
                Ok(mv)
            } else {
                Err(format!("moves_b64 move {}: pile {} out of range", i + 1, mv.pile))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_moves_round_trip_and_accept_lenient_cards() {
        let m = CardMove::parse("10h>th").unwrap();
        assert_eq!(m.to_string(), "TH>TH");
        assert_eq!(CardMove::parse("7H>7C").unwrap().to_string(), "7H>7C");
        assert!(CardMove::parse("7H-7C").is_err());
        assert!(CardMove::parse("7H>ZZ").is_err());
    }

    #[test]
    fn compact_round_trips() {
        let moves = vec![Move::slide(1), Move::leap(51), Move::slide(3)];
        let text = encode_compact(&moves);
        assert_eq!(decode_compact(&text).unwrap(), moves);
        assert!(decode_compact("not base64!").is_err());
        assert!(decode_compact(&STANDARD.encode([200u8])).is_err());
    }
}
