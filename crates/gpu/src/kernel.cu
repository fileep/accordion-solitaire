// Ladder attempts on the GPU (PLAN.md Phase 5 track F, Phase 8): one attempt per thread. An attempt plays `prefix`
// random moves on its position, then a first-win depth-first search of what is left (at most 16 piles) with sleep
// sets and the connectivity dead-end test, up to `budget` positions. The rules are the Rust fast engine's
// (crates/core/src/fast.rs); every win is replayed by the CPU's reference rules before it counts, and the
// `playouts` kernel at the end lets the tests (crates/gpu/tests) check this rules code against the Rust engines.
//
// Everything stays on chip. The prefix plays on a 64-byte row in shared memory (stored byte-major, so a warp's 32
// threads touch 32 different banks; in attempts_split, on 13 words in registers), starting from legal-move masks the
// host works out once per position. The search keeps its at most 16 cards in three 32-bit words of bit planes (each
// holds two 16-bit planes of the cards' 6-bit codes, so one shift and xor compares the whole row with itself: bp_match),
// and undoes moves instead of copying positions. Its moves and covered cards are byte stacks in registers, and so are
// the current depth's asleep moves; shared memory holds only the asleep moves of the depths above it. Choosing the k-th
// legal move halves the mask by popcount (no loop that runs k times), and the dead-end test's trigger looks at all four
// suits at once in 16-bit lanes.
//
// Shared memory per thread decides how many attempts a multiprocessor holds at once, so the kernel keeps only 64 bytes
// there, enough for 1,024 attempts per multiprocessor on the T550 (its maximum). The untried moves at a depth are
// always its legal moves minus its asleep ones (a move tried there joins the asleep set), so they live in registers for
// the current depth and are recomputed when the search backs up. The row and the asleep moves share the same bytes
// (the row is done with once the residual is in registers), and the prefix's moves are not kept: a winning attempt
// replays its prefix from the same random numbers to write its line (in attempts_split, `write_lines` does so for all of
// a launch's wins after it).
//
// Threads are persistent: each keeps taking the next attempt from a global counter. Attempt i is try i / deals of
// deal i % deals, so a deal's tries are spread over the run; once some thread has won a deal, its later tries are
// skipped. Two rungs share one launch: the first `tries` tries of a deal use `prefix` random moves, the next
// `tries2` use `prefix2`.
//
// `attempts_split` gives the prefixes and the searches warps of their own, joined by a queue of residuals in shared
// memory, keeps the prefix's row in registers, lets groups of a deal's tries share their first prefix moves, runs the
// prefix warps in lockstep, keeps its hot code small, and doesn't choose its prefix moves blindly (docs/BENCHMARKS.md:
// "split warps", "the prefix row in registers", "shared early prefix moves", "prefix warps in lockstep", "the search
// step, and what code size costs" and "what winning prefixes have in common"). It replaced `attempts`, each thread on
// its own attempts start to finish, and `attempts_stepwise`, every thread a step at a time (1.7 times the deals per
// second of `attempts`, and many times fewer than `attempts_split`); their code and Settings are in the history of
// this file (git log -S attempts_stepwise), and BENCHMARKS.md ("After Phase 8 — where the GPU's time goes" and
// "the GPU's bit operations") has their numbers.
//
// A position is 52 bytes of engine codes (suit << 4 | rank), SENTINEL after its last pile; a deal fills all 52.

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;

#ifndef BLOCK
#define BLOCK 64
#endif
#define SENTINEL 0x7F
#define OFF 3
#define ROW 0x1FFFull
#define RES 16            // residual piles a search can hold (16-bit planes)
#define MAX_PREFIX 48

// Four cards at once (bytes of a word, each a card code or SENTINEL, all below 0x80): 0x80 in each byte where a and b
// share a suit or a rank. A byte of (d & 0x0F) + 0x7F has its top bit set when the ranks differ, and of
// (d & 0x70) + 0x7F when the suits differ; neither sum carries into the next byte. Two sentinels match.
__device__ __forceinline__ u32 match4(u32 a, u32 b) {
    u32 d = a ^ b;
    return ~(((d & 0x0F0F0F0Fu) + 0x7F7F7F7Fu) & ((d & 0x70707070u) + 0x7F7F7F7Fu)) & 0x80808080u;
}
// The top bits of m's four bytes (each 0x80 or 0) as four bits, byte 0 lowest: one multiply puts bits 7, 15, 23
// and 31 at bits 28 to 31, with nothing else landing there.
__device__ __forceinline__ u32 gather4(u32 m) { return (m * 0x00204081u) >> 28; }
// Bit i: card i of the words (w0 lowest) matches card i - 1 (d = 8) or card i - 3 (d = 24), with `before` the word
// before w0. 16 cards.
__device__ __forceinline__ u32 match16(u32 before, u32 w0, u32 w1, u32 w2, u32 w3, int d) {
    return gather4(match4(w0, __funnelshift_l(before, w0, d))) | gather4(match4(w1, __funnelshift_l(w0, w1, d))) << 4
           | gather4(match4(w2, __funnelshift_l(w1, w2, d))) << 8 | gather4(match4(w3, __funnelshift_l(w2, w3, d))) << 12;
}

__device__ __forceinline__ bool codes_match(u32 a, u32 b) {
    u32 x = a ^ b;
    return ((x & 0x0F) == 0) | ((x & 0x70) == 0);
}

// xorshift32: 32-bit operations only (64-bit multiplies take several instructions on the GPU), and good enough for move
// choice (the CPU checks every result). The state must not be zero.
__device__ __forceinline__ u32 next_u32(u32& s) {
    s ^= s << 13; s ^= s >> 17; s ^= s << 5;
    return s;
}
__device__ __forceinline__ u32 below(u32& s, u32 n) { return __umulhi(next_u32(s), n); }
// A 32-bit starting state from a 64-bit seed (splitmix64's finalizer), never zero.
__device__ __forceinline__ u32 seed_u32(u64 z) {
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return (u32)(z ^ (z >> 31)) | 1u;
}
// The position of the k-th set bit of m (k from 0; m has more than k), by halving: if the low half of the bits
// still to search holds no more than k set bits, skip it. Every thread takes the same few steps, where clearing the
// lowest bit k times made a warp wait for its largest k (branch-free, the kernel ran 13% faster).
// `w` is the width searched: 16 for the search's masks (at most RES piles), 32 for a word.
template <int W>
__device__ __forceinline__ int select_bits(u32 m, u32 k) {
    int pos = 0;
    #pragma unroll
    for (int w = W / 2; w >= 1; w >>= 1) {
        u32 c = __popc(m & ((1u << w) - 1));
        bool up = k >= c;
        k -= up ? c : 0;
        m = up ? m >> w : m;
        pos += up ? w : 0;
    }
    return pos;
}
__device__ __forceinline__ int select_bit64(u64 m, u32 k) {
    u32 lo = (u32)m, c = __popc(lo);
    bool up = k >= c;     // one copy of the steps: written as a choice of two copies, it compiled to a branch in every draw
    return (up ? 32 : 0) + select_bits<32>(up ? (u32)(m >> 32) : lo, up ? k - c : k);
}

// Covering `covered` broke the last rank its suit shared with another suit (fast.rs breaks_link): some other suit
// has the covered card's rank and now shares no rank with the covered card's suit. All four suits at once, one per
// 16-bit lane of `present`: the suit's row copied into every lane by one multiply, and "shares a rank" read from bit
// 15 of each lane after adding 0x7FFF (a 13-bit lane can't carry into the next).
__device__ __forceinline__ bool breaks_link(u64 present, u32 covered) {
    u32 s = covered >> 4, r = covered & 0x0F;
    u64 row = (present >> (16 * s)) & ROW;
    u64 shares = ((present & (row * 0x0001000100010001ull)) + 0x7FFF7FFF7FFF7FFFull) & 0x8000800080008000ull;
    u64 has_rank = ((present >> r) & 0x0001000100010001ull) << 15;
    return (has_rank & ~shares & ~(0x8000ull << (16 * s))) != 0 && row != 0;
}

// fast.rs connected: the suits present are linked, two suits when they share a rank. Branch-free, so a warp never
// waits for its slowest thread (the search loop it replaced, one suit a turn until nothing changed, cost about 18%
// of the kernel's time). About 40 operations, against 55 for three steps out from the lowest suit present.
__device__ __forceinline__ bool connected(u64 present) {
    const u32 K = 0x7FFF7FFFu, H = 0x80008000u;
    u32 p0 = (u32)present, p1 = (u32)(present >> 32);
    // The ranks each pair of suits shares, two pairs to a word, and whether it shares any: the carry out of its lane.
    u32 a = (p0 & p1) + K, b = (p0 & __funnelshift_l(p1, p1, 16)) + K, c = ((p0 & (p0 >> 16)) | (p1 & (p1 << 16))) + K;
    u32 w = ((a & H) >> 15) | ((b & H) >> 14) | ((c & H) >> 13);
    u32 links = (w | (w >> 13)) & 0x3F;
    // Those 6 bits say into how many pieces the links part the 4 suits, less one, in 2 bits; a suit that is absent is a
    // piece of its own, so the suits present are linked when pieces and suits present come to 5.
    u32 pieces = (links & 32) ? ((links & 16) ? 0x10001u : 0x10156u) : ((links & 16) ? 0x10516u : 0x1116166Bu);
    u32 suits = ((p0 + K) & H) | (((p1 + K) & H) >> 1);
    return ((pieces >> (2 * (links & 15))) & 3) + __popc(suits) == 4;
}

// fast.rs connected_without_leftmost: pile 0 never moves, so the other top cards must be connected among themselves
// and pile 0's card `left` must match one of them.
// Whichever test `leftmost` asks for, with one copy of the code.
__device__ __forceinline__ bool connected_test(u64 present, u32 left, int len, u32 leftmost) {
    u64 rest = leftmost ? present & ~(1ull << left) : present;
    u32 r0 = (u32)rest, r1 = (u32)(rest >> 32), rank = left & 0x0F;
    bool matched = !leftmost || ((((left & 32) ? r1 : r0) >> (left & 16)) & 0x1FFFu) != 0 || (((r0 >> rank) | (r1 >> rank)) & 0x00010001u) != 0;
    return (len < 2) | (matched & connected(rest));
}
__device__ __forceinline__ bool connected_without_leftmost(u64 present, u32 left, int len) {
    u64 rest = present & ~(1ull << left);
    u32 r0 = (u32)rest, r1 = (u32)(rest >> 32), rank = left & 0x0F;
    bool matched = ((((left & 32) ? r1 : r0) >> (left & 16)) & 0x1FFFu) != 0 || (((r0 >> rank) | (r1 >> rank)) & 0x00010001u) != 0;
    return (len < 2) | (matched & connected(rest));
}

// Settings::leftmost and conn_gate for the search in attempts_split, where a runtime choice takes a branch every step.
#ifndef LEFTMOST
#define LEFTMOST 1
#endif
#ifndef CONN_GATE
#define CONN_GATE 19
#endif
// A card's bit in the cards present, without a 64-bit shift by a varying amount.
__device__ __forceinline__ u64 card_bit(u32 c) {
    u32 b = 1u << (c & 31);
    return (u64)((c & 32) ? b : 0u) << 32 | ((c & 32) ? 0u : b);
}
// connected_test for a position of at least two piles, with no branches and no 64-bit shifts.
__device__ __forceinline__ bool alive(u64 present, u32 left) {
    u64 rest = LEFTMOST ? present & ~card_bit(left) : present;
    u32 r0 = (u32)rest, r1 = (u32)(rest >> 32), rank = left & 0x0F;
    u32 mates = (((left & 32) ? r1 : r0) >> (left & 16) & 0x1FFFu) | (((r0 >> rank) | (r1 >> rank)) & 0x00010001u);
    return (!LEFTMOST | (mates != 0)) & connected(rest);
}

// ---- The row: piles 0.. at row[OFF..], SENTINEL around them, and the legal-move masks l1 (slides) and l3 (leaps).

// Loads a position onto the row; returns its pile count and sets `present` and the masks, which the host worked
// out once per position (`m`: slides, leaps, cards present; a deal is tried many times). The 52 bytes come in as 13
// words loaded at once.
__device__ __forceinline__ int row_load(u8 (*row)[BLOCK], int t, const u8* cards, const u64* m, u64& present, u64& l1,
                                        u64& l3) {
    u32 w[13];
    #pragma unroll
    for (int j = 0; j < 13; j++) w[j] = __ldg((const u32*)cards + j);
    l1 = __ldg(m); l3 = __ldg(m + 1); present = __ldg(m + 2);
    #pragma unroll
    for (int i = 0; i < OFF; i++) row[i][t] = SENTINEL;
    #pragma unroll
    for (int i = 0; i < 52; i++) row[OFF + i][t] = (u8)(w[i >> 2] >> (8 * (i & 3)));
    #pragma unroll
    for (int i = OFF + 52; i < 64; i++) row[i][t] = SENTINEL;
    return __popcll(present);
}

// Attempt a's deal and try: a / n by multiplying by inv = ceil(2^64 / n) (exact for a below 2^32), not dividing.
__device__ __forceinline__ void deal_and_try(u32 a, u32 n, u64 inv, u32& deal, u32& attempt_try) {
    attempt_try = n == 1 ? a : (u32)__umul64hi(a, inv);
    deal = a - attempt_try * n;
}

// Plays a legal move on the row and updates the masks incrementally (fast.rs apply); returns the covered card.
__device__ __forceinline__ u32 row_move(u8 (*row)[BLOCK], int t, int& len, u64& l1, u64& l3, int pile, bool leap) {
    int d = leap ? 3 : 1;
    u32 mover = row[OFF + pile][t], covered = row[OFF + pile - d][t];
    row[OFF + pile - d][t] = mover;
    for (int i = OFF + pile; i < OFF + len; i++) row[i][t] = row[i + 1][t];
    len--;
    #define SB(j) ((u64)codes_match(row[OFF + (j)][t], row[OFF + (j) - 1][t]) << (j))
    #define LB(j) ((u64)codes_match(row[OFF + (j)][t], row[OFF + (j) - 3][t]) << (j))
    u64 low = (1ull << pile) - 1;
    u64 m1 = (l1 & low) | ((l1 >> 1) & ~low), m3 = (l3 & low) | ((l3 >> 1) & ~low);
    u64 w1, b1, w3, b3;
    if (leap) {
        w1 = 0b1011ull << (pile - 3); b1 = SB(pile - 3) | SB(pile - 2) | SB(pile);
        w3 = 0b111001ull << (pile - 3); b3 = LB(pile - 3) | LB(pile) | LB(pile + 1) | LB(pile + 2);
    } else {
        w1 = 1ull << (pile - 1); b1 = SB(pile - 1);
        w3 = 0b111ull << (pile - 1); b3 = LB(pile - 1) | LB(pile) | LB(pile + 1);
    }
    #undef SB
    #undef LB
    u64 valid = (1ull << len) - 1;
    l1 = ((m1 & ~w1) | b1) & valid & ~1ull;
    l3 = ((m3 & ~w3) | b3) & valid & ~7ull;
    return covered;
}

// ---- The prefix row in registers (attempts_split's prefix warps): card i is byte i % 4 of R[i / 4], SENTINEL after
// the last pile. Registers can't be indexed at run time, so every access is unrolled over the words: a move closes its
// gap with a funnel shift of each word, and the cards around it come from a window of three words picked out by a
// barrel shift. Nothing goes through shared memory, and no thread waits for another's longer shift
// (docs/BENCHMARKS.md, "the prefix row in registers"). A row of at most 4W cards needs only the first W words, and one
// of at most 32 cards fits its masks in 32 bits: W and the mask type M say how much of the row a move touches.
#define RW 13
#define SENT4 0x7F7F7F7Fu

__device__ __forceinline__ u32 count_bits(u32 m) { return __popc(m); }
__device__ __forceinline__ u32 count_bits(u64 m) { return __popcll(m); }
__device__ __forceinline__ int select_bit(u32 m, u32 k) { return select_bits<32>(m, k); }
__device__ __forceinline__ int select_bit(u64 m, u32 k) { return select_bit64(m, k); }

// row_load into registers; returns the piles.
__device__ __forceinline__ int rrow_load(u32 (&R)[RW], const u8* cards, const u64* m, u64& l1, u64& l3) {
    #pragma unroll
    for (int j = 0; j < RW; j++) R[j] = __ldg((const u32*)cards + j);
    l1 = __ldg(m); l3 = __ldg(m + 1);
    return __popcll(__ldg(m + 2));
}

// The cards present among the row's first 16 places (all of a residual): SENTINEL lands on bit 63, which no card has.
__device__ __forceinline__ u64 rrow_present(const u32 (&R)[RW]) {
    u64 p = 0;
    #pragma unroll
    for (int i = 0; i < 16; i++) p |= 1ull << ((R[i >> 2] >> (8 * (i & 3))) & 63);
    return p & ~(1ull << 63);
}

// Word p of the row's first W with two SENTINEL words before them and after them (p is a constant once unrolled).
template <int W>
__device__ __forceinline__ u32 rrow_padded(const u32 (&R)[RW], int p) {
    return p >= 2 && p < W + 2 ? R[p >= 2 && p < W + 2 ? p - 2 : 0] : SENT4;
}

// Cards x - 6 .. x + 5 (SENTINEL outside the row) as three words, byte k card x - 6 + k: the padded words from
// (x + 2) / 4 on, shifted into place a power of two at a time (only those still needed), then by the byte offset.
// BITS is how many bits (x + 2) / 4 can take: 3 when x is at most 29.
template <int W, int BITS>
__device__ __forceinline__ void rrow_window(const u32 (&R)[RW], int x, u32& w0, u32& w1, u32& w2) {
    u32 q = (u32)(x + 2) >> 2, r = 8 * ((x + 2) & 3);    // card x - 6 is byte x + 2 of the padded row
    u32 a[11], b[7], c[5], d[4];
    if (BITS == 4) {
        // At 8 words q is at most 8, so when q & 8 the later stages shift nothing and read only the first four.
        #pragma unroll
        for (int i = 0; i < 11; i++) a[i] = (i < 4 || W > 8) && (q & 8) ? rrow_padded<W>(R, i + 8) : rrow_padded<W>(R, i);
    } else {
        #pragma unroll
        for (int i = 0; i < 11; i++) a[i] = rrow_padded<W>(R, i);
    }
    #pragma unroll
    for (int i = 0; i < 7; i++) b[i] = (q & 4) ? a[i + 4] : a[i];
    #pragma unroll
    for (int i = 0; i < 5; i++) c[i] = (q & 2) ? b[i + 2] : b[i];
    #pragma unroll
    for (int i = 0; i < 4; i++) d[i] = (q & 1) ? c[i + 1] : c[i];
    w0 = __funnelshift_r(d[0], d[1], r);
    w1 = __funnelshift_r(d[1], d[2], r);
    w2 = __funnelshift_r(d[2], d[3], r);
}

// The bits of f (bit i for pile x - 3 + i) as a mask; those below pile 0 fall away.
template <int BITS, typename M>
__device__ __forceinline__ M rrow_place(u32 f, int x) {
    if (sizeof(M) == 8 || BITS == 4) return (M)(((u64)f << x) >> 3);
    return (M)((f << 26) >> (29 - x));
}

// What playing pile x (onto x - 3 with `leap`, else onto x - 1) does, without playing it: the masks after it, which
// row_move works out from the row after the move and this from the cards around x before it, and the mover.
template <int W, int BITS, typename M>
__device__ __forceinline__ void rrow_after(const u32 (&R)[RW], int len, M l1, M l3, int x, bool leap, M& nl1, M& nl3,
                                           u32& mover, u32& covered) {
    u32 w0, w1, w2;
    rrow_window<W, BITS>(R, x, w0, w1, w2);
    mover = (w1 >> 16) & 0xFF;
    covered = leap ? w0 >> 24 : (w1 >> 8) & 0xFF;
    // Cards x - 6 .. x + 2 after the move, byte k card x - 6 + k: the mover on its target, the cards above x down one.
    u32 n0 = leap ? __byte_perm(w0, mover, 0x4210) : w0;
    u32 n1 = __byte_perm(w1, w2, 0x4310);
    n1 = leap ? n1 : __byte_perm(n1, mover, 0x3240);
    u32 n2 = w2 >> 8;
    // The links a move can change (row_move's w1 and w3), from card x - 3 on: a slide changes the mover's link to the
    // card before it, and those of the cards it now lies under; a leap, as well, the two cards between. Only these get
    // tested: cards 3 to 6 against the card before each, and against the card three before them cards 5 to 8, with card
    // 5 in the first place replaced by the mover (card 3 on a leap, 5 already on a slide), so one word does both.
    u32 before = __funnelshift_r(n0, n1, 16);
    u32 s = gather4(match4(__funnelshift_r(n0, n1, 24), before));
    u32 l = gather4(match4((__funnelshift_r(n1, n2, 8) & 0xFFFFFF00u) | mover, leap ? __byte_perm(before, n0, 0x3214) : before));
    l = (l & 0xE) << 2 | (l & 1) << (leap ? 0 : 2);
    u32 ks = leap ? 0x0Bu : 0x04u, kl = leap ? 0x39u : 0x1Cu;     // the bits of s and l this move can change
    M low = ((M)1 << x) - 1;
    M m1 = (l1 & low) | ((l1 >> 1) & ~low), m3 = (l3 & low) | ((l3 >> 1) & ~low);
    M valid = ((M)1 << (len - 1)) - 1;
    nl1 = ((m1 & ~rrow_place<BITS, M>(ks, x)) | rrow_place<BITS, M>(s & ks, x)) & valid & ~(M)1;
    nl3 = ((m3 & ~rrow_place<BITS, M>(kl, x)) | rrow_place<BITS, M>(l & kl, x)) & valid & ~(M)7;
}

template <int W, int BITS, typename M>
__device__ __forceinline__ void rrow_after(const u32 (&R)[RW], int len, M l1, M l3, int x, bool leap, M& nl1, M& nl3,
                                           u32& mover) {
    u32 covered;
    rrow_after<W, BITS, M>(R, len, l1, l3, x, leap, nl1, nl3, mover, covered);
}

// Plays pile x onto pile y (below it): the mover onto y's byte, and every card above x down one place.
template <int W>
__device__ __forceinline__ void rrow_play(u32 (&R)[RW], int x, int y, u32 mover) {
    int yw = y >> 2;
    u32 ysel = (0x3210u & ~(0xFu << (4 * (y & 3)))) | (4u << (4 * (y & 3)));
    #pragma unroll
    for (int j = 0; j < W; j++) {
        u32 next = j + 1 < W ? R[j + 1 < W ? j + 1 : 0] : SENT4;
        u32 keep = __funnelshift_lc(0xFFFFFFFFu, 0u, (u32)max(8 * x - 32 * j, 0));   // the bytes below x
        u32 v = (R[j] & keep) | (__funnelshift_r(R[j], next, 8) & ~keep);
        R[j] = j == yw ? __byte_perm(v, mover, ysel) : v;
    }
}

// How a prefix chooses: it plays a move covering the card whose rank has the fewest others left, and of those one whose
// mover isn't among the last three piles; among equals at random, leaps as usual. Winning lines do that a little more
// often than chance at every move, and it mounts up over a prefix.
// The others left of each rank, 2 bits each: a whole deal has 3 of every rank. For a
// position that isn't one, the counts are only a worse guide to which move to like.
#define RANKS_FULL 0x3FFFFFFu

// The row's places holding rank r: bit i for card i, of its first W words. A byte of (d & 0x0F) + 0x0F has bit 4 set when
// the ranks differ, and SENTINEL's 15 is no rank.
template <int W, typename M>
__device__ __forceinline__ M rrow_rank(const u32 (&R)[RW], u32 r) {
    const u32 pat = r * 0x01010101u;
    M m = 0;
    #pragma unroll
    for (int j = 0; j < W; j++) {
        u32 z = ~(((R[j] ^ pat) & 0x0F0F0F0Fu) + 0x0F0F0F0Fu) & 0x10101010u;
        m |= (M)gather4(z << 3) << (4 * j);
    }
    return m;
}

#ifndef WL_MINB
#define WL_MINB 0      // write_lines' minimum blocks an SM (0: no bound)
#endif
#ifndef REC
#define REC 1          // 1: own moves recorded on the way, not replayed in write_lines
#endif
#ifndef SHR_WIN
#define SHR_WIN 1      // 1: shared moves only among the first 32 piles
#endif
// prefix_move on the register row, choosing as above. A move that would leave no move is found from the masks before
// it is played, so it is never played and taken back.
template <int W, int BITS, typename M>
__device__ __forceinline__ int rrow_move(u32 (&R)[RW], int& len, M& l1, M& l3, u32& rc, u32& rng, u32 leap_shift, M& t1, M& t2, M& t3) {
    M c1 = l1, c3 = l3;
    for (;;) {
        if ((c1 | c3) == 0) return -1;
        // t1, t2, t3: the row's places whose rank has at most 2, 1, 0 others left. The moves that cover the scarcest,
        // and of those the ones whose movers aren't among the last three piles.
        const M win_ = SHR_WIN ? (M)0xFFFFFFFFull : ~(M)0;
        M s_ = c1 & win_, l_ = c3 & win_;
        if (SHR_WIN && (s_ | l_) == 0) return -1;
        {
            M a1, a3;
            bool any;
            a1 = s_ & (t1 << 1); a3 = l_ & (t1 << 3);
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
            a1 = s_ & (t2 << 1); a3 = l_ & (t2 << 3);
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
            a1 = s_ & (t3 << 1); a3 = l_ & (t3 << 3);
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
            M front = len > 3 ? ((M)1 << (len - 3)) - 1 : (M)0;
            a1 = s_ & front; a3 = l_ & front;
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
        }
        u32 n1 = count_bits(s_), n3 = count_bits(l_);
        u32 k = below(rng, n1 + (n3 << leap_shift));
        bool leap = k >= n1;
        int x = select_bit(leap ? l_ : s_, leap ? (k - n1) >> leap_shift : k);
        M nl1, nl3;
        u32 mover, card;
        rrow_after<W, BITS, M>(R, len, l1, l3, x, leap, nl1, nl3, mover, card);
        if (((nl1 | nl3) & win_) == 0 && len > 2) {    // no move left after it: choose another
            M bit = (M)1 << x;
            if (leap) c3 &= ~bit; else c1 &= ~bit;
            continue;
        }
        int y = x - (leap ? 3 : 1);
        rrow_play<W>(R, x, y, mover);
        len--;
        l1 = nl1; l3 = nl3;
        const u32 others = (rc >> (2 * (card & 15))) & 3;
        rc -= (u32)(others != 0) << (2 * (card & 15));
        const M low = ((M)1 << x) - 1, ybit = (M)1 << y;
        #define T_MOVED(t) do { M b = (t >> x) & 1; t = (t & low) | ((t >> 1) & ~low); t = (t & ~ybit) | (b << y); } while (0)
        T_MOVED(t1); T_MOVED(t2); T_MOVED(t3);
        #undef T_MOVED
        const M p = rrow_rank<W, M>(R, card & 15);
        t1 |= others == 3 ? p : (M)0;
        t2 |= others == 2 ? p : (M)0;
        t3 |= others == 1 ? p : (M)0;
        return x << 1 | (leap ? 1 : 0);
    }
}

template <int W, int BITS, typename M>
__device__ __forceinline__ int rrow_try(u32 (&R)[RW], int& len, M& l1, M& l3, M& c1, M& c3, u32& rc, u32& rng, u32 leap_shift, M& t1, M& t2, M& t3) {
    {
        if ((c1 | c3) == 0) return -1;
        // t1, t2, t3: the row's places whose rank has at most 2, 1, 0 others left. The moves that cover the scarcest,
        // and of those the ones whose movers aren't among the last three piles.
        const M win_ = SHR_WIN ? (M)0xFFFFFFFFull : ~(M)0;
        M s_ = c1 & win_, l_ = c3 & win_;
        if (SHR_WIN && (s_ | l_) == 0) return -1;
        {
            M a1, a3;
            bool any;
            a1 = s_ & (t1 << 1); a3 = l_ & (t1 << 3);
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
            a1 = s_ & (t2 << 1); a3 = l_ & (t2 << 3);
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
            a1 = s_ & (t3 << 1); a3 = l_ & (t3 << 3);
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
            M front = len > 3 ? ((M)1 << (len - 3)) - 1 : (M)0;
            a1 = s_ & front; a3 = l_ & front;
            any = (a1 | a3) != 0;
            s_ = any ? a1 : s_; l_ = any ? a3 : l_;
        }
        u32 n1 = count_bits(s_), n3 = count_bits(l_);
        u32 k = below(rng, n1 + (n3 << leap_shift));
        bool leap = k >= n1;
        int x = select_bit(leap ? l_ : s_, leap ? (k - n1) >> leap_shift : k);
        M nl1, nl3;
        u32 mover, card;
        rrow_after<W, BITS, M>(R, len, l1, l3, x, leap, nl1, nl3, mover, card);
        if (((nl1 | nl3) & win_) == 0 && len > 2) {    // no move left after it: choose another
            M bit = (M)1 << x;
            if (leap) c3 &= ~bit; else c1 &= ~bit;
            return -2;
        }
        int y = x - (leap ? 3 : 1);
        rrow_play<W>(R, x, y, mover);
        len--;
        l1 = nl1; l3 = nl3; c1 = nl1; c3 = nl3;
        const u32 others = (rc >> (2 * (card & 15))) & 3;
        rc -= (u32)(others != 0) << (2 * (card & 15));
        const M low = ((M)1 << x) - 1, ybit = (M)1 << y;
        #define T_MOVED(t) do { M b = (t >> x) & 1; t = (t & low) | ((t >> 1) & ~low); t = (t & ~ybit) | (b << y); } while (0)
        T_MOVED(t1); T_MOVED(t2); T_MOVED(t3);
        #undef T_MOVED
        const M p = rrow_rank<W, M>(R, card & 15);
        t1 |= others == 3 ? p : (M)0;
        t2 |= others == 2 ? p : (M)0;
        t3 |= others == 1 ? p : (M)0;
        return x << 1 | (leap ? 1 : 0);
    }
}

// ---- The residual: card i is byte i of (lo, hi); bytes at and above len hold SENTINEL.

__device__ __forceinline__ u32 card_at(u64 lo, u64 hi, int i) {
    return (u32)((i < 8 ? lo >> (8 * i) : hi >> (8 * (i - 8))) & 0xFF);
}
__device__ __forceinline__ u64 below_bytes(int i) { return i <= 0 ? 0 : (i >= 8 ? ~0ull : (1ull << (8 * i)) - 1); }

__device__ __forceinline__ void set_card(u64& lo, u64& hi, int i, u32 c) {
    if (i < 8) { int s = 8 * i; lo = (lo & ~(0xFFull << s)) | ((u64)c << s); }
    else { int s = 8 * (i - 8); hi = (hi & ~(0xFFull << s)) | ((u64)c << s); }
}
// Removes card i: the cards above it move down one place and a sentinel enters at the top.
__device__ __forceinline__ void delete_card(u64& lo, u64& hi, int i) {
    if (i < 8) {
        u64 keep = below_bytes(i);
        lo = (lo & keep) | ((lo >> 8) & ~keep) | (hi << 56);
        hi = (hi >> 8) | (0x7Full << 56);
    } else {
        u64 keep = below_bytes(i - 8);
        hi = (hi & keep) | ((hi >> 8) & ~keep) | (0x7Full << 56);
    }
}
// Puts card c back at i: the cards from i up move up one place.
__device__ __forceinline__ void insert_card(u64& lo, u64& hi, int i, u32 c) {
    if (i < 8) {
        u64 keep = below_bytes(i);
        hi = (hi << 8) | (lo >> 56);
        lo = (lo & keep) | ((lo << 8) & ~keep & ~(0xFFull << (8 * i))) | ((u64)c << (8 * i));
    } else {
        int j = i - 8;
        u64 keep = below_bytes(j);
        hi = (hi & keep) | ((hi << 8) & ~keep & ~(0xFFull << (8 * j))) | ((u64)c << (8 * j));
    }
}
// ---- The search's row in bit planes: three words, each holding two 16-bit planes of the 6-bit codes of up to 16 cards
// (R[0]: rank bits 0 and 1, R[1]: rank bits 2 and 3, R[2]: the suit's two bits), so that one shift and xor compares the
// whole row with itself, and moving a card is a mask and a shift on each word.
__device__ __forceinline__ void bp_load(u32 (&R)[3], u32 w0, u32 w1, u32 w2, u32 w3) {
    #pragma unroll
    for (int k = 0; k < 6; k++) {
        const u32 h = 0x80808080u;
        u32 plane = gather4((w0 << (7 - k)) & h) | gather4((w1 << (7 - k)) & h) << 4 | gather4((w2 << (7 - k)) & h) << 8
                    | gather4((w3 << (7 - k)) & h) << 12;
        if (k & 1) R[k >> 1] |= plane << 16; else R[k >> 1] = plane;
    }
}
__device__ __forceinline__ u32 bp_card(const u32 (&R)[3], int i) {
    u32 a = (R[0] >> i) & 0x00010001u, b = (R[1] >> i) & 0x00010001u, c = (R[2] >> i) & 0x00010001u;
    u32 x = a | b << 2 | c << 4;
    return (x | x >> 15) & 0x3Fu;
}
__device__ __forceinline__ u32 bp_match(const u32 (&R)[3], int d) {
    u32 r = (R[0] ^ (R[0] << d)) | (R[1] ^ (R[1] << d)), s = R[2] ^ (R[2] << d);
    return ~((r | r >> 16) & (s | s >> 16));
}
__device__ __forceinline__ void bp_masks(const u32 (&R)[3], int len, u32& s, u32& l) {
    u32 valid = (1u << len) - 1;
    s = bp_match(R, 1) & valid & ~1u;
    l = bp_match(R, 3) & valid & ~7u;
}

// ---- One edit for playing a move and taking it back, so that the threads doing either run the same instructions.
__device__ __forceinline__ u32 rotr(u32 x, u32 n) { return __funnelshift_r(x, x, n); }
// Pile x = y + dd onto pile y (`fwd`), or back with `cov`, the covered card, returning to y. Bits at and above the row's end are junk.
__device__ __forceinline__ void bp_edit(u32 (&R)[3], bool fwd, u32 x, u32 y, u32 dd, u32 cov) {
    u32 Y = 0x00010001u << y, X = Y << dd, LOW = X - 0x00010001u;
    u32 src = fwd ? X : Y, clr = fwd ? Y : (X | Y), sh = fwd ? 1u : 31u, shm = fwd ? dd : 32u - dd;
    u32 e = fwd ? 0u : __funnelshift_l(cov * 0x8001u, cov * 0x8001u, y);
    #pragma unroll
    for (int j = 0; j < 3; j++) {
        u32 w = R[j];
        u32 s = (w & LOW) | (rotr(w, sh) & ~LOW);
        R[j] = (s & ~clr) | rotr(w & src, shm) | (rotr(e, 2 * j) & Y);
    }
}
// REC: the deals from this one on have lines free past their learn words during attempts_split (won_bits uses the others').
__device__ __forceinline__ u32 rec_low(u32 n_deals) { return ((n_deals + 31) / 32 + 3) / 4; }
// REC: a won try's own moves, a byte each in r0-r4, into its line from place `from`.
__device__ __noinline__ void write_own(u8* lines, u32 deal, int from, int n, u32 r0, u32 r1, u32 r2, u32 r3, u32 r4) {
    u8* out = lines + (u64)deal * 51 + from;
    for (int j = 0; j < n; j++) {
        const int q = j >> 2;
        const u32 w = q == 0 ? r0 : q == 1 ? r1 : q == 2 ? r2 : q == 3 ? r3 : r4;
        out[j] = (u8)(w >> (8 * (j & 3)));
    }
}
// The moves of a won search, from their frames.
__device__ __noinline__ void write_search_f(u8* lines, u32 deal, int moves, int depth, const u32* v, int stride) {
    u8* out = lines + (u64)deal * 51;
    for (int d = 0; d < depth; d++) { u32 x = v[d * stride] & 31; out[moves + d] = (u8)((x & 15) << 1 | x >> 4); }
}
// alive on the cards in two words, with no 64-bit operations and pile 0's card already out of them.
__device__ __forceinline__ bool alive_rest(u32 r0, u32 r1, u32 left) {
    const u32 K = 0x7FFF7FFFu, H = 0x80008000u;
    u32 rank = left & 0x0F;
    u32 mates = (((left & 32) ? r1 : r0) >> (left & 16) & 0x1FFFu) | (((r0 >> rank) | (r1 >> rank)) & 0x00010001u);
    u32 a = (r0 & r1) + K, b = (r0 & __funnelshift_l(r1, r1, 16)) + K, c = ((r0 & (r0 >> 16)) | (r1 & (r1 << 16))) + K;
    u32 w = ((a & H) >> 15) | ((b & H) >> 14) | ((c & H) >> 13);
    u32 links = (w | (w >> 13)) & 0x3F;
    u32 pieces = (links & 32) ? ((links & 16) ? 0x10001u : 0x10156u) : ((links & 16) ? 0x10516u : 0x1116166Bu);
    u32 suits = ((r0 + K) & H) | (((r1 + K) & H) >> 1);
    return (mates != 0) & (((pieces >> (2 * (links & 15))) & 3) + __popc(suits) == 4);
}

// Legal-move masks of the residual: bit i of s, pile i may slide; of l, leap. Four piles at a time in 32-bit words
// (match16); pile 0 can't slide and piles 0 to 2 can't leap, since sentinels lie before them.
__device__ __forceinline__ void masks(u64 lo, u64 hi, int len, u32& s, u32& l) {
    u32 w0 = (u32)lo, w1 = (u32)(lo >> 32), w2 = (u32)hi, w3 = (u32)(hi >> 32);
    u32 valid = (1u << len) - 1;
    s = match16(0x7F7F7F7Fu, w0, w1, w2, w3, 8) & valid;
    l = match16(0x7F7F7F7Fu, w0, w1, w2, w3, 24) & valid;
}
// The row's piles (at most RES of them) into registers.
__device__ __forceinline__ void load_residual(u8 (*row)[BLOCK], int t, int len, u64& lo, u64& hi) {
    lo = 0x7F7F7F7F7F7F7F7Full; hi = 0x7F7F7F7F7F7F7F7Full;
    for (int i = 0; i < len; i++) set_card(lo, hi, i, row[OFF + i][t]);
}

// Profiling (`accgpu --profile`, compiled with -DPROFILE): each thread sums the clock cycles it spends in each phase
// of an attempt in registers, and adds them to `profile` when it runs out of work, with its span in nanoseconds.
// Without PROFILE none of this is compiled. Slots, as `accordion_gpu::PROFILE_SLOTS` names them:
enum {
    P_FETCH, P_LOAD, P_PREFIX, P_GATE, P_SEARCH, P_RESULT,   // cycles: taking work, loading the position, the prefix,
                                                             // the dead-end gate, the search (with loading the
                                                             // residual), writing the result
    P_SKIP,                                                  // cycles taking work that was skipped (its deal was won)
    P_WARP_STEPS, P_STEPS,                                   // search loop steps by warps, and by threads
    P_PREFIX_WARP_STEPS, P_PREFIX_STEPS,                     // the same for the prefix loop
    P_FETCHES, P_SKIPS,                                      // attempts taken, and skipped
    P_BUSY_NS, P_END_NS, P_START_NS_INV,                     // thread spans summed; latest end; ~earliest start
    P_BLOCKED, P_STARVED,                                    // attempts_split: turns of prefix threads with a residual
                                                             // the queue had no room for, and of search threads with
                                                             // none to take
    P_SLOTS
};
#ifdef PROFILE
__device__ __forceinline__ u64 now_ns() { u64 t; asm volatile("mov.u64 %0, %%globaltimer;" : "=l"(t)); return t; }
#define PROF(...) __VA_ARGS__
#else
#define PROF(...)
#endif

// Shared early prefix moves (attempts_split): a deal's tries go in groups of SHARE_G (tries 0 to SHARE_G - 1, and so
// on), and a group's first SHARE_K prefix moves are played once, from random numbers of the group's own; each try then
// plays the rest of its prefix and searches with random numbers of its own.
#ifndef SHARE_G
#define SHARE_G 1
#endif
#ifndef SHARE_K
#define SHARE_K 0
#endif
// A deal's first SOLO_GROUPS groups have one try each, with no group mate (tries 2j + 1 for j < SOLO_GROUPS are not
// played): a mate early on is mostly wasted work, played beside a try that wins. 6.41 attempts a deal to 4.94.
#ifndef SOLO_GROUPS
#define SOLO_GROUPS 16
#endif
// The random numbers of attempt a, and of the group whose first try is attempt a0.
__device__ __forceinline__ u32 own_rng(u64 seed, u32 a) { return seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (a + 1))); }
__device__ __forceinline__ u32 group_rng(u64 seed, u32 a0) { return seed_u32(seed ^ (0xD1B54A32D192ED03ull * (a0 + 1))); }

// Residuals with many ranks left rarely win for what their searches cost (Settings::rank_gate): the most ranks a
// residual of each rung may have to be searched.
#ifndef RANK_GATE
#define RANK_GATE 13
#endif
#ifndef RANK_GATE2
#define RANK_GATE2 13
#endif
// Learning within a deal: a group's tries treat the ranks that the deal's best residuals so far had lost as one class
// scarcer, by starting with rc lower for those ranks and t1 holding their places, so that no code runs per move.
// LEARN_UNTIL: tries from here on neither use the word nor write it (later ones, without targets, find what the targets
// miss). LEARN_FROM: tries before this learn but don't use it. LEARN_TIES: a residual level with the best counts too;
// LEARN_UNION: then the targets are the ranks either lost, otherwise it replaces them. LEARN_CLASSES: how many classes
// scarcer. LEARN_GAP (not negative): within LEARN_UNTIL, residuals with this many ranks more than the best aren't searched.
#define LEARN_UNTIL 128
#define LEARN_FROM 0
#define LEARN_TIES 1
#define LEARN_UNION 1
#define LEARN_CLASSES 1
#define LEARN_GAP -1
#define LEARN_PLUS1 1
// The deal's word sits in its line (zeroed for the launch), whose first 36 bytes no search writes, until write_lines fills
// them. Word 0: the target ranks (13 bits) and above them 14 less the best residual's ranks (0: none yet), stored without
// a lock, as a lost store costs only a little learning. Word 1: the target ranks the winning try's group used.
__device__ __forceinline__ u32* learn_word(u8* lines, u32 deal) { return (u32*)(((u64)(lines + (u64)deal * 51) + 3) & ~3ull); }
// The 13 bits of `ranks` at every second place, as rc holds a rank.
__device__ __forceinline__ u32 learn_spread(u32 r) {
    r = (r | r << 8) & 0x00FF00FFu;
    r = (r | r << 4) & 0x0F0F0F0Fu;
    r = (r | r << 2) & 0x33333333u;
    r = (r | r << 1) & 0x55555555u;
    return r;
}
// The places in the row of the cards of `ranks` (SENTINEL's 15 is no rank).
__device__ __forceinline__ u64 rrow_places(const u32 (&R)[RW], u32 ranks) {
    u64 m = 0;
    #pragma unroll
    for (int j = 0; j < RW; j++) {
        const u32 w = R[j];
        const u32 f = ((ranks >> (w & 15)) & 1) | ((ranks >> ((w >> 8) & 15)) & 1) << 1
                      | ((ranks >> ((w >> 16) & 15)) & 1) << 2 | ((ranks >> ((w >> 24) & 15)) & 1) << 3;
        m |= (u64)f << (4 * j);
    }
    return m;
}
// A won deal's bit: 32 deals a word, 4 words in the free bytes (12 to 35) of a low deal's line, zero at the launch, written over by write_lines.
#ifndef STRETCH_TRIES
#define STRETCH_TRIES 64
#endif
#define STRETCH_WORDS 32u
#ifndef BUNDLE_TRIES
#define BUNDLE_TRIES 128
#endif
#ifndef BUNDLE
#define BUNDLE 32u
#endif
__device__ __forceinline__ u32* won_bits(u8* lines, u32 w) { return learn_word(lines, w >> 2) + 2 + (w & 3); }
// How long a prefix warp sleeps with no room in the queue: its polling takes turns from the warps doing the work.
#define QUEUE_WAIT_NS 10000
#define QSIZE 32           // residuals attempts_split's queue holds
// Search threads take residuals once TAKE_MIN of them need one, or fewer than TAKE_FLOOR are searching: taking is dear
// (a compare-and-swap, and a load of the won flag the whole warp waits for), so not every turn.
#ifndef TAKE_MIN
#define TAKE_MIN 8
#endif
#ifndef TAKE_FLOOR
#define TAKE_FLOOR 16
#endif
#ifndef PREFIX_THREADS
#define PREFIX_THREADS (BLOCK / 2)   // attempts_split's threads a block that play prefixes (whole warps)
#endif

// Whether groups share moves at all, and the words of the row they leave on a deal (52 - SHARE_K cards).
#define SHARING (SHARE_G > 1 && SHARE_K >= 4)
#define OWN_W ((55 - SHARE_K) / 4)

// The row a group's shared moves left, kept for its other tries in the prefix thread's 16 words of s_mem (`w` its
// first; word k at w[k * PREFIX_THREADS]): the words it needs, and the masks (a group shares at least 20 moves, past
// which they fit 32 bits).
__device__ __forceinline__ void keep_row(u32* w, const u32 (&R)[RW], u64 l1, u64 l3, u32 rc, u64 t1, u64 t2, u64 t3) {
    #pragma unroll
    for (int j = 0; j < OWN_W; j++) w[j * PREFIX_THREADS] = R[j];
    w[OWN_W * PREFIX_THREADS] = (u32)l1; w[(OWN_W + 1) * PREFIX_THREADS] = (u32)l3;
    w[(OWN_W + 2) * PREFIX_THREADS] = (u32)t1; w[(OWN_W + 3) * PREFIX_THREADS] = (u32)t2; w[(OWN_W + 4) * PREFIX_THREADS] = (u32)t3;
    w[(OWN_W + 5) * PREFIX_THREADS] = rc;
}
__device__ __forceinline__ void kept_row(const u32* w, u32 (&R)[RW], u64& l1, u64& l3, u32& rc, u64& t1, u64& t2, u64& t3) {
    #pragma unroll
    for (int j = 0; j < OWN_W; j++) R[j] = w[j * PREFIX_THREADS];
    l1 = w[OWN_W * PREFIX_THREADS]; l3 = w[(OWN_W + 1) * PREFIX_THREADS];
    t1 = w[(OWN_W + 2) * PREFIX_THREADS]; t2 = w[(OWN_W + 3) * PREFIX_THREADS]; t3 = w[(OWN_W + 4) * PREFIX_THREADS];
    rc = w[(OWN_W + 5) * PREFIX_THREADS];
}

// ---- A try's own moves on a bit-sliced row: bit i of B[k] is bit k of card i (rank B[0..3], suit B[4..5]), in 32-bit masks
// (a group shares 20 moves, so at most 32 piles are left). A match at any distance over the whole row takes one word's work,
// and a move's after-masks a few bit selections, with no cards fetched: exact choice here costs less than on bytes, and
// scoring several options costs little more than choosing one.
// A place's weight is 0 to 3, w0 + 2 w1: how many of its rank's cards have been covered (exact choice's classes in two
// masks). The move's own choice uses no rank counts: a rank's places all have its weight, one up each time one is covered.
// The last moves are chosen by look-ahead. Of the moves covering a rank with no other card, or all when there are none, up to
// LOOK_TRIES in cyclic order from a random place (leaps first; a row seldom has more) are scored by the moves they'd leave,
// each by how scarce a rank it would cover, 11, 5, 1 for 0, 1, 2 others (cumulative below: 6, 4, 1); 37, 16, 2 for covering
// such a rank now; and 2 random bits, below one unit, which part ties and keep tries from repeating each other.
#define SLICED (SHARING && SHARE_K >= 20)
#if SLICED
#define SLICE_W ((55 - SHARE_K) / 4)
#define LOOK_FROM 27       // the first move chosen by look-ahead
#define LOOK_TRIES 4
#define LOOK_NOW 0x25100200u   // bytes by the covered card's weight: 0, 2, 16, 37
template <int W>
__device__ __forceinline__ void slice_row(const u32 (&R)[RW], u32 (&B)[6]) {
    #pragma unroll
    for (int b = 0; b < 6; b++) {
        u32 m = 0;
        #pragma unroll
        for (int j = 0; j < W; j++) m |= gather4((R[j] << (7 - b)) & 0x80808080u) << (4 * j);
        B[b] = m;
    }
}
__device__ __forceinline__ u32 spread4(u32 n) { return ((n & 15) * 0x00204081u) & 0x01010101u; }
// The first 16 cards back as words, SENTINEL from len on.
__device__ __forceinline__ void unslice_row(const u32 (&B)[6], int len, u32 (&R)[RW]) {
    const u32 fill = len < 16 ? ~0u << len : 0;
    #pragma unroll
    for (int j = 0; j < 4; j++) {
        u32 w = spread4(fill >> (4 * j)) * 0x7F;
        #pragma unroll
        for (int b = 0; b < 6; b++) w |= spread4(B[b] >> (4 * j)) << b;
        R[j] = w;
    }
}
// ---- Shared moves on planes of the row's first 32 piles (SHR_WIN): only those piles move, so every mask fits a word. The piles
// beyond them wait in planes T, the next at bit 0, with their weights in u0, u1, and come in at place 31 as the row shortens. The
// choice is rrow_move's, on the same random numbers, with weights in two masks as in own moves (t1 = w0 | w1, t2 = w1, t3 = w0 & w1),
// so write_lines replays it on words. The links are worked out afresh from the planes after the move: one word's work each.
// Bit i: card i matches card i - d. Places below d and from the row's end on mean nothing; the callers' masks drop them.
template <int d>
__device__ __forceinline__ u32 slice_match(const u32 (&B)[6]) {
    u32 r = (B[0] ^ (B[0] << d)) | (B[1] ^ (B[1] << d)) | (B[2] ^ (B[2] << d)) | (B[3] ^ (B[3] << d));
    u32 s = (B[4] ^ (B[4] << d)) | (B[5] ^ (B[5] << d));
    return ~(r & s);
}
__device__ __forceinline__ void win_load(const u32 (&R)[RW], u32 (&B)[6], u32 (&T)[6]) {
    slice_row<8>(R, B);
    #pragma unroll
    for (int b = 0; b < 6; b++) {
        u32 m = 0xFFF00000u;                       // past the row's end: the sentinel's bits
        #pragma unroll
        for (int j = 0; j < 5; j++) m |= gather4((R[8 + j] << (7 - b)) & 0x80808080u) << (4 * j);
        T[b] = m;
    }
}
__device__ __forceinline__ int win_try(u32 (&B)[6], u32 (&T)[6], int& len, u32& l1, u32& l3, u32& c1, u32& c3, u32& w0, u32& w1, u32& u0, u32& u1, u32& rng, u32 leap_shift) {
    if ((c1 | c3) == 0) return -1;
    u32 s_ = c1, l_ = c3, a1, a3;
    bool any;
    const u32 t1 = w0 | w1, t3 = w0 & w1;
    a1 = s_ & (t1 << 1); a3 = l_ & (t1 << 3);
    any = (a1 | a3) != 0;
    s_ = any ? a1 : s_; l_ = any ? a3 : l_;
    a1 = s_ & (w1 << 1); a3 = l_ & (w1 << 3);
    any = (a1 | a3) != 0;
    s_ = any ? a1 : s_; l_ = any ? a3 : l_;
    a1 = s_ & (t3 << 1); a3 = l_ & (t3 << 3);
    any = (a1 | a3) != 0;
    s_ = any ? a1 : s_; l_ = any ? a3 : l_;
    const u32 front = len >= 35 ? ~0u : (1u << (len - 3)) - 1;
    a1 = s_ & front; a3 = l_ & front;
    any = (a1 | a3) != 0;
    s_ = any ? a1 : s_; l_ = any ? a3 : l_;
    const u32 n1 = __popc(s_), n3 = __popc(l_);
    const u32 k = below(rng, n1 + (n3 << leap_shift));
    const bool leap = k >= n1;
    const int x = select_bit(leap ? l_ : s_, leap ? (k - n1) >> leap_shift : k);
    const u32 dd = leap ? 3 : 1;
    const int y = x - dd;
    const u32 bit = 1u << y, low = (1u << x) - 1;
    u32 N[6];
    #pragma unroll
    for (int b = 0; b < 6; b++) {
        const u32 v = B[b];
        N[b] = (((v & low) | (((v >> 1) | (T[b] << 31)) & ~low)) & ~bit) | ((v >> dd) & bit);
    }
    const u32 valid = len > 32 ? ~0u : (1u << (len - 1)) - 1;
    const u32 nl1 = slice_match<1>(N) & valid & ~1u, nl3 = slice_match<3>(N) & valid & ~7u;
    if ((nl1 | nl3) == 0 && len > 2) {           // no move left after it: choose another
        const u32 got = 1u << x;
        if (leap) c3 &= ~got; else c1 &= ~got;
        return -2;
    }
    // The weights: the mover's go with it, those of the covered card's rank go up one, here and among the piles waiting.
    u32 e = 0, ue = 0;
    #pragma unroll
    for (int j = 0; j < 4; j++) {
        const u32 sg = (u32)((int)(B[j] << (31 - y)) >> 31);
        e |= N[j] ^ sg;
        ue |= T[j] ^ sg;
    }
    const u32 p = ~e, q = ~ue;
    u32 m0 = (w0 & low) | (((w0 >> 1) | (u0 << 31)) & ~low), m1 = (w1 & low) | (((w1 >> 1) | (u1 << 31)) & ~low);
    m0 = (m0 & ~bit) | ((w0 >> dd) & bit);
    m1 = (m1 & ~bit) | ((w1 >> dd) & bit);
    w0 = m0 ^ (p & ~(m0 & m1)); w1 = m1 | (m0 & p);
    const u32 z0 = u0 ^ (q & ~(u0 & u1)), z1 = u1 | (u0 & q);
    u0 = z0 >> 1; u1 = z1 >> 1;
    #pragma unroll
    for (int b = 0; b < 6; b++) { B[b] = N[b]; T[b] >>= 1; }
    len--;
    l1 = nl1; l3 = nl3; c1 = nl1; c3 = nl3;
    return x << 1 | (leap ? 1 : 0);
}
// Move chosen by patterns, phase 1: all options scored at once in lanes, the best found bit by bit from the top.
__device__ __forceinline__ int pat_move(u32 (&B)[6], int& len, u32& l1, u32& l3, u32& w0, u32& w1, u32& rng, const bool last) {
    u32 r0 = next_u32(rng), r1 = __funnelshift_r(r0, r0, 13);
    const u32 PV = (2u << (len - 1)) - 1u;
    const u32 D2 = ~(((B[0] ^ (B[0] << 2)) | (B[1] ^ (B[1] << 2)) | (B[2] ^ (B[2] << 2)) | (B[3] ^ (B[3] << 2))) & ((B[4] ^ (B[4] << 2)) | (B[5] ^ (B[5] << 2)))) & PV & ~3u;
    const u32 D4 = ~(((B[0] ^ (B[0] << 4)) | (B[1] ^ (B[1] << 4)) | (B[2] ^ (B[2] << 4)) | (B[3] ^ (B[3] << 4))) & ((B[4] ^ (B[4] << 4)) | (B[5] ^ (B[5] << 4)))) & PV & ~15u;
    const u32 D6 = ~(((B[0] ^ (B[0] << 6)) | (B[1] ^ (B[1] << 6)) | (B[2] ^ (B[2] << 6)) | (B[3] ^ (B[3] << 6))) & ((B[4] ^ (B[4] << 6)) | (B[5] ^ (B[5] << 6)))) & PV & ~63u;
    const u32 O0 = w0 & w1, O1 = ~w0 & w1, O2 = w0 & ~w1, O3 = ~(w0 | w1);
    const u32 G3 = last ? ~0u : 0u;
    // Bit len-1-k of a rank plane, as a whole word: the planes shifted once, so each broadcast is one or two shifts.
    const u32 TOPB = 1u << (len - 1);
    const u32 BT0 = B[0] << (32 - len);
    const u32 BT1 = B[1] << (32 - len);
    const u32 BT2 = B[2] << (32 - len);
    const u32 BT3 = B[3] << (32 - len);
    const u32 BC0_0 = (u32)((int)BT0 >> 31);
    const u32 BC0_1 = (u32)((int)(BT0 << 1) >> 31);
    const u32 BC0_2 = (u32)((int)(BT0 << 2) >> 31);
    const u32 BC0_3 = (u32)((int)(BT0 << 3) >> 31);
    const u32 BC0_4 = (u32)((int)(BT0 << 4) >> 31);
    const u32 BC0_5 = (u32)((int)(BT0 << 5) >> 31);
    const u32 BC1_0 = (u32)((int)BT1 >> 31);
    const u32 BC1_1 = (u32)((int)(BT1 << 1) >> 31);
    const u32 BC1_2 = (u32)((int)(BT1 << 2) >> 31);
    const u32 BC1_3 = (u32)((int)(BT1 << 3) >> 31);
    const u32 BC1_4 = (u32)((int)(BT1 << 4) >> 31);
    const u32 BC1_5 = (u32)((int)(BT1 << 5) >> 31);
    const u32 BC2_0 = (u32)((int)BT2 >> 31);
    const u32 BC2_1 = (u32)((int)(BT2 << 1) >> 31);
    const u32 BC2_2 = (u32)((int)(BT2 << 2) >> 31);
    const u32 BC2_3 = (u32)((int)(BT2 << 3) >> 31);
    const u32 BC2_4 = (u32)((int)(BT2 << 4) >> 31);
    const u32 BC2_5 = (u32)((int)(BT2 << 5) >> 31);
    const u32 BC3_0 = (u32)((int)BT3 >> 31);
    const u32 BC3_1 = (u32)((int)(BT3 << 1) >> 31);
    const u32 BC3_2 = (u32)((int)(BT3 << 2) >> 31);
    const u32 BC3_3 = (u32)((int)(BT3 << 3) >> 31);
    const u32 BC3_4 = (u32)((int)(BT3 << 4) >> 31);
    const u32 BC3_5 = (u32)((int)(BT3 << 5) >> 31);
    u32 Y0 = 0, Y1 = 0, Y2 = 0, Y3 = 0, Y4 = 0, Y5 = 0, AS0 = 0, AL0 = 0, AS1 = 0, AL1 = 0, AS2 = 0, AL2 = 0, AS3 = 0, AL3 = 0, AS4 = 0, AL4 = 0, AS5 = 0, AL5 = 0, AS6 = 0, AL6 = 0;
    if (last) {
        const u32 E0 = ~((B[0] ^ BC0_0) | (B[1] ^ BC1_0) | (B[2] ^ BC2_0) | (B[3] ^ BC3_0)) & ~(TOPB >> 0);
        const u32 E1 = ~((B[0] ^ BC0_1) | (B[1] ^ BC1_1) | (B[2] ^ BC2_1) | (B[3] ^ BC3_1)) & ~(TOPB >> 1);
        const u32 E2 = ~((B[0] ^ BC0_2) | (B[1] ^ BC1_2) | (B[2] ^ BC2_2) | (B[3] ^ BC3_2)) & ~(TOPB >> 2);
        const u32 E3 = ~((B[0] ^ BC0_3) | (B[1] ^ BC1_3) | (B[2] ^ BC2_3) | (B[3] ^ BC3_3)) & ~(TOPB >> 3);
        const u32 E4 = ~((B[0] ^ BC0_4) | (B[1] ^ BC1_4) | (B[2] ^ BC2_4) | (B[3] ^ BC3_4)) & ~(TOPB >> 4);
        const u32 gs1 = E0 ^ E1 ^ E2;
        const u32 gc1 = (E0 & E1) | (E2 & (E0 ^ E1));
        const u32 gs2 = E3 ^ E4 ^ gs1;
        const u32 gc2 = (E3 & E4) | (gs1 & (E3 ^ E4));
        const u32 gs3 = E0 ^ E1 ^ E2;
        const u32 gc3 = (E0 & E1) | (E2 & (E0 ^ E1));
        const u32 gs4 = E3 ^ E4 ^ gs3;
        const u32 gc4 = (E3 & E4) | (gs3 & (E3 ^ E4));
        const u32 gs5 = E0 ^ E1 ^ E2;
        const u32 gc5 = (E0 & E1) | (E2 & (E0 ^ E1));
        const u32 gs6 = E3 ^ E4 ^ gs5;
        const u32 gc6 = (E3 & E4) | (gs5 & (E3 ^ E4));
        const u32 gs7 = E0 ^ E1 ^ E2;
        const u32 gc7 = (E0 & E1) | (E2 & (E0 ^ E1));
        const u32 gs8 = E3 ^ E4 ^ gs7;
        const u32 gc8 = (E3 & E4) | (gs7 & (E3 ^ E4));
        const u32 Yt1 = (gs2) & (w0 & w1);
        const u32 Yt2 = ~((gs6) & (~w0 & w1));
        const u32 Yt3 = O0 & ((TOPB >> 2) | (TOPB >> 3) | (TOPB >> 4));
        const u32 Yt4 = O0 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt5 = (w1 & PV);
        const u32 Yt6 = ~0u;
        const u32 Yt7 = Yt1 ^ Yt2 ^ Yt3;
        const u32 Yt8 = (Yt1 & Yt2) | (Yt3 & (Yt1 ^ Yt2));
        const u32 Yt9 = Yt4 ^ Yt5 ^ Yt6;
        const u32 Yt10 = (Yt4 & Yt5) | (Yt6 & (Yt4 ^ Yt5));
        const u32 Yt11 = Yt7 ^ Yt9;
        const u32 Yt12 = Yt7 & Yt9;
        const u32 Yt13 = (gc1) & (w0 & w1);
        const u32 Yt14 = (gc2) & (w0 & w1);
        const u32 Yt15 = ~((gc5) & (~w0 & w1));
        const u32 Yt16 = ~((gc6) & (~w0 & w1));
        const u32 Yt17 = ((w0 | w1) & PV);
        const u32 Yt18 = Yt8;
        const u32 Yt19 = Yt10;
        const u32 Yt20 = Yt12;
        const u32 Yt21 = Yt13 ^ Yt14 ^ Yt15;
        const u32 Yt22 = (Yt13 & Yt14) | (Yt15 & (Yt13 ^ Yt14));
        const u32 Yt23 = Yt16 ^ Yt17 ^ Yt18;
        const u32 Yt24 = (Yt16 & Yt17) | (Yt18 & (Yt16 ^ Yt17));
        const u32 Yt25 = Yt19 ^ Yt20 ^ Yt21;
        const u32 Yt26 = (Yt19 & Yt20) | (Yt21 & (Yt19 ^ Yt20));
        const u32 Yt27 = Yt23 ^ Yt25;
        const u32 Yt28 = Yt23 & Yt25;
        const u32 Yt29 = (gs4) & (w0 & w1);
        const u32 Yt30 = ~((gs8) & (~w0 & w1));
        const u32 Yt31 = O0 & ((TOPB >> 2) | (TOPB >> 3) | (TOPB >> 4));
        const u32 Yt32 = O0 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt33 = (w1 & PV);
        const u32 Yt34 = Yt22;
        const u32 Yt35 = Yt24;
        const u32 Yt36 = Yt26;
        const u32 Yt37 = Yt28;
        const u32 Yt38 = Yt29 ^ Yt30 ^ Yt31;
        const u32 Yt39 = (Yt29 & Yt30) | (Yt31 & (Yt29 ^ Yt30));
        const u32 Yt40 = Yt32 ^ Yt33 ^ Yt34;
        const u32 Yt41 = (Yt32 & Yt33) | (Yt34 & (Yt32 ^ Yt33));
        const u32 Yt42 = Yt35 ^ Yt36 ^ Yt37;
        const u32 Yt43 = (Yt35 & Yt36) | (Yt37 & (Yt35 ^ Yt36));
        const u32 Yt44 = Yt38 ^ Yt40 ^ Yt42;
        const u32 Yt45 = (Yt38 & Yt40) | (Yt42 & (Yt38 ^ Yt40));
        const u32 Yt46 = (gc3) & (w0 & w1);
        const u32 Yt47 = (gc4) & (w0 & w1);
        const u32 Yt48 = ~((gc7) & (~w0 & w1));
        const u32 Yt49 = ~((gc8) & (~w0 & w1));
        const u32 Yt50 = ((w0 & w1) & PV);
        const u32 Yt51 = Yt39;
        const u32 Yt52 = Yt41;
        const u32 Yt53 = Yt43;
        const u32 Yt54 = Yt45;
        const u32 Yt55 = Yt46 ^ Yt47 ^ Yt48;
        const u32 Yt56 = (Yt46 & Yt47) | (Yt48 & (Yt46 ^ Yt47));
        const u32 Yt57 = Yt49 ^ Yt50 ^ Yt51;
        const u32 Yt58 = (Yt49 & Yt50) | (Yt51 & (Yt49 ^ Yt50));
        const u32 Yt59 = Yt52 ^ Yt53 ^ Yt54;
        const u32 Yt60 = (Yt52 & Yt53) | (Yt54 & (Yt52 ^ Yt53));
        const u32 Yt61 = Yt55 ^ Yt57 ^ Yt59;
        const u32 Yt62 = (Yt55 & Yt57) | (Yt59 & (Yt55 ^ Yt57));
        const u32 Yt63 = Yt56;
        const u32 Yt64 = Yt58;
        const u32 Yt65 = Yt60;
        const u32 Yt66 = Yt62;
        const u32 Yt67 = Yt63 ^ Yt64 ^ Yt65;
        const u32 Yt68 = (Yt63 & Yt64) | (Yt65 & (Yt63 ^ Yt64));
        const u32 Yt69 = Yt66 ^ Yt67;
        const u32 Yt70 = Yt66 & Yt67;
        const u32 Yt71 = Yt68;
        const u32 Yt72 = Yt70;
        const u32 Yt73 = Yt71 ^ Yt72;
        const u32 Yt74 = Yt71 & Yt72;
        const u32 Yt75 = Yt74;
        const u32 ASt1 = (D2) & (O0 << 2);
        const u32 ASt2 = ~((l1 << 1) & (O0 << 2));
        const u32 ASt3 = ~((~0u) & (O0 << 1));
        const u32 ASt4 = ASt1 ^ ASt2 ^ ASt3;
        const u32 ASt5 = (ASt1 & ASt2) | (ASt3 & (ASt1 ^ ASt2));
        const u32 ASt6 = (D4) & (O0 << 4);
        const u32 ASt7 = ~((l3 << 1) & (O0 << 4));
        const u32 ASt8 = (D4 >> 1) & (O0 << 3);
        const u32 ASt9 = ~((l3) & (O0 << 3));
        const u32 ASt10 = (D4 >> 2) & (O0 << 2);
        const u32 ASt11 = ~((l3 >> 1) & (O0 << 2));
        const u32 ASt12 = ~((l3 >> 2) & (O0 << 1));
        const u32 ASt13 = ASt5;
        const u32 ASt14 = ASt6 ^ ASt7 ^ ASt8;
        const u32 ASt15 = (ASt6 & ASt7) | (ASt8 & (ASt6 ^ ASt7));
        const u32 ASt16 = ASt9 ^ ASt10 ^ ASt11;
        const u32 ASt17 = (ASt9 & ASt10) | (ASt11 & (ASt9 ^ ASt10));
        const u32 ASt18 = ASt12 ^ ASt13 ^ ASt14;
        const u32 ASt19 = (ASt12 & ASt13) | (ASt14 & (ASt12 ^ ASt13));
        const u32 ASt20 = ASt16 ^ ASt18;
        const u32 ASt21 = ASt16 & ASt18;
        const u32 ASt22 = (D4) & (O0 << 4);
        const u32 ASt23 = ~((l3 << 1) & (O0 << 4));
        const u32 ASt24 = (D4 >> 1) & (O0 << 3);
        const u32 ASt25 = ~((l3) & (O0 << 3));
        const u32 ASt26 = (D4 >> 2) & (O0 << 2);
        const u32 ASt27 = ~((l3 >> 1) & (O0 << 2));
        const u32 ASt28 = ~((l3 >> 2) & (O0 << 1));
        const u32 ASt29 = (D2) & (O0 << 2);
        const u32 ASt30 = ~((l1 << 1) & (O0 << 2));
        const u32 ASt31 = ~((~0u) & (O0 << 1));
        const u32 ASt32 = ASt15;
        const u32 ASt33 = ASt17;
        const u32 ASt34 = ASt19;
        const u32 ASt35 = ASt21;
        const u32 ASt36 = ASt22 ^ ASt23 ^ ASt24;
        const u32 ASt37 = (ASt22 & ASt23) | (ASt24 & (ASt22 ^ ASt23));
        const u32 ASt38 = ASt25 ^ ASt26 ^ ASt27;
        const u32 ASt39 = (ASt25 & ASt26) | (ASt27 & (ASt25 ^ ASt26));
        const u32 ASt40 = ASt28 ^ ASt29 ^ ASt30;
        const u32 ASt41 = (ASt28 & ASt29) | (ASt30 & (ASt28 ^ ASt29));
        const u32 ASt42 = ASt31 ^ ASt32 ^ ASt33;
        const u32 ASt43 = (ASt31 & ASt32) | (ASt33 & (ASt31 ^ ASt32));
        const u32 ASt44 = ASt34 ^ ASt35 ^ ASt36;
        const u32 ASt45 = (ASt34 & ASt35) | (ASt36 & (ASt34 ^ ASt35));
        const u32 ASt46 = ASt38 ^ ASt40 ^ ASt42;
        const u32 ASt47 = (ASt38 & ASt40) | (ASt42 & (ASt38 ^ ASt40));
        const u32 ASt48 = ASt44 ^ ASt46;
        const u32 ASt49 = ASt44 & ASt46;
        const u32 ASt50 = ASt37;
        const u32 ASt51 = ASt39;
        const u32 ASt52 = ASt41;
        const u32 ASt53 = ASt43;
        const u32 ASt54 = ASt45;
        const u32 ASt55 = ASt47;
        const u32 ASt56 = ASt49;
        const u32 ASt57 = ASt50 ^ ASt51 ^ ASt52;
        const u32 ASt58 = (ASt50 & ASt51) | (ASt52 & (ASt50 ^ ASt51));
        const u32 ASt59 = ASt53 ^ ASt54 ^ ASt55;
        const u32 ASt60 = (ASt53 & ASt54) | (ASt55 & (ASt53 ^ ASt54));
        const u32 ASt61 = ASt56 ^ ASt57 ^ ASt59;
        const u32 ASt62 = (ASt56 & ASt57) | (ASt59 & (ASt56 ^ ASt57));
        const u32 ASt63 = ~0u;
        const u32 ASt64 = ASt58;
        const u32 ASt65 = ASt60;
        const u32 ASt66 = ASt62;
        const u32 ASt67 = ASt63 ^ ASt64 ^ ASt65;
        const u32 ASt68 = (ASt63 & ASt64) | (ASt65 & (ASt63 ^ ASt64));
        const u32 ASt69 = ASt66 ^ ASt67;
        const u32 ASt70 = ASt66 & ASt67;
        const u32 ASt71 = ASt68;
        const u32 ASt72 = ASt70;
        const u32 ASt73 = ASt71 ^ ASt72;
        const u32 ASt74 = ASt71 & ASt72;
        const u32 ASt75 = ASt74;
        const u32 ALt1 = (D4) & (O0 << 4);
        const u32 ALt2 = ~((l1 << 3) & (O0 << 4));
        const u32 ALt3 = (D2) & O0;
        const u32 ALt4 = ~((l1 >> 1) & O0);
        const u32 ALt5 = ~((l1 << 2) & (O0 << 3));
        const u32 ALt6 = (D2 >> 1) & (O0 << 1);
        const u32 ALt7 = ~((l1) & (O0 << 1));
        const u32 ALt8 = ALt1 ^ ALt2 ^ ALt3;
        const u32 ALt9 = (ALt1 & ALt2) | (ALt3 & (ALt1 ^ ALt2));
        const u32 ALt10 = ALt4 ^ ALt5 ^ ALt6;
        const u32 ALt11 = (ALt4 & ALt5) | (ALt6 & (ALt4 ^ ALt5));
        const u32 ALt12 = ALt7 ^ ALt8 ^ ALt10;
        const u32 ALt13 = (ALt7 & ALt8) | (ALt10 & (ALt7 ^ ALt8));
        const u32 ALt14 = (D6) & (O0 << 6);
        const u32 ALt15 = ~((l3 << 3) & (O0 << 6));
        const u32 ALt16 = ~((~0u) & (O0 << 3));
        const u32 ALt17 = (l1 >> 1) & O0;
        const u32 ALt18 = ~((l3 >> 3) & O0);
        const u32 ALt19 = (D4 >> 2) & (O0 << 2);
        const u32 ALt20 = ~((l3 >> 1) & (O0 << 2));
        const u32 ALt21 = (D4 >> 3) & (O0 << 1);
        const u32 ALt22 = ~((l3 >> 2) & (O0 << 1));
        const u32 ALt23 = ALt9;
        const u32 ALt24 = ALt11;
        const u32 ALt25 = ALt13;
        const u32 ALt26 = ALt14 ^ ALt15 ^ ALt16;
        const u32 ALt27 = (ALt14 & ALt15) | (ALt16 & (ALt14 ^ ALt15));
        const u32 ALt28 = ALt17 ^ ALt18 ^ ALt19;
        const u32 ALt29 = (ALt17 & ALt18) | (ALt19 & (ALt17 ^ ALt18));
        const u32 ALt30 = ALt20 ^ ALt21 ^ ALt22;
        const u32 ALt31 = (ALt20 & ALt21) | (ALt22 & (ALt20 ^ ALt21));
        const u32 ALt32 = ALt23 ^ ALt24 ^ ALt25;
        const u32 ALt33 = (ALt23 & ALt24) | (ALt25 & (ALt23 ^ ALt24));
        const u32 ALt34 = ALt26 ^ ALt28 ^ ALt30;
        const u32 ALt35 = (ALt26 & ALt28) | (ALt30 & (ALt26 ^ ALt28));
        const u32 ALt36 = ALt32 ^ ALt34;
        const u32 ALt37 = ALt32 & ALt34;
        const u32 ALt38 = (D6) & (O0 << 6);
        const u32 ALt39 = ~((l3 << 3) & (O0 << 6));
        const u32 ALt40 = ~((~0u) & (O0 << 3));
        const u32 ALt41 = (l1 >> 1) & O0;
        const u32 ALt42 = ~((l3 >> 3) & O0);
        const u32 ALt43 = (D4 >> 2) & (O0 << 2);
        const u32 ALt44 = ~((l3 >> 1) & (O0 << 2));
        const u32 ALt45 = (D4 >> 3) & (O0 << 1);
        const u32 ALt46 = ~((l3 >> 2) & (O0 << 1));
        const u32 ALt47 = (D4) & (O0 << 4);
        const u32 ALt48 = ~((l1 << 3) & (O0 << 4));
        const u32 ALt49 = (D2) & O0;
        const u32 ALt50 = ~((l1 >> 1) & O0);
        const u32 ALt51 = ~((l1 << 2) & (O0 << 3));
        const u32 ALt52 = (D2 >> 1) & (O0 << 1);
        const u32 ALt53 = ~((l1) & (O0 << 1));
        const u32 ALt54 = ALt27;
        const u32 ALt55 = ALt29;
        const u32 ALt56 = ALt31;
        const u32 ALt57 = ALt33;
        const u32 ALt58 = ALt35;
        const u32 ALt59 = ALt37;
        const u32 ALt60 = ALt38 ^ ALt39 ^ ALt40;
        const u32 ALt61 = (ALt38 & ALt39) | (ALt40 & (ALt38 ^ ALt39));
        const u32 ALt62 = ALt41 ^ ALt42 ^ ALt43;
        const u32 ALt63 = (ALt41 & ALt42) | (ALt43 & (ALt41 ^ ALt42));
        const u32 ALt64 = ALt44 ^ ALt45 ^ ALt46;
        const u32 ALt65 = (ALt44 & ALt45) | (ALt46 & (ALt44 ^ ALt45));
        const u32 ALt66 = ALt47 ^ ALt48 ^ ALt49;
        const u32 ALt67 = (ALt47 & ALt48) | (ALt49 & (ALt47 ^ ALt48));
        const u32 ALt68 = ALt50 ^ ALt51 ^ ALt52;
        const u32 ALt69 = (ALt50 & ALt51) | (ALt52 & (ALt50 ^ ALt51));
        const u32 ALt70 = ALt53 ^ ALt54 ^ ALt55;
        const u32 ALt71 = (ALt53 & ALt54) | (ALt55 & (ALt53 ^ ALt54));
        const u32 ALt72 = ALt56 ^ ALt57 ^ ALt58;
        const u32 ALt73 = (ALt56 & ALt57) | (ALt58 & (ALt56 ^ ALt57));
        const u32 ALt74 = ALt59 ^ ALt60 ^ ALt62;
        const u32 ALt75 = (ALt59 & ALt60) | (ALt62 & (ALt59 ^ ALt60));
        const u32 ALt76 = ALt64 ^ ALt66 ^ ALt68;
        const u32 ALt77 = (ALt64 & ALt66) | (ALt68 & (ALt64 ^ ALt66));
        const u32 ALt78 = ALt70 ^ ALt72 ^ ALt74;
        const u32 ALt79 = (ALt70 & ALt72) | (ALt74 & (ALt70 ^ ALt72));
        const u32 ALt80 = ALt76 ^ ALt78;
        const u32 ALt81 = ALt76 & ALt78;
        const u32 ALt82 = ALt61;
        const u32 ALt83 = ALt63;
        const u32 ALt84 = ALt65;
        const u32 ALt85 = ALt67;
        const u32 ALt86 = ALt69;
        const u32 ALt87 = ALt71;
        const u32 ALt88 = ALt73;
        const u32 ALt89 = ALt75;
        const u32 ALt90 = ALt77;
        const u32 ALt91 = ALt79;
        const u32 ALt92 = ALt81;
        const u32 ALt93 = ALt82 ^ ALt83 ^ ALt84;
        const u32 ALt94 = (ALt82 & ALt83) | (ALt84 & (ALt82 ^ ALt83));
        const u32 ALt95 = ALt85 ^ ALt86 ^ ALt87;
        const u32 ALt96 = (ALt85 & ALt86) | (ALt87 & (ALt85 ^ ALt86));
        const u32 ALt97 = ALt88 ^ ALt89 ^ ALt90;
        const u32 ALt98 = (ALt88 & ALt89) | (ALt90 & (ALt88 ^ ALt89));
        const u32 ALt99 = ALt91 ^ ALt92 ^ ALt93;
        const u32 ALt100 = (ALt91 & ALt92) | (ALt93 & (ALt91 ^ ALt92));
        const u32 ALt101 = ALt95 ^ ALt97 ^ ALt99;
        const u32 ALt102 = (ALt95 & ALt97) | (ALt99 & (ALt95 ^ ALt97));
        const u32 ALt103 = ALt94;
        const u32 ALt104 = ALt96;
        const u32 ALt105 = ALt98;
        const u32 ALt106 = ALt100;
        const u32 ALt107 = ALt102;
        const u32 ALt108 = ALt103 ^ ALt104 ^ ALt105;
        const u32 ALt109 = (ALt103 & ALt104) | (ALt105 & (ALt103 ^ ALt104));
        const u32 ALt110 = ALt106 ^ ALt107 ^ ALt108;
        const u32 ALt111 = (ALt106 & ALt107) | (ALt108 & (ALt106 ^ ALt107));
        const u32 ALt112 = ALt109;
        const u32 ALt113 = ALt111;
        const u32 ALt114 = ALt112 ^ ALt113;
        const u32 ALt115 = ALt112 & ALt113;
        const u32 ALt116 = ALt115;
        Y0 = Yt11;
        Y1 = Yt27;
        Y2 = Yt44;
        Y3 = Yt61;
        Y4 = Yt69;
        Y5 = Yt73;
        AS0 = ASt4; AL0 = ALt12;
        AS1 = ASt20; AL1 = ALt36;
        AS2 = ASt48; AL2 = ALt80;
        AS3 = ASt61; AL3 = ALt101;
        AS4 = ASt69; AL4 = ALt110;
        AS5 = ASt73; AL5 = ALt114;
        AS6 = ASt75; AL6 = ALt116;
    } else {
        const u32 E0 = ~((B[0] ^ BC0_0) | (B[1] ^ BC1_0) | (B[2] ^ BC2_0) | (B[3] ^ BC3_0)) & ~(TOPB >> 0);
        const u32 E1 = ~((B[0] ^ BC0_1) | (B[1] ^ BC1_1) | (B[2] ^ BC2_1) | (B[3] ^ BC3_1)) & ~(TOPB >> 1);
        const u32 E2 = ~((B[0] ^ BC0_2) | (B[1] ^ BC1_2) | (B[2] ^ BC2_2) | (B[3] ^ BC3_2)) & ~(TOPB >> 2);
        const u32 E3 = ~((B[0] ^ BC0_3) | (B[1] ^ BC1_3) | (B[2] ^ BC2_3) | (B[3] ^ BC3_3)) & ~(TOPB >> 3);
        const u32 E4 = ~((B[0] ^ BC0_4) | (B[1] ^ BC1_4) | (B[2] ^ BC2_4) | (B[3] ^ BC3_4)) & ~(TOPB >> 4);
        const u32 E5 = ~((B[0] ^ BC0_5) | (B[1] ^ BC1_5) | (B[2] ^ BC2_5) | (B[3] ^ BC3_5)) & ~(TOPB >> 5);
        const u32 gs1 = E0 ^ E1 ^ E2;
        const u32 gc1 = (E0 & E1) | (E2 & (E0 ^ E1));
        const u32 gs2 = E3 ^ E4 ^ gs1;
        const u32 gc2 = (E3 & E4) | (gs1 & (E3 ^ E4));
        const u32 gs3 = E0 ^ E1;
        const u32 gc3 = E0 & E1;
        const u32 gs4 = E0 ^ E1 ^ E2;
        const u32 gc4 = (E0 & E1) | (E2 & (E0 ^ E1));
        const u32 gs5 = E3 ^ E4 ^ gs4;
        const u32 gc5 = (E3 & E4) | (gs4 & (E3 ^ E4));
        const u32 gs6 = E0 ^ E1;
        const u32 gc6 = E0 & E1;
        const u32 gs7 = E3 ^ E4;
        const u32 gc7 = E3 & E4;
        const u32 gs8 = E0 ^ E1;
        const u32 gc8 = E0 & E1;
        const u32 gs9 = E3 ^ E4;
        const u32 gc9 = E3 & E4;
        const u32 gs10 = E0 ^ E1;
        const u32 gc10 = E0 & E1;
        const u32 Yt1 = (gs2) & (w0 & w1);
        const u32 Yt2 = ~((gs6) & (w0 & ~w1));
        const u32 Yt3 = ~((E2) & (~w0 & w1));
        const u32 Yt4 = ~((E5) & (~w0 & ~w1));
        const u32 Yt5 = ~((gs9) & ~w0);
        const u32 Yt6 = O0 & ((TOPB >> 2) | (TOPB >> 3) | (TOPB >> 4));
        const u32 Yt7 = O0 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt8 = O1 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt9 = ~(O3 & ((TOPB >> 3) | (TOPB >> 4) | (TOPB >> 5)));
        const u32 Yt10 = ~0u;
        const u32 Yt11 = Yt1 ^ Yt2 ^ Yt3;
        const u32 Yt12 = (Yt1 & Yt2) | (Yt3 & (Yt1 ^ Yt2));
        const u32 Yt13 = Yt4 ^ Yt5 ^ Yt6;
        const u32 Yt14 = (Yt4 & Yt5) | (Yt6 & (Yt4 ^ Yt5));
        const u32 Yt15 = Yt7 ^ Yt8 ^ Yt9;
        const u32 Yt16 = (Yt7 & Yt8) | (Yt9 & (Yt7 ^ Yt8));
        const u32 Yt17 = Yt10 ^ Yt11 ^ Yt13;
        const u32 Yt18 = (Yt10 & Yt11) | (Yt13 & (Yt10 ^ Yt11));
        const u32 Yt19 = Yt15 ^ Yt17;
        const u32 Yt20 = Yt15 & Yt17;
        const u32 Yt21 = (gc1) & (w0 & w1);
        const u32 Yt22 = (gc2) & (w0 & w1);
        const u32 Yt23 = (gs3) & (w0 & w1);
        const u32 Yt24 = ~((gc6) & (w0 & ~w1));
        const u32 Yt25 = ~((gs7) & (w0 ^ w1));
        const u32 Yt26 = ~((E2) & ~(w0 & w1));
        const u32 Yt27 = ~((gc9) & ~w0);
        const u32 Yt28 = ~((gs10) & ~w1);
        const u32 Yt29 = ~(O3 & ((TOPB >> 0) | (TOPB >> 1) | (TOPB >> 2)));
        const u32 Yt30 = O1 & ((TOPB >> 2) | (TOPB >> 3) | (TOPB >> 4));
        const u32 Yt31 = O0 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt32 = O1 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt33 = ~0u;
        const u32 Yt34 = Yt12;
        const u32 Yt35 = Yt14;
        const u32 Yt36 = Yt16;
        const u32 Yt37 = Yt18;
        const u32 Yt38 = Yt20;
        const u32 Yt39 = Yt21 ^ Yt22 ^ Yt23;
        const u32 Yt40 = (Yt21 & Yt22) | (Yt23 & (Yt21 ^ Yt22));
        const u32 Yt41 = Yt24 ^ Yt25 ^ Yt26;
        const u32 Yt42 = (Yt24 & Yt25) | (Yt26 & (Yt24 ^ Yt25));
        const u32 Yt43 = Yt27 ^ Yt28 ^ Yt29;
        const u32 Yt44 = (Yt27 & Yt28) | (Yt29 & (Yt27 ^ Yt28));
        const u32 Yt45 = Yt30 ^ Yt31 ^ Yt32;
        const u32 Yt46 = (Yt30 & Yt31) | (Yt32 & (Yt30 ^ Yt31));
        const u32 Yt47 = Yt33 ^ Yt34 ^ Yt35;
        const u32 Yt48 = (Yt33 & Yt34) | (Yt35 & (Yt33 ^ Yt34));
        const u32 Yt49 = Yt36 ^ Yt37 ^ Yt38;
        const u32 Yt50 = (Yt36 & Yt37) | (Yt38 & (Yt36 ^ Yt37));
        const u32 Yt51 = Yt39 ^ Yt41 ^ Yt43;
        const u32 Yt52 = (Yt39 & Yt41) | (Yt43 & (Yt39 ^ Yt41));
        const u32 Yt53 = Yt45 ^ Yt47 ^ Yt49;
        const u32 Yt54 = (Yt45 & Yt47) | (Yt49 & (Yt45 ^ Yt47));
        const u32 Yt55 = Yt51 ^ Yt53;
        const u32 Yt56 = Yt51 & Yt53;
        const u32 Yt57 = (gc3) & (w0 & w1);
        const u32 Yt58 = (gs5) & (w0 & w1);
        const u32 Yt59 = ~((gc7) & (w0 ^ w1));
        const u32 Yt60 = ~((gs8) & (~w0 & w1));
        const u32 Yt61 = ~((gc10) & ~w1);
        const u32 Yt62 = O0 & ((TOPB >> 2) | (TOPB >> 3) | (TOPB >> 4));
        const u32 Yt63 = O0 & ((TOPB >> 0) | (TOPB >> 1));
        const u32 Yt64 = Yt40;
        const u32 Yt65 = Yt42;
        const u32 Yt66 = Yt44;
        const u32 Yt67 = Yt46;
        const u32 Yt68 = Yt48;
        const u32 Yt69 = Yt50;
        const u32 Yt70 = Yt52;
        const u32 Yt71 = Yt54;
        const u32 Yt72 = Yt56;
        const u32 Yt73 = Yt57 ^ Yt58 ^ Yt59;
        const u32 Yt74 = (Yt57 & Yt58) | (Yt59 & (Yt57 ^ Yt58));
        const u32 Yt75 = Yt60 ^ Yt61 ^ Yt62;
        const u32 Yt76 = (Yt60 & Yt61) | (Yt62 & (Yt60 ^ Yt61));
        const u32 Yt77 = Yt63 ^ Yt64 ^ Yt65;
        const u32 Yt78 = (Yt63 & Yt64) | (Yt65 & (Yt63 ^ Yt64));
        const u32 Yt79 = Yt66 ^ Yt67 ^ Yt68;
        const u32 Yt80 = (Yt66 & Yt67) | (Yt68 & (Yt66 ^ Yt67));
        const u32 Yt81 = Yt69 ^ Yt70 ^ Yt71;
        const u32 Yt82 = (Yt69 & Yt70) | (Yt71 & (Yt69 ^ Yt70));
        const u32 Yt83 = Yt72 ^ Yt73 ^ Yt75;
        const u32 Yt84 = (Yt72 & Yt73) | (Yt75 & (Yt72 ^ Yt73));
        const u32 Yt85 = Yt77 ^ Yt79 ^ Yt81;
        const u32 Yt86 = (Yt77 & Yt79) | (Yt81 & (Yt77 ^ Yt79));
        const u32 Yt87 = Yt83 ^ Yt85;
        const u32 Yt88 = Yt83 & Yt85;
        const u32 Yt89 = (gc4) & (w0 & w1);
        const u32 Yt90 = (gc5) & (w0 & w1);
        const u32 Yt91 = ~((gc8) & (~w0 & w1));
        const u32 Yt92 = Yt74;
        const u32 Yt93 = Yt76;
        const u32 Yt94 = Yt78;
        const u32 Yt95 = Yt80;
        const u32 Yt96 = Yt82;
        const u32 Yt97 = Yt84;
        const u32 Yt98 = Yt86;
        const u32 Yt99 = Yt88;
        const u32 Yt100 = Yt89 ^ Yt90 ^ Yt91;
        const u32 Yt101 = (Yt89 & Yt90) | (Yt91 & (Yt89 ^ Yt90));
        const u32 Yt102 = Yt92 ^ Yt93 ^ Yt94;
        const u32 Yt103 = (Yt92 & Yt93) | (Yt94 & (Yt92 ^ Yt93));
        const u32 Yt104 = Yt95 ^ Yt96 ^ Yt97;
        const u32 Yt105 = (Yt95 & Yt96) | (Yt97 & (Yt95 ^ Yt96));
        const u32 Yt106 = Yt98 ^ Yt99 ^ Yt100;
        const u32 Yt107 = (Yt98 & Yt99) | (Yt100 & (Yt98 ^ Yt99));
        const u32 Yt108 = Yt102 ^ Yt104 ^ Yt106;
        const u32 Yt109 = (Yt102 & Yt104) | (Yt106 & (Yt102 ^ Yt104));
        const u32 Yt110 = Yt101;
        const u32 Yt111 = Yt103;
        const u32 Yt112 = Yt105;
        const u32 Yt113 = Yt107;
        const u32 Yt114 = Yt109;
        const u32 Yt115 = Yt110 ^ Yt111 ^ Yt112;
        const u32 Yt116 = (Yt110 & Yt111) | (Yt112 & (Yt110 ^ Yt111));
        const u32 Yt117 = Yt113 ^ Yt114 ^ Yt115;
        const u32 Yt118 = (Yt113 & Yt114) | (Yt115 & (Yt113 ^ Yt114));
        const u32 Yt119 = ~0u;
        const u32 Yt120 = Yt116;
        const u32 Yt121 = Yt118;
        const u32 Yt122 = Yt119 ^ Yt120 ^ Yt121;
        const u32 Yt123 = (Yt119 & Yt120) | (Yt121 & (Yt119 ^ Yt120));
        const u32 Yt124 = Yt123;
        Y0 = Yt19;
        Y1 = Yt55;
        Y2 = Yt87;
        Y3 = Yt108;
        Y4 = Yt117;
        Y5 = Yt122;
    }
    const u32 MSt1 = ((l1 << 3) | D4);
    const u32 MSt2 = ~(((l1 << 3) | (l3 << 1)));
    const u32 MSt3 = ((l1 << 2) | (D4 >> 1));
    const u32 MSt4 = ~(((l1 << 2) | l3));
    const u32 MSt5 = (D2 | (D4 >> 2));
    const u32 MSt6 = ~(((l1 << 1) | (l3 >> 1)));
    const u32 MSt7 = MSt1 ^ MSt2 ^ MSt3;
    const u32 MSt8 = (MSt1 & MSt2) | (MSt3 & (MSt1 ^ MSt2));
    const u32 MSt9 = MSt4 ^ MSt5 ^ MSt6;
    const u32 MSt10 = (MSt4 & MSt5) | (MSt6 & (MSt4 ^ MSt5));
    const u32 MSt11 = MSt7 ^ MSt9;
    const u32 MSt12 = MSt7 & MSt9;
    const u32 MSt13 = ~0u;
    const u32 MSt14 = MSt8;
    const u32 MSt15 = MSt10;
    const u32 MSt16 = MSt12;
    const u32 MSt17 = MSt13 ^ MSt14 ^ MSt15;
    const u32 MSt18 = (MSt13 & MSt14) | (MSt15 & (MSt13 ^ MSt14));
    const u32 MSt19 = MSt16 ^ MSt17;
    const u32 MSt20 = MSt16 & MSt17;
    const u32 MSt21 = MSt18;
    const u32 MSt22 = MSt20;
    const u32 MSt23 = MSt21 ^ MSt22;
    const u32 MSt24 = MSt21 & MSt22;
    const u32 MSt25 = MSt24;
    const u32 MLt1 = ((l1 << 5) | D6);
    const u32 MLt2 = ~(((l1 << 5) | (l3 << 3)));
    const u32 MLt3 = (D4 | (l3 << 1));
    const u32 MLt4 = ~(((l1 << 3) | (l3 << 1)));
    const u32 MLt5 = ((l1 << 1) | (D4 >> 2));
    const u32 MLt6 = ~(((l1 << 1) | (l3 >> 1)));
    const u32 MLt7 = ((D2 >> 1) | (D4 >> 3));
    const u32 MLt8 = ~((l1 | (l3 >> 2)));
    const u32 MLt9 = (D2 | (l1 >> 1));
    const u32 MLt10 = ~(((l1 >> 1) | (l3 >> 3)));
    const u32 MLt11 = MLt1 ^ MLt2 ^ MLt3;
    const u32 MLt12 = (MLt1 & MLt2) | (MLt3 & (MLt1 ^ MLt2));
    const u32 MLt13 = MLt4 ^ MLt5 ^ MLt6;
    const u32 MLt14 = (MLt4 & MLt5) | (MLt6 & (MLt4 ^ MLt5));
    const u32 MLt15 = MLt7 ^ MLt8 ^ MLt9;
    const u32 MLt16 = (MLt7 & MLt8) | (MLt9 & (MLt7 ^ MLt8));
    const u32 MLt17 = MLt10 ^ MLt11 ^ MLt13;
    const u32 MLt18 = (MLt10 & MLt11) | (MLt13 & (MLt10 ^ MLt11));
    const u32 MLt19 = MLt15 ^ MLt17;
    const u32 MLt20 = MLt15 & MLt17;
    const u32 MLt21 = MLt12;
    const u32 MLt22 = MLt14;
    const u32 MLt23 = MLt16;
    const u32 MLt24 = MLt18;
    const u32 MLt25 = MLt20;
    const u32 MLt26 = MLt21 ^ MLt22 ^ MLt23;
    const u32 MLt27 = (MLt21 & MLt22) | (MLt23 & (MLt21 ^ MLt22));
    const u32 MLt28 = MLt24 ^ MLt25 ^ MLt26;
    const u32 MLt29 = (MLt24 & MLt25) | (MLt26 & (MLt24 ^ MLt25));
    const u32 MLt30 = MLt27;
    const u32 MLt31 = MLt29;
    const u32 MLt32 = MLt30 ^ MLt31;
    const u32 MLt33 = MLt30 & MLt31;
    const u32 MLt34 = MLt33;
    const u32 St1 = r0;
    const u32 St2 = MSt11;
    const u32 St3 = (MSt11 & G3);
    const u32 St4 = (Y0 << 1);
    const u32 St5 = AS0;
    const u32 St6 = __funnelshift_r(r0, r0, 11);
    const u32 St7 = St2 ^ St3 ^ St4;
    const u32 St8 = (St2 & St3) | (St4 & (St2 ^ St3));
    const u32 St9 = St5 ^ St6 ^ St7;
    const u32 St10 = (St5 & St6) | (St7 & (St5 ^ St6));
    const u32 St11 = MSt19;
    const u32 St12 = (MSt19 & G3);
    const u32 St13 = (Y1 << 1);
    const u32 St14 = AS1;
    const u32 St15 = St8;
    const u32 St16 = St10;
    const u32 St17 = St11 ^ St12 ^ St13;
    const u32 St18 = (St11 & St12) | (St13 & (St11 ^ St12));
    const u32 St19 = St14 ^ St15 ^ St16;
    const u32 St20 = (St14 & St15) | (St16 & (St14 ^ St15));
    const u32 St21 = St17 ^ St19;
    const u32 St22 = St17 & St19;
    const u32 St23 = MSt23;
    const u32 St24 = (MSt23 & G3);
    const u32 St25 = (Y2 << 1);
    const u32 St26 = AS2;
    const u32 St27 = St18;
    const u32 St28 = St20;
    const u32 St29 = St22;
    const u32 St30 = St23 ^ St24 ^ St25;
    const u32 St31 = (St23 & St24) | (St25 & (St23 ^ St24));
    const u32 St32 = St26 ^ St27 ^ St28;
    const u32 St33 = (St26 & St27) | (St28 & (St26 ^ St27));
    const u32 St34 = St29 ^ St30 ^ St32;
    const u32 St35 = (St29 & St30) | (St32 & (St29 ^ St30));
    const u32 St36 = MSt25;
    const u32 St37 = (MSt25 & G3);
    const u32 St38 = (Y3 << 1);
    const u32 St39 = AS3;
    const u32 St40 = St31;
    const u32 St41 = St33;
    const u32 St42 = St35;
    const u32 St43 = St36 ^ St37 ^ St38;
    const u32 St44 = (St36 & St37) | (St38 & (St36 ^ St37));
    const u32 St45 = St39 ^ St40 ^ St41;
    const u32 St46 = (St39 & St40) | (St41 & (St39 ^ St40));
    const u32 St47 = St42 ^ St43 ^ St45;
    const u32 St48 = (St42 & St43) | (St45 & (St42 ^ St43));
    const u32 St49 = (Y4 << 1);
    const u32 St50 = AS4;
    const u32 St51 = St44;
    const u32 St52 = St46;
    const u32 St53 = St48;
    const u32 St54 = St49 ^ St50 ^ St51;
    const u32 St55 = (St49 & St50) | (St51 & (St49 ^ St50));
    const u32 St56 = St52 ^ St53 ^ St54;
    const u32 St57 = (St52 & St53) | (St54 & (St52 ^ St53));
    const u32 St58 = (Y5 << 1);
    const u32 St59 = AS5;
    const u32 St60 = St55;
    const u32 St61 = St57;
    const u32 St62 = St58 ^ St59 ^ St60;
    const u32 St63 = (St58 & St59) | (St60 & (St58 ^ St59));
    const u32 St64 = St61 ^ St62;
    const u32 St65 = St61 & St62;
    const u32 Lt1 = r1;
    const u32 Lt2 = MLt19;
    const u32 Lt3 = (MLt19 & G3);
    const u32 Lt4 = (Y0 << 3);
    const u32 Lt5 = AL0;
    const u32 Lt6 = __funnelshift_r(r1, r1, 11);
    const u32 Lt7 = Lt2 ^ Lt3 ^ Lt4;
    const u32 Lt8 = (Lt2 & Lt3) | (Lt4 & (Lt2 ^ Lt3));
    const u32 Lt9 = Lt5 ^ Lt6 ^ Lt7;
    const u32 Lt10 = (Lt5 & Lt6) | (Lt7 & (Lt5 ^ Lt6));
    const u32 Lt11 = MLt28;
    const u32 Lt12 = (MLt28 & G3);
    const u32 Lt13 = (Y1 << 3);
    const u32 Lt14 = AL1;
    const u32 Lt15 = Lt8;
    const u32 Lt16 = Lt10;
    const u32 Lt17 = Lt11 ^ Lt12 ^ Lt13;
    const u32 Lt18 = (Lt11 & Lt12) | (Lt13 & (Lt11 ^ Lt12));
    const u32 Lt19 = Lt14 ^ Lt15 ^ Lt16;
    const u32 Lt20 = (Lt14 & Lt15) | (Lt16 & (Lt14 ^ Lt15));
    const u32 Lt21 = Lt17 ^ Lt19;
    const u32 Lt22 = Lt17 & Lt19;
    const u32 Lt23 = MLt32;
    const u32 Lt24 = (MLt32 & G3);
    const u32 Lt25 = (Y2 << 3);
    const u32 Lt26 = AL2;
    const u32 Lt27 = Lt18;
    const u32 Lt28 = Lt20;
    const u32 Lt29 = Lt22;
    const u32 Lt30 = Lt23 ^ Lt24 ^ Lt25;
    const u32 Lt31 = (Lt23 & Lt24) | (Lt25 & (Lt23 ^ Lt24));
    const u32 Lt32 = Lt26 ^ Lt27 ^ Lt28;
    const u32 Lt33 = (Lt26 & Lt27) | (Lt28 & (Lt26 ^ Lt27));
    const u32 Lt34 = Lt29 ^ Lt30 ^ Lt32;
    const u32 Lt35 = (Lt29 & Lt30) | (Lt32 & (Lt29 ^ Lt30));
    const u32 Lt36 = MLt34;
    const u32 Lt37 = (MLt34 & G3);
    const u32 Lt38 = (Y3 << 3);
    const u32 Lt39 = AL3;
    const u32 Lt40 = Lt31;
    const u32 Lt41 = Lt33;
    const u32 Lt42 = Lt35;
    const u32 Lt43 = Lt36 ^ Lt37 ^ Lt38;
    const u32 Lt44 = (Lt36 & Lt37) | (Lt38 & (Lt36 ^ Lt37));
    const u32 Lt45 = Lt39 ^ Lt40 ^ Lt41;
    const u32 Lt46 = (Lt39 & Lt40) | (Lt41 & (Lt39 ^ Lt40));
    const u32 Lt47 = Lt42 ^ Lt43 ^ Lt45;
    const u32 Lt48 = (Lt42 & Lt43) | (Lt45 & (Lt42 ^ Lt43));
    const u32 Lt49 = (Y4 << 3);
    const u32 Lt50 = AL4;
    const u32 Lt51 = Lt44;
    const u32 Lt52 = Lt46;
    const u32 Lt53 = Lt48;
    const u32 Lt54 = Lt49 ^ Lt50 ^ Lt51;
    const u32 Lt55 = (Lt49 & Lt50) | (Lt51 & (Lt49 ^ Lt50));
    const u32 Lt56 = Lt52 ^ Lt53 ^ Lt54;
    const u32 Lt57 = (Lt52 & Lt53) | (Lt54 & (Lt52 ^ Lt53));
    const u32 Lt58 = (Y5 << 3);
    const u32 Lt59 = AL5;
    const u32 Lt60 = Lt55;
    const u32 Lt61 = Lt57;
    const u32 Lt62 = Lt58 ^ Lt59 ^ Lt60;
    const u32 Lt63 = (Lt58 & Lt59) | (Lt60 & (Lt58 ^ Lt59));
    const u32 Lt64 = Lt61 ^ Lt62;
    const u32 Lt65 = Lt61 & Lt62;
    const u32 D2S1 = D2 >> 1, D2S2 = D2 >> 2, D4S1 = D4 >> 1, D4S3 = D4 >> 3, D6S3 = D6 >> 3;
    const u32 L1S = l1 >> 1, L3S = l3 >> 1, W0S = w0 >> 1, W1S = w1 >> 1;
    const u32 valid = (1u << (len - 1)) - 1, V1 = valid & ~1u, V3 = valid & ~7u;
    u32 c1 = l1, c3 = l3;
    for (;;) {
        if ((c1 | c3) == 0) return -1;
        u32 cs = c1, cl = c3;
        #define KEEP(sb, lb) { const u32 a_ = cs & (sb), b_ = cl & (lb); const bool any_ = (a_ | b_) != 0; cs = any_ ? a_ : cs; cl = any_ ? b_ : cl; }
        KEEP(St64, Lt64)
        KEEP(St56, Lt56)
        KEEP(St47, Lt47)
        KEEP(St34, Lt34)
        KEEP(St21, Lt21)
        KEEP(St9, Lt9)
        KEEP(St1, Lt1)
        #undef KEEP
        const u32 r2 = r0 >> 16;
        const u32 rot = ((r2 & 0xFFFFu) * (u32)len) >> 16;
        const bool leap = cl != 0;
        const u32 mm = leap ? cl : cs;
        const int x = (int)((rot + __ffs(__funnelshift_r(mm, mm, rot)) - 1) & 31);
        const int y = x - (leap ? 3 : 1);
        const u32 bit = 1u << y, low = (1u << x) - 1;
        const u32 a1_ = (l1 & low) | (L1S & ~low), a3_ = (l3 & low) | (L3S & ~low);
        const u32 f1 = leap ? (bit & D4S3) | ((bit << 1) & D2S2) | ((bit << 3) & D2S1) : bit & D2S1;
        const u32 f3 = leap ? (bit & D6S3) | ((bit << 3) & L1S) | ((bit * 0x30u) & D4S1) : (bit * 7u) & D4S1;
        const u32 k1 = leap ? bit * 0x0Bu : bit, k3 = leap ? bit * 0x39u : bit * 7u;
        const u32 nl1 = ((a1_ & ~k1) | f1) & V1, nl3 = ((a3_ & ~k3) | f3) & V3;
        if ((nl1 | nl3) == 0 && len > 2) return -1;
        u32 m0 = (w0 & low) | (W0S & ~low), m1 = (w1 & low) | (W1S & ~low);
        m0 = (m0 & ~bit) | ((w0 >> (x - y)) & bit);
        m1 = (m1 & ~bit) | ((w1 >> (x - y)) & bit);
        u32 e = 0;
        #pragma unroll
        for (int b = 0; b < 4; b++) e |= B[b] ^ (u32)((int)(B[b] << (31 - y)) >> 31);
        const u32 pe = ~e;
        const u32 p = (((pe & low) | ((pe >> 1) & ~low)) & ~bit) | ((pe >> (x - y)) & bit);
        const u32 v0 = m0 ^ (p & ~(m0 & m1)), v1 = m1 | (m0 & p);   // saturating: target ranks reach 3 with a card still left
        #pragma unroll
        for (int b = 0; b < 6; b++) {
            const u32 v = B[b];
            B[b] = (((v & low) | ((v >> 1) & ~low)) & ~bit) | ((v >> (x - y)) & bit);
        }
        len--;
        l1 = nl1; l3 = nl3;
        w0 = v0; w1 = v1;
        return x << 1 | (leap ? 1 : 0);
    }
}
// A group's kept row, sliced: 10 words of the thread's 16 (word k at w[k * PREFIX_THREADS]).
__device__ __forceinline__ void slice_keep(u32* w, const u32 (&B)[6], u32 o1, u32 o3, u32 w0, u32 w1) {
    #pragma unroll
    for (int j = 0; j < 6; j++) w[j * PREFIX_THREADS] = B[j];
    w[6 * PREFIX_THREADS] = o1; w[7 * PREFIX_THREADS] = o3; w[8 * PREFIX_THREADS] = w0; w[9 * PREFIX_THREADS] = w1;
}
__device__ __forceinline__ void slice_kept(const u32* w, u32 (&B)[6], u32& o1, u32& o3, u32& w0, u32& w1) {
    #pragma unroll
    for (int j = 0; j < 6; j++) B[j] = w[j * PREFIX_THREADS];
    o1 = w[6 * PREFIX_THREADS]; o3 = w[7 * PREFIX_THREADS]; w0 = w[8 * PREFIX_THREADS]; w1 = w[9 * PREFIX_THREADS];
}
#endif

// Attempts with prefixes and searches in different warps: threads that play prefixes and threads that search, in the
// same warp, take turns, so each turn runs both steps at about half the warp's width. Here some of a block's warps play
// prefixes, all 32 threads stepping together (prefixes have a fixed length), and put each residual worth searching in a
// queue in shared memory; the others search, each thread taking the next residual as soon as it has finished one.
// Queue entry i (counted over the launch) lives in slot i % QSIZE, whose number says where it is: i, the slot is free
// for it; i + 1, written; i + QSIZE, copied out, and the slot free for entry i + QSIZE. Prefix warps reserve runs of
// free entries and search warps take runs of written ones, a compare-and-swap each; slots are freed out of order, by
// threads of different warps. Prefix threads keep their rows in registers, and in their words of s_mem the row a
// group's shared moves left (keep_row); search threads keep their asleep stacks in their words of s_mem.
// With shared early moves (SHARE_G, SHARE_K) a prefix thread takes a group of a deal's tries at a time and plays them
// one after another: the shared moves once, then each try's own moves from the row they left. Its attempts' first
// moves come from the group's random numbers; a won line replays the same way.
// The queue's shared memory leaves room for 8 blocks of 96 on the T550 (1 prefix warp to 2 search warps; the default is
// now 8 blocks of 64, 1 to 1, which the search step made faster: docs/BENCHMARKS.md, "shared early prefix moves"), so the kernel may take 80 registers (24 warps a multiprocessor);
// at 72 it spilled.
extern "C" __global__ void __launch_bounds__(BLOCK, 768 / BLOCK) attempts_split(
    const u8* __restrict__ deals, const u64* __restrict__ deal_masks, u32 n_deals, u64 inv_deals, u32 tries,
    u32 prefix, u32 tries2, u32 prefix2, u32 budget, u32 conn_gate, u32 leftmost, u32 leap_shift,
    u64 seed, u32* next_attempt, u32* won, u8* lines, u64* deal_counts, u64* profile)
{
    // 16 words a thread: a prefix thread's kept row (keep_row), then a search thread's asleep moves at each depth
    // (slides | leaps << 16), so that a warp's 32 threads touch 32 different banks with one access each.
    __shared__ u32 s_mem[16 * BLOCK + 16 * (BLOCK - PREFIX_THREADS)];
    #define ASLEEP(d) s_mem[16 * PREFIX_THREADS + (d) * (BLOCK - PREFIX_THREADS) + t - PREFIX_THREADS]
    #define VIAS(d) s_mem[16 * BLOCK + (d) * (BLOCK - PREFIX_THREADS) + t - PREFIX_THREADS]
    __shared__ u64 q_lo[QSIZE], q_hi[QSIZE], q_present[QSIZE];   // the residual and its cards
    __shared__ u32 q_a[QSIZE], q_seq[QSIZE];                      // its attempt; the slot's number
    __shared__ u32 q_moves[QSIZE];                                 // its prefix's moves
    __shared__ u32 q_resv, q_head, q_closed;                      // entries reserved, entries taken, prefix warps done
    #if REC && SHR_WIN && SLICED
    __shared__ u32 s_scan[5 * (PREFIX_THREADS / 32)];             // each prefix warp's scan state (the same in all its threads)
    __shared__ u32 q_rec[5 * QSIZE];                              // the entry's own moves, a byte each
    #define RECON 1
    #else
    #define RECON 0
    #endif
    const int t = threadIdx.x;
    const u32 lane = t & 31, FULL = 0xFFFFFFFFu, below_me = (1u << lane) - 1, prefix_warps = PREFIX_THREADS / 32;
    if (t == 0) q_resv = q_head = q_closed = 0;
    for (int i = t; i < QSIZE; i += BLOCK) q_seq[i] = i;
    __syncthreads();
    if (prefix > MAX_PREFIX) prefix = MAX_PREFIX;
    if (prefix2 > MAX_PREFIX) prefix2 = MAX_PREFIX;
    // The moves a group's tries share: none when a rung's prefix is shorter.
    const int share = SHARING && (int)min(prefix, tries2 ? prefix2 : prefix) >= SHARE_K ? SHARE_K : 0;
    const bool rec_on = RECON && share && (int)max(prefix, tries2 ? prefix2 : prefix) - share <= 20;
    // Profiling: each warp's time in each part of its loop, by thread 0 of the warp, and how many of its threads
    // stepped (P_PREFIX_STEPS or P_STEPS), waited on the queue, or took and skipped attempts.
    PROF(const u64 t_start = now_ns(); u64 pf[P_SKIP + 1] = {0}, turns = 0, stepping = 0, waited = 0, taken = 0,
         skipped = 0, c0 = clock64(), c1;)
    #define WLAP(slot) PROF(c1 = clock64(); if (lane == 0) pf[slot] += c1 - c0; c0 = c1;)

    if (__ballot_sync(FULL, t < 32 * prefix_warps) != 0) {
        // Slots P_FETCH, P_LOAD, P_PREFIX and P_GATE (queueing, with waiting for room) of pf.
        // The warp's threads go in lockstep. Each takes a group of tries (numbered like attempts: group j of deal d is
        // j * deals + d); together they play the groups' shared moves, keep the rows, then play try after try from
        // them, each thread on its own group's next try, queueing the residuals before they start the next. A thread
        // whose group ends early (its deal won, or the group short) waits for the warp's next groups. So the warp
        // takes work only once a cycle, and a try's own moves touch only the words its row can still hold.
        const u32 all_tries = tries + tries2;
        const u64 total = n_deals * (u64)((all_tries + SHARE_G - 1) / SHARE_G);
        const int kmax = (int)max(prefix, tries2 ? prefix2 : prefix);
        u32* const kw = s_mem + t;
        #if RECON
        u32* const ks = s_scan + (t >> 5);
        const int SCW = PREFIX_THREADS / 32;
        #else
        u32* const ks = kw + 10 * PREFIX_THREADS;
        const int SCW = PREFIX_THREADS;
        #endif
        // A warp takes attempts 32 at a time, with one atomic and one load of their deals' won flags; attempts it can't use
        // yet wait, one per thread, for the next threads that need one:
        u32 held = 0;              // an attempt this thread holds for the warp, when its bit of `pend` is set
        u32 pend = 0;              // the same in every thread of the warp, as is `exhausted`
        #pragma unroll
        for (int w = 0; w < 5; w++) ks[w * SCW] = 0;
        // Passes over the deals as taken: from b_pass on, each stands for BUNDLE of a deal's groups.
        const u32 g_all = (all_tries + SHARE_G - 1) / SHARE_G, b_pass = BUNDLE_TRIES / SHARE_G;
        const u64 v_total = n_deals * (u64)(g_all <= b_pass ? g_all : b_pass + (g_all - b_pass + BUNDLE - 1) / BUNDLE);
        const u32 st_from = (n_deals & 31) == 0 && n_deals >= 32 * STRETCH_WORDS ? (STRETCH_TRIES / SHARE_G) * n_deals : 0xFFFFFFFFu;
        bool exhausted = false;
        u32 R[RW];
        #pragma unroll
        for (int j = 0; j < RW; j++) R[j] = SENT4;
        for (;;) {
            u32 need = FULL, got = 0, a = 0;
            // The words of the stretch still to visit, its first group, the last group taken, the bundles to give yet, the lanes whose deal is in one.
            u32 st_left = ks[0], st_base = ks[SCW], last_base = ks[2 * SCW], rep = ks[3 * SCW], rep_mask = ks[4 * SCW];
            while (need) {
                if (!pend && rep) {
                    rep--;
                    held += n_deals;
                    u32 hd, hj;
                    deal_and_try(held, n_deals, inv_deals, hd, hj);
                    const bool more = ((rep_mask >> lane) & 1) && (u64)held < total && !*((volatile u32*)&won[hd]);
                    pend = rep_mask = __ballot_sync(FULL, more);
                    if (!pend) rep = 0;
                    continue;
                }
                if (!pend) {
                    if (exhausted && !st_left) break;
                    u32 base = 0;
                    if (st_left) {
                        base = st_base + 32 * (__ffs(st_left) - 1);
                        st_left &= st_left - 1;
                    } else {
                        const u32 step = last_base >= st_from ? 32 * STRETCH_WORDS : 32u;
                        if (lane == 0) base = atomicAdd(next_attempt, step);
                        base = __shfl_sync(FULL, base, 0);
                        last_base = base;
                        exhausted = __ballot_sync(FULL, (u64)base + step >= v_total) != 0;
                        if (step != 32u) {
                            u32 d0, j0;
                            deal_and_try(base, n_deals, inv_deals, d0, j0);
                            const u32 wd = d0 + 32 * lane, dw = wd >= n_deals ? wd - n_deals : wd;
                            const bool here = (u64)base + 32 * lane < v_total;
                            const bool todo = here && *(volatile u32*)won_bits(lines, dw >> 5) != ~0u;
                            PROF(if (here && !todo) { taken += 32 * SHARE_G; skipped += 32 * SHARE_G; })
                            st_left = __ballot_sync(FULL, todo);
                            st_base = base;
                            continue;
                        }
                    }
                    u64 s = (u64)base + lane;
                    u32 d, j;
                    deal_and_try((u32)s, n_deals, inv_deals, d, j);
                    bool ok = s < v_total && !*((volatile u32*)&won[d]);
                    PROF(if (s < total) { u32 g = min((u32)SHARE_G, all_tries - j * SHARE_G); taken += g; if (!ok) skipped += g; })
                    const bool deep = j >= b_pass;
                    if (deep) s = (u64)(b_pass + (j - b_pass) * BUNDLE) * n_deals + d;
                    pend = __ballot_sync(FULL, ok);
                    held = (u32)s;
                    rep_mask = __ballot_sync(FULL, ok && deep);
                    rep = rep_mask ? BUNDLE - 1 : 0;
                    // A thread that needs one and just found one keeps it.
                    u32 own = pend & need;
                    if ((own >> lane) & 1) a = held;
                    got |= own;
                    pend &= ~own;
                    need &= ~own;
                    continue;
                }
                u32 from = __ffs(pend) - 1, to = __ffs(need) - 1;
                u32 s = __shfl_sync(FULL, held, from);
                if (lane == to) a = s;
                got |= 1u << to;
                pend &= pend - 1;
                need &= need - 1;
            }
            ks[0] = st_left; ks[SCW] = st_base; ks[2 * SCW] = last_base; ks[3 * SCW] = rep; ks[4 * SCW] = rep_mask;
            WLAP(P_FETCH);
            if (!got) break;
            bool alive = (got >> lane) & 1;
            u32 deal = 0, left = 0, rng = 1, k_moves = 0;   // left: the group's tries after this one
            int moves = 0, len = 0, klen = 0;              // klen: the piles the shared moves left
            #if SLICED
            u32 B[6], o1, o3, w0, w1;                      // the row in own moves, its masks and weights
            #endif
            u64 l1 = 0, l3 = 0, c1 = 0, c3 = 0;            // c1, c3: the moves still to draw from
            u32 rc = 0;                                    // the others left of each rank
            u64 t1 = 0, t2 = 0, t3 = 0;
            if (alive) {
                u32 j;
                deal_and_try(a, n_deals, inv_deals, deal, j);    // a is the group's number here
                left = min((u32)SHARE_G, all_tries - j * SHARE_G) - 1;
                a = j * SHARE_G * n_deals + deal;               // its first try
                if (j < (u32)SOLO_GROUPS) left = 0;
            }
            for (int g = share ? -1 : 0; g < SHARE_G; g++) {
                // The group's next try, unless it has no more or the deal was won meanwhile.
                if (g > 0 && alive) {
                    if (left == 0) {
                        alive = false;
                    } else {
                        a += n_deals;
                        if (*((volatile u32*)&won[deal])) {
                            PROF(skipped += left;)
                            alive = false;
                        }
                        left--;
                    }
                }
                if (!__any_sync(FULL, alive)) break;
                if (g < 0 || !share) {
                    // From the deal: the shared moves, or a whole prefix when there are none.
                    if (alive || SLICED) {       // on every thread when sliced, so that no word of the row stays live through own moves
                        len = rrow_load(R, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, l1, l3);
                        c1 = l1; c3 = l3;
                        rc = RANKS_FULL;
                        t1 = t2 = t3 = 0;
                        moves = 0;
                        k_moves = g < 0 ? share : a < tries * n_deals ? prefix : prefix2;
                        rng = g < 0 ? group_rng(seed, a) : own_rng(seed, a);
                    }
                    {
                        // The group's copy of the deal's word, and above it its targets. A best of 15 is never beaten.
                        u32 gw = 15u << 13;
                        if (alive && a < (u32)LEARN_UNTIL * n_deals) {
                            const u32 w = *(volatile u32*)learn_word(lines, deal);
                            gw = w | (a < (u32)LEARN_FROM * n_deals ? 0u : (w & 0x1FFFu) << 17);
                        }
                        const u32 targets = gw >> 17;
                        if (__any_sync(FULL, targets != 0)) {
                            rc -= LEARN_CLASSES * learn_spread(targets);
                            t1 = rrow_places(R, targets);
                            #if LEARN_CLASSES >= 2
                            t2 = t1;
                            #endif
                        }
                        kw[15 * PREFIX_THREADS] = gw;
                    }
                    WLAP(P_LOAD);
                    #if SHR_WIN && SLICED
                    if (g < 0) {
                        u32 T[6], oc1 = (u32)l1, oc3 = (u32)l3, u0 = (u32)(t1 >> 32), u1 = 0;
                        win_load(R, B, T);
                        o1 = oc1; o3 = oc3; w0 = (u32)t1; w1 = 0;
                        for (int m = share; m > 0 || __any_sync(FULL, alive && moves < (int)k_moves); m--) {
                            PROF(u32 nm = __popc(__ballot_sync(FULL, alive && moves < (int)k_moves)); if (lane == 0) { turns++; stepping += nm; })
                            if (alive && moves < (int)k_moves) {
                                int played = win_try(B, T, len, o1, o3, oc1, oc3, w0, w1, u0, u1, rng, leap_shift);
                                if (played == -1) k_moves = moves;
                                else if (played >= 0) moves++;
                            }
                        }
                        WLAP(P_PREFIX);
                        slice_keep(kw, B, o1, o3, w0, w1);
                        klen = len;
                        WLAP(P_LOAD);
                        continue;
                    }
                    #endif
                    for (int m = g < 0 ? share : kmax; m > 0 || __any_sync(FULL, alive && moves < (int)k_moves); m--) {
                        PROF(u32 nm = __popc(__ballot_sync(FULL, alive && moves < (int)k_moves)); if (lane == 0) { turns++; stepping += nm; })
                        if (alive && moves < (int)k_moves) {
                            int played = rrow_try<RW, 4, u64>(R, len, l1, l3, c1, c3, rc, rng, leap_shift, t1, t2, t3);
                            if (played == -1) k_moves = moves;   // all lose
                            else if (played >= 0) moves++;
                        }
                    }
                    WLAP(P_PREFIX);
                    if (g < 0) {
                        #if SLICED
                        slice_row<SLICE_W>(R, B);
                        o1 = (u32)l1; o3 = (u32)l3; w1 = (u32)t2; w0 = ((u32)t1 & ~(u32)t2) | (u32)t3;
                        slice_keep(kw, B, o1, o3, w0, w1);
                        klen = len;
                        #else
                        if (SHARING && alive && moves == share) {
                            keep_row(kw, R, l1, l3, rc, t1, t2, t3);
                            klen = len;
                        }
                        #endif
                        WLAP(P_LOAD);
                        continue;
                    }
                }
                if (SHARING && share) {
                    #if SLICED
                    if (g > 0) {             // on every thread, so that the row isn't live through the rest of the loop
                        slice_kept(kw, B, o1, o3, w0, w1);
                        len = klen;
                    }
                    #endif
                    if (alive) {
                        if (g == 0 && moves < share) {
                            // Stuck before the shared moves were all played: the group's other tries would play the
                            // same moves and search the same residual with the same random numbers, so they end with
                            // this one.
                            if (left) atomicAdd(&deal_counts[deal], (u64)left * ((u64)moves << 16 | 1u));
                            left = 0;
                            k_moves = moves;
                        } else {
                            #if !SLICED
                            if (g > 0) {
                                kept_row(kw, R, l1, l3, rc, t1, t2, t3);
                                len = klen;
                            }
                            #endif
                            moves = share;
                            k_moves = a < tries * n_deals ? prefix : prefix2;
                            rng = own_rng(seed, a);
                        }
                    }
                    WLAP(P_LOAD);
                    int m = share;
                    #define OWN_MOVES(W, BITS, M, A, B, C1, C3, T1, T2, T3)                                                  \
                        for (; m < kmax || __any_sync(FULL, alive && moves < (int)k_moves); m++) {                                                                      \
                            PROF(u32 nm = __popc(__ballot_sync(FULL, alive && moves < (int)k_moves)); if (lane == 0) { turns++; stepping += nm; }) \
                            if (alive && moves < (int)k_moves) {                                                     \
                                int played = rrow_try<W, BITS, M>(R, len, A, B, C1, C3, rc, rng, leap_shift, T1, T2, T3); if (played == -1) k_moves = moves; \
                                else if (played >= 0) moves++;                                                                        \
                            }                                                                                        \
                        }
                    // One copy of the code, on the longest row a try can have (more copies, each for fewer cards, do
                    // less work but ran slower: code size shows in the instruction cache).
                    #if SLICED
                    for (; m < kmax; m++) {
                        PROF(u32 nm = __popc(__ballot_sync(FULL, alive && moves < (int)k_moves)); if (lane == 0) { turns++; stepping += nm; })
                        if (alive && moves < (int)k_moves) {
                            #if RECON
                            const int played = pat_move(B, len, o1, o3, w0, w1, rng, m >= 30);
                            if (played < 0) k_moves = moves;
                            else {
                                const int j = moves - share;
                                if (rec_on) ((u8*)&kw[(10 + (j >> 2)) * PREFIX_THREADS])[j & 3] = (u8)played;
                                moves++;
                            }
                            #else
                            if (pat_move(B, len, o1, o3, w0, w1, rng, m >= 30) < 0) k_moves = moves;
                            else moves++;
                            #endif
                        }
                    }
                    unslice_row(B, len, R);
                    #elif SHARE_K >= 20
                    u32 o1 = (u32)l1, o3 = (u32)l3, oc1 = o1, oc3 = o3, ot1 = (u32)t1, ot2 = (u32)t2, ot3 = (u32)t3;
                    OWN_MOVES(OWN_W, (SHARE_K >= 22 ? 3 : 4), u32, o1, o3, oc1, oc3, ot1, ot2, ot3)
                    #else
                    OWN_MOVES(OWN_W, 4, u64, l1, l3, c1, c3, t1, t2, t3)
                    #endif
                    #undef OWN_MOVES
                    WLAP(P_PREFIX);
                }
                #if SLICED
                if (!share) slice_row<4>(R, B);    // a rung too short to share moves: the row is still in words
                const u32 P[3] = {(B[0] & 0xFFFFu) | B[1] << 16, (B[2] & 0xFFFFu) | B[3] << 16, (B[4] & 0xFFFFu) | B[5] << 16};
                #endif
                // Stuck before the residual got small, or already disconnected: lost anyway.
                bool ready = false;
                u64 present = 0;
                if (alive) {
                    present = rrow_present(R);
                    bool linked = connected_test(present, R[0] & 0xFF, len, leftmost);
                    bool search = len <= RES && !(len > 1 && len <= (int)conn_gate && !linked);
                    u32 p0 = (u32)present, p1 = (u32)(present >> 32);
                    const u32 here = (p0 | (p0 >> 16) | p1 | (p1 >> 16)) & 0x1FFFu;
                    u32 ranks = __popc(here);
                    search = search && (len < 2 || ranks <= (u32)(a < tries * n_deals ? RANK_GATE : RANK_GATE2));
                    #if LEARN_GAP >= 0
                    const u32 gap_w = (kw[15 * PREFIX_THREADS] >> 13) & 15;
                    search = search && (len < 2 || gap_w == 15 || 14 - ranks + LEARN_GAP >= gap_w);
                    #endif
                    if (search && len > 1) {
                        ready = true;
                        // Better than the best this group knew of, or as good with something new: into the deal's word.
                        const u32 gw = kw[15 * PREFIX_THREADS], mine = 14 - ranks, was = (gw >> 13) & 15, lost = 0x1FFFu & ~here;
                        #if LEARN_TIES && LEARN_UNION
                        const bool level = mine == was && (lost & ~gw & 0x1FFFu) != 0;
                        #elif LEARN_TIES
                        const bool level = mine == was && lost != (gw & 0x1FFFu);
                        #else
                        const bool level = false;
                        #endif
                        #if LEARN_PLUS1
                        const bool close_by = mine + 1 == was && (lost & ~gw & 0x1FFFu) != 0;   // one rank worse, with something new
                        #else
                        const bool close_by = false;
                        #endif
                        if (mine > was || level || close_by) {
                            const u32 w = (LEARN_UNION && (level || close_by) ? (gw & 0x1FFFu) | lost : lost) | (close_by ? was : mine) << 13;
                            *(volatile u32*)learn_word(lines, deal) = w;
                            kw[15 * PREFIX_THREADS] = (gw & 0xFFFE0000u) | w;
                        }
                    } else {
                        atomicAdd(&deal_counts[deal], (u64)moves << 16 | 1u);
                        if (search && atomicCAS(&won[deal], 0u, a + 1) == 0u) {   // one pile left: won outright
                            learn_word(lines, deal)[1] = kw[15 * PREFIX_THREADS] >> 17;
                            #if RECON
                            if (rec_on && deal >= rec_low(n_deals))
                                write_own(lines, deal, share, moves - share, kw[10 * PREFIX_THREADS], kw[11 * PREFIX_THREADS], kw[12 * PREFIX_THREADS],
                                          kw[13 * PREFIX_THREADS], kw[14 * PREFIX_THREADS]);
                            #endif
                            atomicOr(won_bits(lines, deal >> 5), 1u << (deal & 31));
                        }
                    }
                }
                WLAP(P_PREFIX);
                // Queue the residuals, all of them before the warp goes on.
                #if SLICED
                const u32 (&planes)[3] = P;                // already the search's planes
                #else
                u32 planes[3];
                if (ready) bp_load(planes, R[0], R[1], R[2], R[3]);
                #endif
                for (u32 rd = __ballot_sync(FULL, ready); rd; rd = __ballot_sync(FULL, ready)) {
                    u32 r = 0, k = 0;
                    for (;;) {
                        if (lane == 0) r = *(volatile u32*)&q_resv;
                        r = __shfl_sync(FULL, r, 0);
                        u32 free = __ballot_sync(FULL, lane < QSIZE && ((volatile u32*)q_seq)[(r + lane) % QSIZE] == r + lane);
                        k = min((u32)__popc(rd), ~free ? (u32)__ffs(~free) - 1 : 32u);
                        if (k == 0) break;
                        u32 seen = 0;
                        if (lane == 0) seen = atomicCAS(&q_resv, r, r + k);
                        if (__ballot_sync(FULL, __shfl_sync(FULL, seen, 0) == r) != 0) break;
                    }
                    if (ready && __popc(rd & below_me) < k) {
                        u32 i = r + __popc(rd & below_me), slot = i % QSIZE;
                        q_lo[slot] = planes[0] | (u64)planes[1] << 32; q_hi[slot] = planes[2] | (u64)((R[0] & 0x3F) | (u32)len << 6 | ((u32)moves | (kw[15 * PREFIX_THREADS] >> 17) << 8) << 11) << 32;
                        q_present[slot] = present & ~(1ull << (R[0] & 63));
                        q_a[slot] = a; q_moves[slot] = deal;
                        #if RECON
                        #pragma unroll
                        for (int k = 0; k < 5; k++) q_rec[k * QSIZE + slot] = kw[(10 + k) * PREFIX_THREADS];
                        #endif
                        __threadfence_block();             // the entry before its number
                        ((volatile u32*)q_seq)[slot] = i + 1;
                        ready = false;
                    }
                    PROF(if (lane == 0) waited += __popc(rd) - k;)
                    if (k == 0) __nanosleep(QUEUE_WAIT_NS);
                }
                WLAP(P_GATE);
            }
        }
        if (lane == 0) atomicAdd(&q_closed, 1u);
        PROF(if (lane == 0) {
                 for (int i = P_FETCH; i <= P_GATE; i++) atomicAdd(&profile[i], pf[i]);
                 atomicAdd(&profile[P_PREFIX_WARP_STEPS], turns); atomicAdd(&profile[P_PREFIX_STEPS], stepping);
                 atomicAdd(&profile[P_BLOCKED], waited);
             }
             atomicAdd(&profile[P_FETCHES], taken); atomicAdd(&profile[P_SKIPS], skipped);)
    } else {
        // Slots P_RESULT (taking residuals and writing results), P_SEARCH and P_SKIP (waiting for residuals) of pf.
        enum { TAKE, SEARCH, FINISH, DONE };
        int phase = TAKE;
        u32 a = 0, deal = 0;
        u32 moves = 0;      // the prefix's moves, and above them its group's target ranks
        int len = 0, depth = 0;
        u32 P0 = 0, P1 = 0, left = 0;      // the cards present, but for pile 0's; pile 0's
        u32 C[3] = {0, 0, 0};
        u32 u = 0, used = 0, z = 0;        // this position's moves left to try, and those asleep: slides | leaps << 16
        bool all_done = false;
        u32 mv = 0;                        // the move tested and waiting to be played: bits 0-4 which, 5-10 the covered card, 11-16 the card out of P, bit 17 set
        bool win = false;
        #if RECON
        u32 rec[5] = {0, 0, 0, 0, 0};
        #endif
        for (;;) {
            u32 need = __ballot_sync(FULL, phase == TAKE || phase == FINISH);
            if (__popc(need) >= TAKE_MIN || (need && __popc(__ballot_sync(FULL, phase == SEARCH)) < TAKE_FLOOR)) {
            if (phase == FINISH) {
                atomicAdd(&deal_counts[deal], (u64)((moves & 0xFFu) + used) << 16 | 1u);
                if (win && atomicCAS(&won[deal], 0u, a + 1) == 0u)
                    { learn_word(lines, deal)[1] = moves >> 8; atomicOr(won_bits(lines, deal >> 5), 1u << (deal & 31)); write_search_f(lines, deal, moves & 0xFF, depth, &VIAS(0), BLOCK - PREFIX_THREADS);
                      #if RECON
                      if (rec_on && deal >= rec_low(n_deals)) write_own(lines, deal, share, (int)(moves & 0xFF) - share, rec[0], rec[1], rec[2], rec[3], rec[4]);
                      #endif
                    }
                win = false;
                phase = TAKE;
            }
                u32 closed = *(volatile u32*)&q_closed, head = 0, k = 0, drained = 0;
                for (;;) {
                    if (lane == 0) head = *(volatile u32*)&q_head;
                    head = __shfl_sync(FULL, head, 0);
                    u32 full = __ballot_sync(FULL, lane < QSIZE && ((volatile u32*)q_seq)[(head + lane) % QSIZE] == head + lane + 1);
                    k = min((u32)__popc(need), ~full ? (u32)__ffs(~full) - 1 : 32u);
                    if (k == 0) break;
                    u32 seen = 0;
                    if (lane == 0) seen = atomicCAS(&q_head, head, head + k);
                    if (__ballot_sync(FULL, __shfl_sync(FULL, seen, 0) == head) != 0) break;
                }
                if (k == 0 && (__ballot_sync(FULL, closed == prefix_warps) & 1)) {
                    drained = __ballot_sync(FULL, lane == 0 && head == *(volatile u32*)&q_resv) != 0;
                }
                if (phase == TAKE && __popc(need & below_me) < k) {
                    u32 i = head + __popc(need & below_me), slot = i % QSIZE;
                    u64 qlo = q_lo[slot], qhi = q_hi[slot];
                    C[0] = (u32)qlo; C[1] = (u32)(qlo >> 32); C[2] = (u32)qhi;
                    {
                        u32 info = (u32)(qhi >> 32);
                        u64 p = q_present[slot];
                        left = info & 0x3F; len = info >> 6 & 0x1F; moves = info >> 11;
                        P0 = (u32)p; P1 = (u32)(p >> 32);
                    }
                    a = q_a[slot]; deal = q_moves[slot];
                    #if RECON
                    #pragma unroll
                    for (int k = 0; k < 5; k++) rec[k] = q_rec[k * QSIZE + slot];
                    #endif
                    __threadfence_block();             // the copy before the slot is freed
                    ((volatile u32*)q_seq)[slot] = i + QSIZE;
                    z = 0; depth = 0; used = 0; mv = 0;
                    u = 1;                             // nothing to take back: masks below
                    phase = SEARCH;
                }
                // Whatever this warp is searching, it stops if its deal was won meanwhile (11% of the positions otherwise):
                // the warp is here every 20 turns or so, and has a memory access to wait for anyway.
                if (phase == SEARCH && (used >= budget || *((volatile u32*)&won[deal]))) phase = FINISH;
                if (drained && phase == TAKE) phase = DONE;
                PROF(if (lane == 0) waited += __popc(need) - k;)
                WLAP(P_RESULT);
                if (k == 0 && !drained && __all_sync(FULL, phase != SEARCH)) __nanosleep(200);
                WLAP(P_SKIP);
                all_done = __all_sync(FULL, phase == DONE);
            }
            PROF(u32 n = __popc(__ballot_sync(FULL, phase == SEARCH)); if (lane == 0) { turns++; stepping += n; })
            // One pass: the edit, then every thread's masks, then a test. A move that passes its test waits for the next
            // pass's edit, and a thread out of moves takes one back there, so all threads changing their rows run one
            // piece of code in a turn, where two blocks each ran for a third of them.
            if ((mv | u | depth) == 0 && phase == SEARCH) phase = FINISH;   // the residual is lost
            if (phase == SEARCH && (mv != 0 || u == 0)) {
                const bool fwd = mv != 0;
                u32 v = mv;
                if (!fwd) {
                    depth--;
                    z = ASLEEP(depth);
                    v = VIAS(depth);
                }
                u32 b = v & 31, pile = b & 15, dd = 1 + (b >> 3 & 2), y = pile - dd, cov = v >> 20 & 63, out = v >> 26;
                if (fwd) {
                    ASLEEP(depth) = z;
                    VIAS(depth) = v;
                    depth++;
                    u32 before = z ^ (1u << b), Y = 0x10001u << y, past = 2u << pile;
                    z = (before & (Y - 0x10001u)) | ((before >> 1) & 0x7FFF7FFFu & ~((past - 1) | ((past << 2) - 1) << 16))
                        | ((u | z) & (dd == 1 ? Y : 1u << y));
                }
                bp_edit(C, fwd, pile, y, dd, cov);
                u32 cb = 1u << (out & 31);
                bool up = (int)v < 0;
                P0 ^= up ? 0u : cb;
                P1 ^= up ? cb : 0u;
                left = y == 0 ? (fwd ? out : cov) : left;
                len += fwd ? -1 : 1;
                mv = 0;
            }
            {
                u32 ms, ml;
                bp_masks(C, len, ms, ml);
                u = (ms | ml << 16) & ~z;
            }
            if (phase == SEARCH && u != 0) {
                {
                    // Leaps first, then slides, the highest pile first: most searches run out of moves within their
                    // budget, so the order hardly matters to what they find, and a fixed one needs no random numbers.
                    u32 b = 31 - __clz(u), pile = b & 15, dd = 1 + (b >> 3 & 2), y = pile - dd;
                    u ^= 1u << b;
                    z |= 1u << b;
                    used++;
                    u32 out = bp_card(C, y == 0 ? pile : y);    // the card leaving P: the covered one, or the mover when it lands on pile 0
                    u32 cov = y == 0 ? left : out;
                    if (len == 2) {
                        VIAS(depth) = b;
                        win = true;
                        depth++;
                        phase = FINISH;
                    } else {
                        u32 nb = ~(1u << (out & 31));
                        bool up = out & 32;
                        if (alive_rest(P0 & (up ? 0xFFFFFFFFu : nb), P1 & (up ? nb : 0xFFFFFFFFu), y == 0 ? out : left))
                            mv = b | cov << 20 | out << 26;
                    }
                }
            }
            WLAP(P_SEARCH);

            WLAP(P_RESULT);
            if (all_done) break;
        }
        PROF(if (lane == 0) {
                 atomicAdd(&profile[P_SEARCH], pf[P_SEARCH]); atomicAdd(&profile[P_RESULT], pf[P_RESULT]);
                 atomicAdd(&profile[P_SKIP], pf[P_SKIP]);
                 atomicAdd(&profile[P_WARP_STEPS], turns); atomicAdd(&profile[P_STEPS], stepping);
                 atomicAdd(&profile[P_STARVED], waited);
             })
    }
    #undef WLAP
    #undef ASLEEP
    #undef VIAS
    PROF(
        const u64 t_end = now_ns();
        atomicAdd(&profile[P_BUSY_NS], t_end - t_start);
        atomicMax(&profile[P_END_NS], t_end);
        atomicMax(&profile[P_START_NS_INV], ~t_start);
    )
}

// Writes the prefixes of a launch's won attempts into their lines, after attempts_split: its won flags say which attempt
// won each deal. Every thread of a warp plays the same move of a different deal's prefix, which is why this costs a few
// percent of a launch where playing each prefix at its win cost a third, and the CPU, which verifies every line, has
// less to do than if it played them.
#if WL_MINB
extern "C" __global__ void __launch_bounds__(BLOCK, WL_MINB) write_lines(
#else
extern "C" __global__ void write_lines(
#endif
    const u8* deals, const u64* deal_masks, u32 n_deals, u32 tries, u32 prefix,
                                       u32 tries2, u32 prefix2, u32 leap_shift, u64 seed, const u32* won, u8* lines,
                                       u32* next_deal) {
    const u32 lane = threadIdx.x % 32;
    const int share = SHARING && (int)min(prefix, tries2 ? prefix2 : prefix) >= SHARE_K ? SHARE_K : 0;
    const int all_moves = (int)max(prefix, prefix2);
    for (;;) {
        u32 base = 0;
        if (lane == 0) base = atomicAdd(next_deal, 32u);
        base = __shfl_sync(0xFFFFFFFFu, base, 0);
        if (base >= n_deals) break;
        u32 deal = base + lane, a = 0, rng = 1, rc = RANKS_FULL, targets = 0;
        u64 t1 = 0, t2 = 0, t3 = 0;
        bool alive = deal < n_deals;
        if (alive) {
            a = __ldg(won + deal) - 1;
            alive = a != 0xFFFFFFFFu;
        }
        if (alive) targets = ((volatile u32*)learn_word(lines, deal))[1];
        u32 R[RW];
        u64 l1 = 0, l3 = 0;
        int len = 0, moves = 0;
        if (alive) {
            len = rrow_load(R, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, l1, l3);
            moves = a < tries * n_deals ? (int)prefix : (int)prefix2;
            rng = share ? group_rng(seed, a - (a / n_deals) % SHARE_G * n_deals) : own_rng(seed, a);
            rc -= LEARN_CLASSES * learn_spread(targets);
            t1 = rrow_places(R, targets);
            #if LEARN_CLASSES >= 2
            t2 = t1;
            #endif
        }
        u8* out = lines + (u64)deal * 51;
        #if SHR_WIN && SLICED
        // The shared moves as attempts_split plays them: on the planes of the first 32 piles.
        u32 B[6], o1 = 0, o3 = 0, w0 = 0, w1 = 0;
        if (share) {
            u32 T[6], c1 = (u32)l1, c3 = (u32)l3, u0 = (u32)(t1 >> 32), u1 = 0;
            win_load(R, B, T);
            o1 = c1; o3 = c3; w0 = (u32)t1;
            for (int m = 0; m < share; m++) {
                if (alive && m < moves) {
                    int played;
                    while ((played = win_try(B, T, len, o1, o3, c1, c3, w0, w1, u0, u1, rng, leap_shift)) == -2) {}
                    if (played < 0) alive = false; else out[m] = (u8)played;
                }
            }
        } else {
            for (int m = 0; m < all_moves; m++) {
                if (alive && m < moves) {
                    int played = rrow_move<RW, 4, u64>(R, len, l1, l3, rc, rng, leap_shift, t1, t2, t3);
                    if (played < 0) alive = false; else out[m] = (u8)played;
                }
            }
        }
        if (alive && share) rng = own_rng(seed, a);
        #else
        for (int m = 0; m < (share ? share : all_moves); m++) {
            if (alive && m < moves) {
                int played = rrow_move<RW, 4, u64>(R, len, l1, l3, rc, rng, leap_shift, t1, t2, t3);
                if (played < 0) alive = false; else out[m] = (u8)played;
            }
        }
        if (alive && share) rng = own_rng(seed, a);
        #endif
        #if REC && SHR_WIN && SLICED
        if (share && (int)max(min(prefix, (u32)MAX_PREFIX), tries2 ? min(prefix2, (u32)MAX_PREFIX) : min(prefix, (u32)MAX_PREFIX)) - share <= 20
            && deal >= rec_low(n_deals)) alive = false;   // its winner wrote its own moves
        #endif
        #if SLICED
        #if !SHR_WIN
        u32 B[6], o1 = (u32)l1, o3 = (u32)l3, w1 = (u32)t2, w0 = ((u32)t1 & ~(u32)t2) | (u32)t3;
        slice_row<SLICE_W>(R, B);
        #endif
        for (int m = share; share && m < all_moves; m++) {
            if (alive && m < moves) {
                int played = pat_move(B, len, o1, o3, w0, w1, rng, m >= 30);
                if (played < 0) alive = false; else out[m] = (u8)played;
            }
        }
        #else
        for (int m = share; share && m < all_moves; m++) {
            if (alive && m < moves) {
                int played = rrow_move<RW, 4, u64>(R, len, l1, l3, rc, rng, leap_shift, t1, t2, t3);
                if (played < 0) alive = false; else out[m] = (u8)played;
            }
        }
        #endif
    }
}

// Random playouts for the tests (crates/gpu/tests), one per thread, recording what the code above computes at every
// step for the CPU to check against its engines. While the position has more than RES piles it plays on the row (the
// prefix's code), and the same moves on the register row (attempts_split's prefix), in every form that fits the
// position; from RES piles down, in registers (the search's code), where every move is also undone and redone.
// Per step, the position's slide and leap masks (two u64), the move played from it (pile << 1 | leap, or 0xFF when
// there is none, which ends the playout), and flags: bit 0 connected(present), bit 1 the last move broke a link
// (breaks_link), bit 2 undoing the move did not restore the position, bit 3 connected_without_leftmost, bit 4 the
// register row disagreed with the row (on the masks after the move, the cards, the residual or its cards present).
extern "C" __global__ void __launch_bounds__(BLOCK) playouts(
    const u8* __restrict__ positions, const u64* __restrict__ position_masks, u32 n, u64 seed,
    u64* masks_out,                    // 52 steps × 2 per playout
    u8* moves_out, u8* flags_out)      // 52 steps per playout
{
    __shared__ u8 s_row[64][BLOCK];
    const int t = threadIdx.x;
    const u32 p = blockIdx.x * BLOCK + t;
    if (p >= n) return;
    u32 rng = seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (p + 1)));
    u64 present, l1, l3, lo = 0, hi = 0;
    int len = row_load(s_row, t, positions + (u64)p * 52, position_masks + 3 * (u64)p, present, l1, l3);
    u32 R[RW];
    u64 r1, r3;
    rrow_load(R, positions + (u64)p * 52, position_masks + 3 * (u64)p, r1, r3);
    bool regs = false, link = false, differs = false;
    for (int step = 0; step < 52; step++) {
        const u64 at = (u64)p * 52 + step;
        if (!regs && len <= RES) {
            load_residual(s_row, t, len, lo, hi);
            differs |= lo != (R[0] | (u64)R[1] << 32) || hi != (R[2] | (u64)R[3] << 32) || rrow_present(R) != present;
            regs = true;
        }
        u64 ms = l1, ml = l3;
        if (regs) {
            u32 s, l;
            masks(lo, hi, len, s, l);
            ms = s; ml = l;
        }
        masks_out[2 * at] = ms;
        masks_out[2 * at + 1] = ml;
        u32 left = regs ? card_at(lo, hi, 0) : s_row[OFF][t];
        u8 flags = (connected(present) ? 1 : 0) | (link ? 2 : 0) | (connected_without_leftmost(present, left, len) ? 8 : 0)
                   | (differs ? 16 : 0);
        u8 mv = 0xFF;
        u32 n1 = __popcll(ms), n3 = __popcll(ml);
        if (n1 + n3 > 0) {
            u32 k = below(rng, n1 + n3);
            bool leap = k >= n1;
            int pile = leap ? select_bit64(ml, k - n1) : select_bit64(ms, k);
            mv = (u8)(pile << 1 | (leap ? 1 : 0));
            u32 covered;
            if (regs) {
                int dd = leap ? 3 : 1;
                u64 lo0 = lo, hi0 = hi;
                u32 mover = card_at(lo, hi, pile);
                covered = card_at(lo, hi, pile - dd);
                set_card(lo, hi, pile - dd, mover);
                delete_card(lo, hi, pile);
                u64 lo1 = lo, hi1 = hi;
                insert_card(lo, hi, pile, mover);
                set_card(lo, hi, pile - dd, covered);
                if (lo != lo0 || hi != hi0) flags |= 4;
                lo = lo1; hi = hi1;
                // The shortest register row: the masks after the move, from before it, and its cards.
                u32 n1s, n3s, mover_s, ss, ls;
                rrow_after<4, 3, u32>(R, len, (u32)r1, (u32)r3, pile, leap, n1s, n3s, mover_s);
                rrow_play<4>(R, pile, pile - dd, mover_s);
                r1 = n1s; r3 = n3s;
                len--;
                masks(lo, hi, len, ss, ls);
                differs |= n1s != ss || n3s != ls || (R[0] | (u64)R[1] << 32) != lo || (R[2] | (u64)R[3] << 32) != hi;
            } else {
                // Every length of the register row that fits this position agrees with the longest, which agrees with
                // the row.
                u64 n1r, n3r;
                u32 mover_r;
                rrow_after<RW, 4, u64>(R, len, r1, r3, pile, leap, n1r, n3r, mover_r);
                u32 R0[RW];
                for (int j = 0; j < RW; j++) R0[j] = R[j];
                rrow_play<RW>(R0, pile, pile - (leap ? 3 : 1), mover_r);
                #define CHECK_SHORT(W, BITS, M, CARDS)                                                              \
                    if (len <= (CARDS)) {                                                                           \
                        u32 S[RW], mover_s;                                                                         \
                        M n1s, n3s;                                                                                 \
                        rrow_after<W, BITS, M>(R, len, (M)r1, (M)r3, pile, leap, n1s, n3s, mover_s);                \
                        differs |= n1s != (M)n1r || n3s != (M)n3r || mover_s != mover_r;                            \
                        for (int j = 0; j < RW; j++) S[j] = R[j];                                                   \
                        rrow_play<W>(S, pile, pile - (leap ? 3 : 1), mover_r);                                      \
                        for (int j = 0; j < W; j++) differs |= S[j] != R0[j];                                       \
                    }
                CHECK_SHORT(12, 4, u64, 48)
                CHECK_SHORT(11, 4, u64, 44)
                CHECK_SHORT(10, 4, u64, 40)
                CHECK_SHORT(9, 4, u64, 36)
                CHECK_SHORT(8, 4, u32, 32)
                CHECK_SHORT(8, 3, u32, 30)
                CHECK_SHORT(7, 3, u32, 28)
                CHECK_SHORT(6, 3, u32, 24)
                CHECK_SHORT(5, 3, u32, 20)
                CHECK_SHORT(4, 3, u32, 16)
                #undef CHECK_SHORT
                for (int j = 0; j < RW; j++) R[j] = R0[j];
                r1 = n1r; r3 = n3r;
                covered = row_move(s_row, t, len, l1, l3, pile, leap);
                differs |= r1 != l1 || r3 != l3;
                #pragma unroll
                for (int i = 0; i < 52; i++) differs |= ((R[i >> 2] >> (8 * (i & 3))) & 0xFF) != s_row[OFF + i][t];
            }
            present &= ~(1ull << covered);
            link = breaks_link(present, covered);
        }
        flags_out[at] = flags;
        moves_out[at] = mv;
        if (mv == 0xFF) break;
    }
}
