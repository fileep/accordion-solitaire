/-
  The second generator: ChaCha20 (RFC 8439) keyed by the seed, an index drawn by masking a byte and redrawing, and the same
  Fisher–Yates shuffle as `deal`. It is `ChaCha20` in crates/core/src/rng.rs, used by builds with the `second-generator`
  feature, and docs/FORMATS.md ("The second generator") specifies it.

  `rfc8439_block` checks the block function against the test vector of RFC 8439, section 2.3.2. A theorem about a seed
  computes `ChaCha.deal seed` itself, so it can only hold if this text gives the deal the certificate was built for.
  Core Lean 4 only, and no `sorry`.
-/
import Accordion

set_option autoImplicit false

namespace Accordion
namespace ChaCha

/-- Arithmetic on 32-bit words, as plain `Nat`, like Part 2 of Accordion.lean. -/
def w32 (x : Nat) : Nat := x % 4294967296
def rotl32 (x k : Nat) : Nat := w32 (Nat.lor (Nat.shiftLeft x k) (Nat.shiftRight x (32 - k)))

/-- The quarter round of RFC 8439, section 2.1, on words a, b, c, d of the 16-word state. -/
def quarter (s : List Nat) (a b c d : Nat) : List Nat :=
  let sa := w32 (s.getD a 0 + s.getD b 0)
  let sd := rotl32 (Nat.xor (s.getD d 0) sa) 16
  let sc := w32 (s.getD c 0 + sd)
  let sb := rotl32 (Nat.xor (s.getD b 0) sc) 12
  let sa := w32 (sa + sb)
  let sd := rotl32 (Nat.xor sd sa) 8
  let sc := w32 (sc + sd)
  let sb := rotl32 (Nat.xor sb sc) 7
  (((s.set a sa).set b sb).set c sc).set d sd

/-- A column round, then a diagonal round. -/
def doubleRound (s : List Nat) : List Nat :=
  let s := quarter s 0 4 8 12
  let s := quarter s 1 5 9 13
  let s := quarter s 2 6 10 14
  let s := quarter s 3 7 11 15
  let s := quarter s 0 5 10 15
  let s := quarter s 1 6 11 12
  let s := quarter s 2 7 8 13
  quarter s 3 4 9 14

def rounds : Nat → List Nat → List Nat
  | 0, s => s
  | n + 1, s => rounds n (doubleRound s)

/-- The block function of RFC 8439, section 2.3: eight key words, a counter and three nonce words give 16 words. -/
def block (key : List Nat) (counter : Nat) (nonce : List Nat) : List Nat :=
  let start := [0x61707865, 0x3320646E, 0x79622D32, 0x6B206574] ++ key ++ [counter] ++ nonce
  List.zipWith (fun x y => w32 (x + y)) (rounds 10 start) start

theorem rfc8439_block :
    block [0x03020100, 0x07060504, 0x0B0A0908, 0x0F0E0D0C, 0x13121110, 0x17161514, 0x1B1A1918, 0x1F1E1D1C] 1
      [0x09000000, 0x4A000000, 0] =
    [0xE4E7F110, 0x15593BD1, 0x1FDD0F50, 0xC47120A3, 0xC7F4D1C7, 0x0368C033, 0x9AAA2204, 0x4E6CD4C3,
     0x466482D2, 0x09AA9F07, 0x05D7C214, 0xA2028BD9, 0xD19C12B5, 0xB94E16DE, 0xE883D0CB, 0x4E3C50A2] := by
  decide +kernel

/-- A block's 64 bytes: word 0 first, each word low byte first. -/
def bytes : List Nat → List Nat
  | [] => []
  | w :: ws => w % 256 :: w / 256 % 256 :: w / 65536 % 256 :: w / 16777216 % 256 :: bytes ws

/-- The byte stream of a seed: the key is the seed's low and high 32 bits and six zero words, the nonce is zero, and the
counter counts blocks from 0. `left` holds the unread bytes of the current block. -/
structure Stream where
  (key : List Nat) (counter : Nat) (left : List Nat)

def Stream.seed (seed : Nat) : Stream :=
  ⟨[seed % 4294967296, seed / 4294967296 % 4294967296, 0, 0, 0, 0, 0, 0], 0, []⟩

def Stream.byte (x : Stream) : Nat × Stream :=
  match x.left with
  | b :: rest => (b, ⟨x.key, x.counter, rest⟩)
  | [] =>
    match bytes (block x.key x.counter [0, 0, 0]) with
    | b :: rest => (b, ⟨x.key, x.counter + 1, rest⟩)
    | [] => (0, x)

/-- 2^b − 1, where b is the number of bits of n − 1, for 1 ≤ n ≤ 256. -/
def mask (n : Nat) : Nat :=
  if n ≤ 1 then 0 else if n ≤ 2 then 1 else if n ≤ 4 then 3 else if n ≤ 8 then 7 else if n ≤ 16 then 15
  else if n ≤ 32 then 31 else if n ≤ 64 then 63 else if n ≤ 128 then 127 else 255

/-- A uniform index below n: mask a byte, and redraw until it is below n. With fuel: a deal needs at most a few redraws in
a row, each with chance under 1/2, so 64 is never used up on a real seed, and if it were the deal would differ and the
proof would fail. -/
def Stream.below (x : Stream) (n : Nat) : Nat × Stream :=
  let rec loop : Nat → Stream → Nat × Stream
    | 0, x => (0, x)
    | f + 1, x =>
      let (b, x') := x.byte
      let v := Nat.land b (mask n)
      if v < n then (v, x') else loop f x'
  loop 64 x

/-- Fisher–Yates from the last position down, as `shuffle` in Accordion.lean. -/
def shuffle : Nat → Stream → List Nat → List Nat
  | 0, _, l => l
  | i + 1, x, l =>
    let (j, x') := x.below (i + 2)
    shuffle i x' (swap l (i + 1) j)

def deal (seed : Nat) : List Card := shuffle 51 (Stream.seed seed) (List.range 52)

end ChaCha
end Accordion
