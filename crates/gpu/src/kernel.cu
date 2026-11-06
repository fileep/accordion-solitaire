// Ladder attempts on the GPU (PLAN.md Phase 5 track F, Phase 8): one attempt per thread. An attempt plays `prefix`
// random moves on its position, then a first-win depth-first search of what is left (at most 16 piles) with sleep
// sets and the connectivity dead-end test, up to `budget` positions. The rules are the Rust fast engine's
// (crates/core/src/fast.rs); every win is replayed by the CPU's reference rules before it counts, and the
// `playouts` kernel at the end lets the tests (crates/gpu/tests) check this rules code against the Rust engines.
//
// Everything stays on chip. The prefix plays on a 64-byte row in shared memory (stored byte-major, so a warp's 32
// threads touch 32 different banks; in attempts_split, on 13 words in registers), starting from legal-move masks the
// host works out once per position. The
// search keeps its at most 16 cards in two 64-bit registers, recomputes the legal-move masks four cards at a time in
// 32-bit words (match4), and undoes moves instead of copying positions. Its moves and covered cards are byte stacks in
// registers, and so are the current depth's asleep moves; shared memory holds only the asleep moves of the depths
// above it. Choosing the k-th legal move halves the mask by popcount (no loop that runs k times), and the dead-end
// test's trigger looks at all four suits at once in 16-bit lanes.
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
// There are three attempt kernels with the same arguments and results. `attempts` runs each thread's attempts start to
// finish; its threads soon drift apart, and a warp then runs them nearly one at a time. `attempts_stepwise` moves
// every thread one step (a prefix move or a search position) each turn of its loop, so the threads doing the same
// thing step together: 1.7 times the deals per second (docs/BENCHMARKS.md, "After Phase 8 — where the GPU's time
// goes" and "the GPU's bit operations"). `attempts_split`, the default, gives the prefixes and the searches warps of
// their own, joined by a queue of residuals in shared memory, keeps the prefix's row in registers, lets groups of a
// deal's tries share their first prefix moves, runs the prefix warps in lockstep, keeps its hot code small, and doesn't
// choose its prefix moves blindly: many times the deals per second of attempts_stepwise ("split warps", "the prefix row
// in registers", "shared early prefix moves", "prefix warps in lockstep", "the search step, and what code size costs"
// and "what winning prefixes have in common").
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
#define RES 16            // residual piles a search can hold (two 64-bit registers)
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

// A 128-bit stack of bytes in two registers: push puts a byte at the bottom of lo and lifts the rest, pop takes it
// back; every shift is a constant one. The search keeps its moves and covered cards this way, not in shared memory.
__device__ __forceinline__ void push_byte(u64& lo, u64& hi, u32 b) {
    hi = (hi << 8) | (lo >> 56);
    lo = (lo << 8) | b;
}
__device__ __forceinline__ u32 pop_byte(u64& lo, u64& hi) {
    u32 b = (u32)(lo & 0xFF);
    lo = (lo >> 8) | (hi << 56);
    hi >>= 8;
    return b;
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
    return k < c ? select_bits<32>(lo, k) : 32 + select_bits<32>((u32)(m >> 32), k - c);
}
__device__ __forceinline__ int select_bit16(u32 m, u32 k) { return select_bits<16>(m, k); }

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

// The suits whose rows hold a rank in `ranks` (13 bits): bit 15 of each suit's 16-bit lane. A lane holds at most
// 0x1FFF, so adding 0x7FFF sets its bit 15 exactly when it isn't empty, and never carries into the next, so the two
// 32-bit words of `present` go on their own.
__device__ __forceinline__ void suits_with32(u32 p0, u32 p1, u32 ranks, u32& a0, u32& a1) {
    u32 r = ranks | ranks << 16;
    a0 = ((p0 & r) + 0x7FFF7FFFu) & 0x80008000u;
    a1 = ((p1 & r) + 0x7FFF7FFFu) & 0x80008000u;
}
// The ranks the suits flagged in a0 and a1 (bit 15 of their lanes) hold between them.
__device__ __forceinline__ u32 ranks_of32(u32 p0, u32 p1, u32 a0, u32 a1) {
    u32 r = (p0 & (a0 - (a0 >> 15))) | (p1 & (a1 - (a1 >> 15)));
    return (r | r >> 16) & 0x1FFFu;
}
// fast.rs connected: the suits present are linked, two suits when they share a rank. Branch-free, so a warp never
// waits for its slowest thread (the search loop it replaced, one suit a turn until nothing changed, cost about 18%
// of the kernel's time): from the lowest suit present, three steps reach every suit linked to it.
__device__ __forceinline__ bool connected(u64 present) {
    u32 p0 = (u32)present, p1 = (u32)(present >> 32);
    u32 all0 = (p0 + 0x7FFF7FFFu) & 0x80008000u, all1 = (p1 + 0x7FFF7FFFu) & 0x80008000u;
    u32 low = all0 ? p0 >> ((all0 & 0x8000u) ? 0 : 16) : p1 >> ((all1 & 0x8000u) ? 0 : 16);     // the lowest suit's
    u32 a0, a1;
    suits_with32(p0, p1, low & 0x1FFFu, a0, a1);
    suits_with32(p0, p1, ranks_of32(p0, p1, a0, a1), a0, a1);
    suits_with32(p0, p1, ranks_of32(p0, p1, a0, a1), a0, a1);
    return ((a0 ^ all0) | (a1 ^ all1)) == 0;
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

// Takes back row_move (the position after it has `len` piles): the piles from `pile` on move up one place, and the
// mover and the covered card go back.
__device__ __forceinline__ void row_unmove(u8 (*row)[BLOCK], int t, int len, int pile, bool leap, u32 mover,
                                           u32 covered) {
    for (int i = OFF + len; i > OFF + pile; i--) row[i][t] = row[i - 1][t];
    row[OFF + pile][t] = mover;
    row[OFF + pile - (leap ? 3 : 1)][t] = covered;
}

// One move of the random prefix, played; returns pile << 1 | leap, or -1 when every legal move leaves a position
// with no move and more than one pile (each loses; so does this position). Not quite uniform, by measurement
// (docs/BENCHMARKS.md, "After Phase 8 — less random prefixes"):
// - a leap is 2^leap_shift times as likely as a slide (8: about a fifth more deals a second on the T550, and a third
//   of the misses; leaps only, or longer prefixes, did worse);
// - a move that leaves no move is taken back and another chosen, which can only spare attempts that would lose.
// The same random numbers choose the same moves, so a winning attempt replays its prefix to write its line.
__device__ int prefix_move(u8 (*row)[BLOCK], int t, int& len, u64& l1, u64& l3, u64& present, u32& rng,
                           u32 leap_shift) {
    u64 c1 = l1, c3 = l3;
    for (;;) {
        u32 n1 = __popcll(c1), n3 = __popcll(c3);
        if (n1 + n3 == 0) return -1;
        u32 k = below(rng, n1 + (n3 << leap_shift));
        bool leap = k >= n1;
        int pile = leap ? select_bit64(c3, (k - n1) >> leap_shift) : select_bit64(c1, k);
        int d = leap ? 3 : 1;
        u32 mover = row[OFF + pile][t], covered = row[OFF + pile - d][t];
        u64 s1 = l1, s3 = l3;
        row_move(row, t, len, l1, l3, pile, leap);
        if ((l1 | l3) != 0 || len == 1) {
            present &= ~(1ull << covered);
            return pile << 1 | (leap ? 1 : 0);
        }
        row_unmove(row, t, len, pile, leap, mover, covered);   // no move left: take it back
        len++;
        l1 = s1; l3 = s3;
        u64 bit = 1ull << pile;
        if (leap) c3 &= ~bit; else c1 &= ~bit;
    }
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
                                           u32& mover) {
    u32 w0, w1, w2;
    rrow_window<W, BITS>(R, x, w0, w1, w2);
    mover = (w1 >> 16) & 0xFF;
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

// How a prefix chooses (Settings::draws, shared_draws): DRAWS random draws from the legal moves for a try's own moves,
// SHARED_DRAWS for the moves its group shares, and the one played covers the card whose rank has the fewest others
// left; winning lines do that a little more often than chance at every move, and it mounts up over a prefix.
#ifndef DRAWS
#define DRAWS 5
#endif
#ifndef SHARED_DRAWS
#define SHARED_DRAWS 3
#endif
// The others left of each rank, 2 bits each: a whole deal has 3 of every rank. For a
// position that isn't one, the counts are only a worse guide to which move to like.
#define RANKS_FULL 0x3FFFFFFu

// Word q of the row's first W words: registers can't be indexed, so a tree of selects.
template <int W>
__device__ __forceinline__ u32 rrow_word(const u32 (&R)[RW], u32 q) {
    u32 a[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) a[i] = i < W ? R[i < W ? i : 0] : SENT4;
    if (W > 8) {
        #pragma unroll
        for (int i = 0; i < 8; i++) a[i] = (q & 8) ? a[i + 8] : a[i];
    }
    #pragma unroll
    for (int i = 0; i < 4; i++) a[i] = (q & 4) ? a[i + 4] : a[i];
    a[0] = (q & 2) ? a[2] : a[0];
    a[1] = (q & 2) ? a[3] : a[1];
    return (q & 1) ? a[1] : a[0];
}
template <int W>
__device__ __forceinline__ u32 rrow_card(const u32 (&R)[RW], int y) {
    return (rrow_word<W>(R, (u32)y >> 2) >> (8 * (y & 3))) & 0x3F;
}
// How little a draw is liked: the others left of the rank it covers, then whether its mover is one of the last three
// piles. Lower is better.
__device__ __forceinline__ u32 draw_score(u32 card, u32 rc, int x, int len) {
    return 2 * ((rc >> (2 * (card & 15))) & 3) + (u32)(x + 3 >= len);
}

// prefix_move on the register row, choosing as above. A move that would leave no move is found from the masks before
// it is played, so it is never played and taken back.
template <int W, int BITS, typename M, int D>
__device__ __forceinline__ int rrow_move(u32 (&R)[RW], int& len, M& l1, M& l3, u32& rc, u32& rng, u32 leap_shift) {
    M c1 = l1, c3 = l3;
    for (;;) {
        u32 n1 = count_bits(c1), n3 = count_bits(c3);
        if (n1 + n3 == 0) return -1;
        u32 k = below(rng, n1 + (n3 << leap_shift));
        bool leap = k >= n1;
        int x = select_bit(leap ? c3 : c1, leap ? (k - n1) >> leap_shift : k);
        u32 card = rrow_card<W>(R, x - (leap ? 3 : 1)), score = draw_score(card, rc, x, len);
        #pragma unroll
        for (int d = 1; d < D; d++) {
            u32 k2 = below(rng, n1 + (n3 << leap_shift));
            bool leap2 = k2 >= n1;
            int x2 = select_bit(leap2 ? c3 : c1, leap2 ? (k2 - n1) >> leap_shift : k2);
            u32 card2 = rrow_card<W>(R, x2 - (leap2 ? 3 : 1)), score2 = draw_score(card2, rc, x2, len);
            bool better = score2 < score;
            x = better ? x2 : x; leap = better ? leap2 : leap;
            card = better ? card2 : card; score = better ? score2 : score;
        }
        M nl1, nl3;
        u32 mover;
        rrow_after<W, BITS, M>(R, len, l1, l3, x, leap, nl1, nl3, mover);
        if ((nl1 | nl3) == 0 && len > 2) {           // no move left after it: choose another
            M bit = (M)1 << x;
            if (leap) c3 &= ~bit; else c1 &= ~bit;
            continue;
        }
        rrow_play<W>(R, x, x - (leap ? 3 : 1), mover);
        len--;
        l1 = nl1; l3 = nl3;
        // A rank's field stays at 0 for its last card: nothing reads it again.
        u32 others = (rc >> (2 * (card & 15))) & 3;
        rc -= (u32)(others != 0) << (2 * (card & 15));
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
// The same on four 32-bit words (attempts_split's search), card i byte i % 4 of C[i / 4]: every word's shift is a funnel
// shift, one instruction, where a 64-bit shift by a varying amount takes several.
__device__ __forceinline__ u32 card4(const u32 (&C)[4], int i) {
    u32 w = (i & 8) ? ((i & 4) ? C[3] : C[2]) : ((i & 4) ? C[1] : C[0]);
    return (w >> (8 * (i & 3))) & 0xFF;
}
// Pile x onto pile y: the mover onto y's byte, and every card above x down one place.
__device__ __forceinline__ void play4(u32 (&C)[4], int x, int y, u32 mover) {
    int yw = y >> 2;
    u32 ysel = (0x3210u & ~(0xFu << (4 * (y & 3)))) | (4u << (4 * (y & 3)));
    #pragma unroll
    for (int j = 0; j < 4; j++) {
        u32 next = j < 3 ? C[j < 3 ? j + 1 : 0] : SENT4;
        u32 keep = __funnelshift_lc(0xFFFFFFFFu, 0u, (u32)max(8 * x - 32 * j, 0));   // the bytes below x
        u32 v = (C[j] & keep) | (__funnelshift_r(C[j], next, 8) & ~keep);
        C[j] = j == yw ? __byte_perm(v, mover, ysel) : v;
    }
}
// Takes it back: the cards from x up move up one place, the mover goes back to x and the covered card to y.
__device__ __forceinline__ void unplay4(u32 (&C)[4], int x, int y, u32 mover, u32 covered) {
    int xw = x >> 2, yw = y >> 2;
    u32 xmask = 0xFFu << (8 * (x & 3)), xbyte = mover << (8 * (x & 3));
    u32 ysel = (0x3210u & ~(0xFu << (4 * (y & 3)))) | (4u << (4 * (y & 3)));
    #pragma unroll
    for (int j = 3; j >= 0; j--) {
        u32 prev = j > 0 ? C[j > 0 ? j - 1 : 0] : 0u;
        u32 keep = __funnelshift_lc(0xFFFFFFFFu, 0u, (u32)max(8 * x - 32 * j, 0));
        u32 v = (C[j] & keep) | (__funnelshift_l(prev, C[j], 8) & ~keep);
        v = j == xw ? (v & ~xmask) | xbyte : v;
        C[j] = j == yw ? __byte_perm(v, covered, ysel) : v;
    }
}
// Legal-move masks of the residual: bit i of s, pile i may slide; of l, leap. Four piles at a time in 32-bit words
// (match16); pile 0 can't slide and piles 0 to 2 can't leap, since sentinels lie before them.
__device__ __forceinline__ void masks(u64 lo, u64 hi, int len, u32& s, u32& l) {
    u32 w0 = (u32)lo, w1 = (u32)(lo >> 32), w2 = (u32)hi, w3 = (u32)(hi >> 32);
    u32 valid = (1u << len) - 1;
    s = match16(0x7F7F7F7Fu, w0, w1, w2, w3, 8) & valid;
    l = match16(0x7F7F7F7Fu, w0, w1, w2, w3, 24) & valid;
}
__device__ __forceinline__ void masks4(const u32 (&C)[4], int len, u32& s, u32& l) {
    u32 valid = (1u << len) - 1;
    s = match16(SENT4, C[0], C[1], C[2], C[3], 8) & valid;
    l = match16(SENT4, C[0], C[1], C[2], C[3], 24) & valid;
}
// The row's piles (at most RES of them) into registers.
__device__ __forceinline__ void load_residual(u8 (*row)[BLOCK], int t, int len, u64& lo, u64& hi) {
    lo = 0x7F7F7F7F7F7F7F7Full; hi = 0x7F7F7F7F7F7F7F7Full;
    for (int i = 0; i < len; i++) set_card(lo, hi, i, row[OFF + i][t]);
}

// dfs.rs asleep_after: the moves asleep in the child after `pile` moves.
__device__ __forceinline__ void asleep_after(u32 ds, u32 dl, int pile, bool leap, u32& s, u32& l) {
    int target = pile - (leap ? 3 : 1);
    u32 left = (1u << target) - 1;
    s = (ds & left) | ((ds >> 1) & ~((1u << (pile + 1)) - 1));
    l = (dl & left) | ((dl >> 1) & ~((1u << (pile + 3)) - 1));
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
    P_H0, P_H16, P_H64, P_H256, P_H1024, P_HMAX,             // searches by positions: 0-15, 16-63, 64-255, 256-1023,
                                                             // 1024 up to the budget, the budget
    P_STUCK, P_LARGE, P_CUT,                                 // prefixes ending with no move left, with more than
                                                             // RES piles, or with the residual disconnected
    P_STUCK_MOVES,                                           // prefix moves played by the attempts that got stuck
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

// Shared memory in `attempts` is byte-major: byte i of thread t is s_mem[i * BLOCK + t], whatever it holds, so a
// thread's row and its search state, which overlay each other, never touch another thread's bytes (threads in a block
// are in different phases at any moment, the prefix or the search). A warp's 32 threads reading byte i read 32
// consecutive bytes, so no bank conflicts.
#define SH(i) s_mem[(i) * BLOCK + t]
#define SH16(i) ((u32)SH(i) | (u32)SH((i) + 1) << 8)
#define SET16(i, v) do { u32 v_ = (v); SH(i) = (u8)v_; SH((i) + 1) = (u8)(v_ >> 8); } while (0)

// Results per position: won flag (0/1), the winning line (51 bytes: pile << 1 | leap), positions generated and
// attempts run.
extern "C" __global__ void __launch_bounds__(BLOCK) attempts(
    const u8* __restrict__ deals,      // 52 bytes per position
    const u64* __restrict__ deal_masks, // 3 per position: slides, leaps, cards present
    u32 n_deals, u64 inv_deals,        // positions, and ceil(2^64 / n_deals)
    u32 tries, u32 prefix, u32 tries2, u32 prefix2, u32 budget, u32 conn_gate,
    u32 leftmost,                      // 1: the connectivity test leaves pile 0's card out (stronger)
    u32 leap_shift,                    // a prefix leap is 2^leap_shift times as likely as a slide (prefix_move)
    u64 seed,
    u32* next_attempt,                 // global work counter, starts at 0
    u32* won,                          // per position: 0, or 1 once some attempt has won it
    u8* lines,                         // 51 bytes per position
    u64* deal_counts,                  // per position: positions generated << 16 | attempts run
    u64* profile)                      // P_SLOTS counters, used only when compiled with PROFILE
{
    // Per depth d, at these byte offsets: asleep slides (DS + 2d, u16) and asleep leaps (DL + 2d, u16); the moves and
    // covered cards are register stacks. The prefix's 64-byte row overlays the asleep moves exactly.
    enum { DS = 0, DL = 2 * RES, BYTES = 4 * RES };
    __shared__ u8 s_mem[BYTES * BLOCK];
    u8 (*s_row)[BLOCK] = (u8 (*)[BLOCK])s_mem;
    static_assert(64 <= BYTES, "the row fits over the asleep moves");
    const int t = threadIdx.x;
    const u64 total = n_deals * (u64)(tries + tries2);
    if (prefix > MAX_PREFIX) prefix = MAX_PREFIX;
    if (prefix2 > MAX_PREFIX) prefix2 = MAX_PREFIX;
    PROF(u64 pf[P_BUSY_NS] = {0}; const u64 t_start = now_ns(); u64 c0 = clock64(), c1;)
    #define LAP(slot) PROF(c1 = clock64(); pf[slot] += c1 - c0; c0 = c1;)
    for (;;) {
        u32 a = atomicAdd(next_attempt, 1u);
        if (a >= total) break;
        u32 deal, attempt_try;
        deal_and_try(a, n_deals, inv_deals, deal, attempt_try);
        const u32 k_moves = attempt_try < tries ? prefix : prefix2;
        PROF(pf[P_FETCHES]++;)
        if (*((volatile u32*)&won[deal])) { PROF(pf[P_SKIPS]++;) LAP(P_SKIP); continue; }
        u32 rng = seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (a + 1)));
        const u32 prefix_rng = rng;
        LAP(P_FETCH);

        // The position and its random prefix.
        u64 present, l1, l3;
        int len = row_load(s_row, t, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, present, l1, l3);
        LAP(P_LOAD);
        int moves = 0;
        while (moves < (int)k_moves) {
            PROF(if ((t & 31) == __ffs(__activemask()) - 1) pf[P_PREFIX_WARP_STEPS]++; pf[P_PREFIX_STEPS]++;)
            if ((l1 | l3) == 0 || prefix_move(s_row, t, len, l1, l3, present, rng, leap_shift) < 0) break;
            moves++;
        }
        LAP(P_PREFIX);
        // Stuck before the residual got small, or already disconnected: lost anyway.
        bool linked = leftmost ? connected_without_leftmost(present, s_row[OFF][t], len) : connected(present);
        bool search = len <= RES && !(len > 1 && len <= (int)conn_gate && !linked);

        bool win = search && len == 1;
        LAP(P_GATE);
        int depth = 0;
        u32 used = 0;
        u64 via_lo = 0, via_hi = 0, cov_lo = 0, cov_hi = 0;   // the search's moves and covered cards, last on top
        if (search && !win) {
            u64 lo, hi;
            load_residual(s_row, t, len, lo, hi);
            u32 ms, ml;
            masks(lo, hi, len, ms, ml);
            u32 us = ms, ul = ml;                 // untried slides and leaps at the current depth
            SET16(DS, 0); SET16(DL, 0);
            // Depth-first search with undo: level d is the position after d search moves.
            while (!win) {
                PROF(if ((t & 31) == __ffs(__activemask()) - 1) pf[P_WARP_STEPS]++; pf[P_STEPS]++;)
                if ((us | ul) == 0) {
                    if (depth == 0) break;        // the residual is lost
                    depth--;
                    u32 via = pop_byte(via_lo, via_hi), cov = pop_byte(cov_lo, cov_hi);
                    int pile = via >> 1, dd = (via & 1) ? 3 : 1;
                    u32 mover = card_at(lo, hi, pile - dd);
                    insert_card(lo, hi, pile, mover);
                    set_card(lo, hi, pile - dd, cov);
                    present |= 1ull << cov;
                    len++;
                    masks(lo, hi, len, ms, ml);
                    us = ms & ~SH16(DS + 2 * depth);
                    ul = ml & ~SH16(DL + 2 * depth);
                    continue;
                }
                if (used >= budget) break;
                u32 n1 = __popc(us), n3 = __popc(ul);
                u32 k = below(rng, n1 + n3);
                bool leap = k >= n1;
                int pile = leap ? select_bit16(ul, k - n1) : select_bit16(us, k);
                u32 ds = SH16(DS + 2 * depth), dl = SH16(DL + 2 * depth);
                u32 sleep_s, sleep_l;
                asleep_after(ds, dl, pile, leap, sleep_s, sleep_l);
                u32 bit = 1u << pile;
                if (leap) { ul &= ~bit; dl |= bit; } else { us &= ~bit; ds |= bit; }
                SET16(DS + 2 * depth, ds); SET16(DL + 2 * depth, dl);
                // Make the move.
                int dd = leap ? 3 : 1;
                u32 mover = card_at(lo, hi, pile), cov = card_at(lo, hi, pile - dd);
                set_card(lo, hi, pile - dd, mover);
                delete_card(lo, hi, pile);
                int parent_len = len;
                len--;
                present &= ~(1ull << cov);
                used++;
                if (len == 1) {
                    push_byte(via_lo, via_hi, pile << 1 | (leap ? 1 : 0));
                    win = true;
                    depth++;
                    break;
                }
                bool dead;
                if (leftmost) {
                    // Also when pile 0's card changed, or the move broke a link among the other cards.
                    u32 left = card_at(lo, hi, 0);
                    dead = len <= (int)conn_gate
                           && (breaks_link(present, cov) || parent_len > (int)conn_gate || pile == dd
                               || breaks_link(present & ~(1ull << left), cov))
                           && !connected_without_leftmost(present, left, len);
                } else {
                    dead = len <= (int)conn_gate && (breaks_link(present, cov) || parent_len > (int)conn_gate)
                           && !connected(present);
                }
                if (dead) {                       // undo at once
                    insert_card(lo, hi, pile, mover);
                    set_card(lo, hi, pile - dd, cov);
                    present |= 1ull << cov;
                    len++;
                    continue;
                }
                push_byte(via_lo, via_hi, pile << 1 | (leap ? 1 : 0));
                push_byte(cov_lo, cov_hi, cov);
                depth++;
                masks(lo, hi, len, ms, ml);
                us = ms & ~sleep_s; ul = ml & ~sleep_l;
                SET16(DS + 2 * depth, sleep_s); SET16(DL + 2 * depth, sleep_l);
            }
        }
        LAP(P_SEARCH);
        PROF(if (search) pf[used >= budget ? P_HMAX : used >= 1024 ? P_H1024 : used >= 256 ? P_H256
                             : used >= 64 ? P_H64 : used >= 16 ? P_H16 : P_H0]++;)
        atomicAdd(&deal_counts[deal], (u64)(moves + used) << 16 | 1u);
        if (win && atomicCAS(&won[deal], 0u, 1u) == 0u) {
            u8* out = lines + (u64)deal * 51;
            // The search's moves, first to last: the first is deepest in the stack.
            for (int d = 0; d < depth; d++) out[moves + d] = (u8)card_at(via_lo, via_hi, depth - 1 - d);
            // Replay the prefix: the same random numbers on the same position choose the same moves.
            u32 r = prefix_rng;
            int n = row_load(s_row, t, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, present, l1, l3);
            for (int i = 0; i < moves; i++) out[i] = (u8)prefix_move(s_row, t, n, l1, l3, present, r, leap_shift);
        }
        LAP(P_RESULT);
    }
    #undef LAP
    PROF(
        for (int i = 0; i < P_BUSY_NS; i++) atomicAdd(&profile[i], pf[i]);
        const u64 t_end = now_ns();
        atomicAdd(&profile[P_BUSY_NS], t_end - t_start);
        atomicMax(&profile[P_END_NS], t_end);
        atomicMax(&profile[P_START_NS_INV], ~t_start);
    )
}

// The same attempts a step at a time (Settings::stepwise). In `attempts` each thread runs its own attempt start to
// finish, and since its threads are soon at different points, a warp ends up running them nearly one at a time
// (`accgpu --profile`: 1.5 of 32 threads step together in the search). Here every turn of the loop moves each thread
// one step through what it is doing, a prefix move or a search position, so the threads doing the same thing step
// together, and a thread that finishes an attempt takes the next at once. The warp takes attempts 32 at a time, with
// one atomic and one load of their deals' won flags; attempts it can't use yet wait, one per thread, for the next
// threads that need one.
// At least 14 blocks a multiprocessor: at most 72 registers, which the kernel fits without spilling (left to itself
// it took 79, 12 blocks fitted, and it ran 5% slower; 64 registers and 16 blocks spilled and lost).
extern "C" __global__ void __launch_bounds__(BLOCK, 14) attempts_stepwise(
    const u8* __restrict__ deals, const u64* __restrict__ deal_masks, u32 n_deals, u64 inv_deals, u32 tries,
    u32 prefix, u32 tries2, u32 prefix2, u32 budget, u32 conn_gate, u32 leftmost, u32 leap_shift,
    u64 seed, u32* next_attempt, u32* won, u8* lines, u64* deal_counts, u64* profile)
{
    enum { DS = 0, DL = 2 * RES, BYTES = 4 * RES };
    enum { FETCH, PREFIX, SEARCH, FINISH, DONE };
    __shared__ u8 s_mem[BYTES * BLOCK];
    u8 (*s_row)[BLOCK] = (u8 (*)[BLOCK])s_mem;
    const int t = threadIdx.x;
    const u32 lane = t & 31, FULL = 0xFFFFFFFFu;
    const u64 total = n_deals * (u64)(tries + tries2);
    if (prefix > MAX_PREFIX) prefix = MAX_PREFIX;
    if (prefix2 > MAX_PREFIX) prefix2 = MAX_PREFIX;
    PROF(u64 pf[P_BUSY_NS] = {0}; const u64 t_start = now_ns(); bool searched = false; u64 c0 = clock64(), c1;)
    // The warp's time in each part of the loop, measured where its threads meet again, by thread 0 of the warp.
    #define WLAP(slot) PROF(c1 = clock64(); if (lane == 0) pf[slot] += c1 - c0; c0 = c1;)

    int phase = FETCH;
    u32 held = 0;              // an attempt this thread holds for the warp, when its bit of `pend` is set
    u32 pend = 0;              // the same in every thread of the warp, as is `exhausted`
    bool exhausted = false;
    u32 a = 0, deal = 0, k_moves = 0, rng = 1, prefix_rng = 1;
    int moves = 0, len = 0, depth = 0;
    u64 present = 0, l1 = 0, l3 = 0, lo = 0, hi = 0, via_lo = 0, via_hi = 0, cov_lo = 0, cov_hi = 0;
    u32 us = 0, ul = 0, used = 0, z = 0;   // z: the current depth's asleep moves (slides | leaps << 16)
    bool win = false;

    for (;;) {
        // Attempts for the threads that need one: the whole warp takes part.
        u32 need = __ballot_sync(FULL, phase == FETCH), got = 0;
        while (need) {
            if (!pend) {
                if (exhausted) break;
                u32 base = 0;
                if (lane == 0) base = atomicAdd(next_attempt, 32u);
                base = __shfl_sync(FULL, base, 0);
                u64 s = (u64)base + lane;
                u32 d, unused;
                deal_and_try((u32)s, n_deals, inv_deals, d, unused);
                bool ok = s < total && !*((volatile u32*)&won[d]);
                PROF(if (s < total) { pf[P_FETCHES]++; if (!ok) pf[P_SKIPS]++; })
                pend = __ballot_sync(FULL, ok);
                held = (u32)s;
                exhausted = (u64)base + 32 >= total;
                continue;
            }
            u32 from = __ffs(pend) - 1, to = __ffs(need) - 1;
            u32 s = __shfl_sync(FULL, held, from);
            if (lane == to) a = s;
            got |= 1u << to;
            pend &= pend - 1;
            need &= need - 1;
        }
        if ((need >> lane) & 1) phase = DONE;
        WLAP(P_FETCH);
        if ((got >> lane) & 1) {
            u32 attempt_try;
            deal_and_try(a, n_deals, inv_deals, deal, attempt_try);
            if (*((volatile u32*)&won[deal])) {
                PROF(pf[P_SKIPS]++;)                  // won since the warp took it
            } else {
                k_moves = attempt_try < tries ? prefix : prefix2;
                rng = seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (a + 1)));
                prefix_rng = rng;
                len = row_load(s_row, t, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, present, l1, l3);
                moves = 0;
                phase = PREFIX;
            }
        }
        WLAP(P_LOAD);

        if (phase == PREFIX) {
            PROF(if (lane == __ffs(__activemask()) - 1) pf[P_PREFIX_WARP_STEPS]++; pf[P_PREFIX_STEPS]++;)
            u32 n1 = __popcll(l1), n3 = __popcll(l3);
            if (moves < (int)k_moves && n1 + n3 > 0) {
                if (prefix_move(s_row, t, len, l1, l3, present, rng, leap_shift) < 0) k_moves = moves;   // all lose
                else moves++;
            } else {
                // Stuck before the residual got small, or already disconnected: lost anyway.
                bool linked = leftmost ? connected_without_leftmost(present, s_row[OFF][t], len) : connected(present);
                bool search = len <= RES && !(len > 1 && len <= (int)conn_gate && !linked);
                PROF(if (moves < (int)k_moves && len > 1) { pf[P_STUCK]++; pf[P_STUCK_MOVES] += moves; }
                     else if (len > RES) pf[P_LARGE]++;
                     else if (!search) pf[P_CUT]++;)
                win = search && len == 1;
                depth = 0;
                used = 0;
                via_lo = via_hi = cov_lo = cov_hi = 0;
                phase = FINISH;
                if (search && !win) {
                    load_residual(s_row, t, len, lo, hi);
                    u32 ms, ml;
                    masks(lo, hi, len, ms, ml);
                    us = ms; ul = ml;
                    z = 0;
                    PROF(searched = true;)
                    phase = SEARCH;
                }
            }
        }
        WLAP(P_PREFIX);
        if (phase == SEARCH) {
            PROF(if (lane == __ffs(__activemask()) - 1) pf[P_WARP_STEPS]++; pf[P_STEPS]++;)
            if ((us | ul) == 0) {
                if (depth == 0) {
                    phase = FINISH;               // the residual is lost
                } else {
                    depth--;
                    u32 via = pop_byte(via_lo, via_hi), cov = pop_byte(cov_lo, cov_hi);
                    int pile = via >> 1, dd = (via & 1) ? 3 : 1;
                    u32 mover = card_at(lo, hi, pile - dd);
                    insert_card(lo, hi, pile, mover);
                    set_card(lo, hi, pile - dd, cov);
                    present |= 1ull << cov;
                    len++;
                    u32 ms, ml;
                    masks(lo, hi, len, ms, ml);
                    z = SH16(DS + 2 * depth) | SH16(DL + 2 * depth) << 16;
                    us = ms & ~(z & 0xFFFF);
                    ul = ml & ~(z >> 16);
                }
            } else if (used >= budget) {
                phase = FINISH;
            } else {
                u32 n1 = __popc(us), n3 = __popc(ul);
                u32 k = below(rng, n1 + n3);
                bool leap = k >= n1;
                int pile = leap ? select_bit16(ul, k - n1) : select_bit16(us, k);
                u32 sleep_s, sleep_l;
                asleep_after(z & 0xFFFF, z >> 16, pile, leap, sleep_s, sleep_l);
                u32 bit = 1u << pile;
                if (leap) ul &= ~bit; else us &= ~bit;
                z |= leap ? bit << 16 : bit;
                int dd = leap ? 3 : 1;
                u32 mover = card_at(lo, hi, pile), cov = card_at(lo, hi, pile - dd);
                set_card(lo, hi, pile - dd, mover);
                delete_card(lo, hi, pile);
                int parent_len = len;
                len--;
                present &= ~(1ull << cov);
                used++;
                if (len == 1) {
                    push_byte(via_lo, via_hi, pile << 1 | (leap ? 1 : 0));
                    win = true;
                    depth++;
                    phase = FINISH;
                } else {
                    bool dead;
                    if (leftmost) {
                        // Also when pile 0's card changed, or the move broke a link among the other cards.
                        // Every thread computes the trigger and the test (both branch-free), which ran faster than
                        // testing only where the trigger fired.
                        u32 left = card_at(lo, hi, 0);
                        bool trigger = breaks_link(present, cov) | (parent_len > (int)conn_gate) | (pile == dd)
                                       | breaks_link(present & ~(1ull << left), cov);
                        dead = (len <= (int)conn_gate) & trigger & !connected_without_leftmost(present, left, len);
                    } else {
                        dead = len <= (int)conn_gate && (breaks_link(present, cov) || parent_len > (int)conn_gate)
                               && !connected(present);
                    }
                    if (dead) {                   // undo at once
                        insert_card(lo, hi, pile, mover);
                        set_card(lo, hi, pile - dd, cov);
                        present |= 1ull << cov;
                        len++;
                    } else {
                        push_byte(via_lo, via_hi, pile << 1 | (leap ? 1 : 0));
                        push_byte(cov_lo, cov_hi, cov);
                        SET16(DS + 2 * depth, z & 0xFFFF); SET16(DL + 2 * depth, z >> 16);
                        depth++;
                        z = sleep_s | sleep_l << 16;
                        u32 ms, ml;
                        masks(lo, hi, len, ms, ml);
                        us = ms & ~sleep_s; ul = ml & ~sleep_l;
                    }
                }
            }
        }
        WLAP(P_SEARCH);

        if (phase == FINISH) {
            PROF(if (searched) pf[used >= budget ? P_HMAX : used >= 1024 ? P_H1024 : used >= 256 ? P_H256
                                 : used >= 64 ? P_H64 : used >= 16 ? P_H16 : P_H0]++;
                 searched = false;)
            atomicAdd(&deal_counts[deal], (u64)(moves + used) << 16 | 1u);
            if (win && atomicCAS(&won[deal], 0u, 1u) == 0u) {
                u8* out = lines + (u64)deal * 51;
                for (int d = 0; d < depth; d++) out[moves + d] = (u8)card_at(via_lo, via_hi, depth - 1 - d);
                u32 r = prefix_rng;
                int n = row_load(s_row, t, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, present, l1, l3);
                for (int i = 0; i < moves; i++) {
                    out[i] = (u8)prefix_move(s_row, t, n, l1, l3, present, r, leap_shift);
                }
            }
            win = false;
            phase = FETCH;
        }
        WLAP(P_RESULT);
        if (__all_sync(FULL, phase == DONE)) break;
    }
    #undef WLAP
    PROF(
        for (int i = 0; i < P_BUSY_NS; i++) atomicAdd(&profile[i], pf[i]);
        const u64 t_end = now_ns();
        atomicAdd(&profile[P_BUSY_NS], t_end - t_start);
        atomicMax(&profile[P_END_NS], t_end);
        atomicMax(&profile[P_START_NS_INV], ~t_start);
    )
}

// Shared early prefix moves (attempts_split): a deal's tries go in groups of SHARE_G (tries 0 to SHARE_G - 1, and so
// on), and a group's first SHARE_K prefix moves are played once, from random numbers of the group's own; each try then
// plays the rest of its prefix and searches with random numbers of its own.
#ifndef SHARE_G
#define SHARE_G 1
#endif
#ifndef SHARE_K
#define SHARE_K 0
#endif
// The random numbers of attempt a, and of the group whose first try is attempt a0.
__device__ __forceinline__ u32 own_rng(u64 seed, u32 a) { return seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (a + 1))); }
__device__ __forceinline__ u32 group_rng(u64 seed, u32 a0) { return seed_u32(seed ^ (0xD1B54A32D192ED03ull * (a0 + 1))); }

// Writes the moves of a won attempt's search (attempts_split) at their places in its line; its won flag holds the
// attempt's number, from which write_lines plays its prefix after the launch. Playing it here took a third of the time:
// one thread, with the 31 others of its warp waiting.
__device__ __forceinline__ void write_search(u8* lines, u32 deal, int moves, int depth, u64 via_lo, u64 via_hi) {
    u8* out = lines + (u64)deal * 51;
    for (int d = 0; d < depth; d++) out[moves + d] = (u8)card_at(via_lo, via_hi, depth - 1 - d);
}

// Residuals with many ranks left rarely win for what their searches cost (Settings::rank_gate): the most ranks a
// residual of each rung may have to be searched.
#ifndef RANK_GATE
#define RANK_GATE 13
#endif
#ifndef RANK_GATE2
#define RANK_GATE2 13
#endif
// How long a prefix warp sleeps with no room in the queue: its polling takes turns from the warps doing the work.
#define QUEUE_WAIT_NS 10000
#define QSIZE 32           // residuals attempts_split's queue holds
// Search threads take residuals once TAKE_MIN of them need one, or fewer than TAKE_FLOOR are searching: taking is dear
// (a compare-and-swap, and a load of the won flag the whole warp waits for), so not every turn.
#ifndef TAKE_MIN
#define TAKE_MIN 4
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
// first; word k at w[k * PREFIX_THREADS]): the words it needs, and the masks (past 20 moves they fit 32 bits).
__device__ __forceinline__ void keep_row(u32* w, const u32 (&R)[RW], u64 l1, u64 l3, u32 rc) {
    #pragma unroll
    for (int j = 0; j < OWN_W; j++) w[j * PREFIX_THREADS] = R[j];
    w[OWN_W * PREFIX_THREADS] = (u32)l1; w[(OWN_W + 1) * PREFIX_THREADS] = (u32)l3;
    w[(OWN_W + 4) * PREFIX_THREADS] = rc;
    if (SHARE_K < 20) {
        w[(OWN_W + 2) * PREFIX_THREADS] = (u32)(l1 >> 32); w[(OWN_W + 3) * PREFIX_THREADS] = (u32)(l3 >> 32);
    }
}
__device__ __forceinline__ void kept_row(const u32* w, u32 (&R)[RW], u64& l1, u64& l3, u32& rc) {
    #pragma unroll
    for (int j = 0; j < OWN_W; j++) R[j] = w[j * PREFIX_THREADS];
    l1 = w[OWN_W * PREFIX_THREADS]; l3 = w[(OWN_W + 1) * PREFIX_THREADS];
    rc = w[(OWN_W + 4) * PREFIX_THREADS];
    if (SHARE_K < 20) {
        l1 |= (u64)w[(OWN_W + 2) * PREFIX_THREADS] << 32; l3 |= (u64)w[(OWN_W + 3) * PREFIX_THREADS] << 32;
    }
}

// The same attempts with prefixes and searches in different warps (Settings::split). In attempts_stepwise a warp's
// threads playing prefixes and those searching take turns, so each turn runs both steps at about half the warp's
// width. Here some of a block's warps play prefixes, all 32 threads stepping together (prefixes have a fixed length),
// and put each residual worth searching in a queue in shared memory; the others search, each thread taking the next
// residual as soon as it has finished one.
// Queue entry i (counted over the launch) lives in slot i % QSIZE, whose number says where it is: i, the slot is free
// for it; i + 1, written; i + QSIZE, copied out, and the slot free for entry i + QSIZE. Prefix warps reserve runs of
// free entries and search warps take runs of written ones, a compare-and-swap each; slots are freed out of order, by
// threads of different warps. Prefix threads keep their rows in registers, and in their words of s_mem the row a
// group's shared moves left (keep_row); search threads keep their asleep stacks in their words of s_mem.
// With shared early moves (SHARE_G, SHARE_K) a prefix thread takes a group of a deal's tries at a time and plays them
// one after another: the shared moves once, then each try's own moves from the row they left. Its attempts are not
// attempts_stepwise's (their first moves come from the group's random numbers); a won line replays the same way.
// The queue's shared memory leaves room for 8 blocks of 96 on the T550, 1 prefix warp to 2 search warps
// (docs/BENCHMARKS.md, "shared early prefix moves"), so the kernel may take 80 registers (24 warps a multiprocessor);
// at 72 it spilled.
extern "C" __global__ void __launch_bounds__(BLOCK, 768 / BLOCK) attempts_split(
    const u8* __restrict__ deals, const u64* __restrict__ deal_masks, u32 n_deals, u64 inv_deals, u32 tries,
    u32 prefix, u32 tries2, u32 prefix2, u32 budget, u32 conn_gate, u32 leftmost, u32 leap_shift,
    u64 seed, u32* next_attempt, u32* won, u8* lines, u64* deal_counts, u64* profile)
{
    // 16 words a thread: a prefix thread's kept row (keep_row), then a search thread's asleep moves at each depth
    // (slides | leaps << 16), so that a warp's 32 threads touch 32 different banks with one access each.
    __shared__ u32 s_mem[16 * BLOCK];
    #define ASLEEP(d) s_mem[16 * PREFIX_THREADS + (d) * (BLOCK - PREFIX_THREADS) + t - PREFIX_THREADS]
    __shared__ u64 q_lo[QSIZE], q_hi[QSIZE], q_present[QSIZE];   // the residual and its cards
    __shared__ u32 q_a[QSIZE], q_seq[QSIZE];                      // its attempt; the slot's number
    __shared__ u8 q_moves[QSIZE];                                 // its prefix's moves
    __shared__ u32 q_resv, q_head, q_closed;                      // entries reserved, entries taken, prefix warps done
    const int t = threadIdx.x;
    const u32 lane = t & 31, FULL = 0xFFFFFFFFu, below_me = (1u << lane) - 1, prefix_warps = PREFIX_THREADS / 32;
    if (t == 0) q_resv = q_head = q_closed = 0;
    for (int i = t; i < QSIZE; i += BLOCK) q_seq[i] = i;
    __syncthreads();
    if (prefix > MAX_PREFIX) prefix = MAX_PREFIX;
    if (prefix2 > MAX_PREFIX) prefix2 = MAX_PREFIX;
    // The moves a group's tries share: none when a rung's prefix is shorter.
    const int share = SHARING && (int)min(prefix, tries2 ? prefix2 : prefix) >= SHARE_K ? SHARE_K : 0;
    // Profiling: each warp's time in each part of its loop, by thread 0 of the warp, and how many of its threads
    // stepped (P_PREFIX_STEPS or P_STEPS), waited on the queue, or took and skipped attempts.
    PROF(const u64 t_start = now_ns(); u64 pf[P_SKIP + 1] = {0}, turns = 0, stepping = 0, waited = 0, taken = 0,
         skipped = 0, c0 = clock64(), c1;)
    #define WLAP(slot) PROF(c1 = clock64(); if (lane == 0) pf[slot] += c1 - c0; c0 = c1;)

    if (t < 32 * prefix_warps) {
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
        u32 held = 0, pend = 0;    // as in attempts_stepwise
        bool exhausted = false;
        u32 R[RW];
        #pragma unroll
        for (int j = 0; j < RW; j++) R[j] = SENT4;
        for (;;) {
            u32 need = FULL, got = 0, a = 0;
            while (need) {
                if (!pend) {
                    if (exhausted) break;
                    u32 base = 0;
                    if (lane == 0) base = atomicAdd(next_attempt, 32u);
                    base = __shfl_sync(FULL, base, 0);
                    u64 s = (u64)base + lane;
                    u32 d, j;
                    deal_and_try((u32)s, n_deals, inv_deals, d, j);
                    bool ok = s < total && !*((volatile u32*)&won[d]);
                    PROF(if (s < total) { u32 g = min((u32)SHARE_G, all_tries - j * SHARE_G); taken += g; if (!ok) skipped += g; })
                    pend = __ballot_sync(FULL, ok);
                    held = (u32)s;
                    exhausted = (u64)base + 32 >= total;
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
            WLAP(P_FETCH);
            if (!got) break;
            bool alive = (got >> lane) & 1;
            u32 deal = 0, left = 0, rng = 1, k_moves = 0;   // left: the group's tries after this one
            int moves = 0, len = 0, klen = 0;              // klen: the piles the shared moves left
            u64 l1 = 0, l3 = 0;
            u32 rc = 0;                                    // the others left of each rank
            if (alive) {
                u32 j;
                deal_and_try(a, n_deals, inv_deals, deal, j);    // a is the group's number here
                left = min((u32)SHARE_G, all_tries - j * SHARE_G) - 1;
                a = j * SHARE_G * n_deals + deal;               // its first try
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
                    if (alive) {
                        len = rrow_load(R, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, l1, l3);
                        rc = RANKS_FULL;
                        moves = 0;
                        k_moves = g < 0 ? share : a < tries * n_deals ? prefix : prefix2;
                        rng = g < 0 ? group_rng(seed, a) : own_rng(seed, a);
                    }
                    WLAP(P_LOAD);
                    for (int m = g < 0 ? share : kmax; m > 0; m--) {
                        PROF(u32 nm = __popc(__ballot_sync(FULL, alive && moves < (int)k_moves)); if (lane == 0) { turns++; stepping += nm; })
                        if (alive && moves < (int)k_moves) {
                            if (rrow_move<RW, 4, u64, (SHARING ? SHARED_DRAWS : DRAWS)>(R, len, l1, l3, rc, rng, leap_shift) < 0) k_moves = moves;   // all lose
                            else moves++;
                        }
                    }
                    WLAP(P_PREFIX);
                    if (g < 0) {
                        if (SHARING && alive && moves == share) {
                            keep_row(kw, R, l1, l3, rc);
                            klen = len;
                        }
                        WLAP(P_LOAD);
                        continue;
                    }
                }
                if (SHARING && share) {
                    if (alive) {
                        if (g == 0 && moves < share) {
                            // Stuck before the shared moves were all played: the group's other tries would play the
                            // same moves and search the same residual with the same random numbers, so they end with
                            // this one.
                            if (left) atomicAdd(&deal_counts[deal], (u64)left * ((u64)moves << 16 | 1u));
                            left = 0;
                            k_moves = moves;
                        } else {
                            if (g > 0) {
                                kept_row(kw, R, l1, l3, rc);
                                len = klen;
                            }
                            moves = share;
                            k_moves = a < tries * n_deals ? prefix : prefix2;
                            rng = own_rng(seed, a);
                        }
                    }
                    WLAP(P_LOAD);
                    int m = share;
                    #define OWN_MOVES(W, BITS, M, A, B)                                                              \
                        for (; m < kmax; m++) {                                                                      \
                            PROF(u32 nm = __popc(__ballot_sync(FULL, alive && moves < (int)k_moves)); if (lane == 0) { turns++; stepping += nm; }) \
                            if (alive && moves < (int)k_moves) {                                                     \
                                if (rrow_move<W, BITS, M, DRAWS>(R, len, A, B, rc, rng, leap_shift) < 0) k_moves = moves; \
                                else moves++;                                                                        \
                            }                                                                                        \
                        }
                    // One copy of the code, on the longest row a try can have (more copies, each for fewer cards, do
                    // less work but ran slower: code size shows in the instruction cache).
                    #if SHARE_K >= 20
                    u32 o1 = (u32)l1, o3 = (u32)l3;
                    OWN_MOVES(OWN_W, (SHARE_K >= 22 ? 3 : 4), u32, o1, o3)
                    #else
                    OWN_MOVES(OWN_W, 4, u64, l1, l3)
                    #endif
                    #undef OWN_MOVES
                    WLAP(P_PREFIX);
                }
                // Stuck before the residual got small, or already disconnected: lost anyway.
                bool ready = false;
                u64 present = 0;
                if (alive) {
                    present = rrow_present(R);
                    bool linked = connected_test(present, R[0] & 0xFF, len, leftmost);
                    bool search = len <= RES && !(len > 1 && len <= (int)conn_gate && !linked);
                    u32 p0 = (u32)present, p1 = (u32)(present >> 32);
                    u32 ranks = __popc((p0 | (p0 >> 16) | p1 | (p1 >> 16)) & 0x1FFFu);
                    search = search && (len < 2 || ranks <= (u32)(a < tries * n_deals ? RANK_GATE : RANK_GATE2));
                    if (search && len > 1) {
                        ready = true;
                    } else {
                        atomicAdd(&deal_counts[deal], (u64)moves << 16 | 1u);
                        if (search) atomicCAS(&won[deal], 0u, a + 1);            // one pile left: won outright
                    }
                }
                WLAP(P_PREFIX);
                // Queue the residuals, all of them before the warp goes on.
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
                        if (__shfl_sync(FULL, seen, 0) == r) break;
                    }
                    if (ready && __popc(rd & below_me) < k) {
                        u32 i = r + __popc(rd & below_me), slot = i % QSIZE;
                        q_lo[slot] = R[0] | (u64)R[1] << 32; q_hi[slot] = R[2] | (u64)R[3] << 32;   // the first 16 cards
                        q_present[slot] = present;
                        q_a[slot] = a; q_moves[slot] = (u8)moves;
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
        int moves = 0, len = 0, depth = 0;
        u64 present = 0, via_lo = 0, via_hi = 0, cov_lo = 0, cov_hi = 0;
        u32 C[4] = {SENT4, SENT4, SENT4, SENT4};
        u32 us = 0, ul = 0, used = 0, z = 0;   // z: the current depth's asleep moves (slides | leaps << 16)
        bool win = false;
        for (;;) {
            bool undo = false, fresh = false;     // what the tail of this turn's step has to do
            int pile = 0, dd = 1;
            u32 mover = 0, cov = 0;
            u32 need = __ballot_sync(FULL, phase == TAKE);
            if (__popc(need) >= TAKE_MIN || (need && __popc(__ballot_sync(FULL, phase == SEARCH)) < TAKE_FLOOR)) {
                // Take the run of written entries from the next untaken one. The count of prefix warps done is read
                // before the head, and the head before the reserved count: once all are done, every entry reserved is
                // written, and none is left once the head has reached them.
                u32 closed = *(volatile u32*)&q_closed, head = 0, k = 0, drained = 0;
                for (;;) {
                    if (lane == 0) head = *(volatile u32*)&q_head;
                    head = __shfl_sync(FULL, head, 0);
                    u32 full = __ballot_sync(FULL, lane < QSIZE && ((volatile u32*)q_seq)[(head + lane) % QSIZE] == head + lane + 1);
                    k = min((u32)__popc(need), ~full ? (u32)__ffs(~full) - 1 : 32u);
                    if (k == 0) break;
                    u32 seen = 0;
                    if (lane == 0) seen = atomicCAS(&q_head, head, head + k);
                    if (__shfl_sync(FULL, seen, 0) == head) break;
                }
                if (k == 0 && closed == prefix_warps) {
                    if (lane == 0) drained = head == *(volatile u32*)&q_resv;
                    drained = __shfl_sync(FULL, drained, 0);
                }
                if (phase == TAKE && __popc(need & below_me) < k) {
                    u32 i = head + __popc(need & below_me), slot = i % QSIZE;
                    u64 qlo = q_lo[slot], qhi = q_hi[slot];
                    C[0] = (u32)qlo; C[1] = (u32)(qlo >> 32); C[2] = (u32)qhi; C[3] = (u32)(qhi >> 32);
                    present = q_present[slot];
                    a = q_a[slot]; moves = q_moves[slot];
                    __threadfence_block();             // the copy before the slot is freed
                    ((volatile u32*)q_seq)[slot] = i + QSIZE;
                    u32 unused;
                    deal_and_try(a, n_deals, inv_deals, deal, unused);
                    len = __popcll(present);
                    fresh = true;
                    z = 0; depth = 0; used = 0;
                    via_lo = via_hi = cov_lo = cov_hi = 0;
                    phase = *((volatile u32*)&won[deal]) ? FINISH : SEARCH;   // won meanwhile: skip it
                }
                if (drained && phase == TAKE) phase = DONE;
                PROF(if (lane == 0) waited += __popc(need) - k;)
                WLAP(P_RESULT);
                if (k == 0 && !drained && __all_sync(FULL, phase != SEARCH)) __nanosleep(200);
                WLAP(P_SKIP);
            }
            PROF(u32 n = __popc(__ballot_sync(FULL, phase == SEARCH)); if (lane == 0) { turns++; stepping += n; })
            if (phase == SEARCH && !fresh) {
                if ((us | ul) == 0) {
                    if (depth == 0) {
                        phase = FINISH;               // the residual is lost
                    } else {
                        depth--;
                        u32 via = pop_byte(via_lo, via_hi);
                        cov = pop_byte(cov_lo, cov_hi);
                        pile = via >> 1;
                        dd = (via & 1) ? 3 : 1;
                        mover = card4(C, pile - dd);
                        z = ASLEEP(depth);
                        undo = true;
                        fresh = true;
                    }
                } else if (used >= budget) {
                    phase = FINISH;
                } else {
                    // Leaps first, then slides, the highest pile first: most searches run out of moves within their
                    // budget, so the order hardly matters to what they find, and a fixed one needs no random numbers.
                    bool leap = ul != 0;
                    pile = 31 - __clz(leap ? ul : us);
                    u32 sleep_s, sleep_l;
                    asleep_after(z & 0xFFFF, z >> 16, pile, leap, sleep_s, sleep_l);
                    u32 bit = 1u << pile;
                    if (leap) ul &= ~bit; else us &= ~bit;
                    z |= leap ? bit << 16 : bit;
                    dd = leap ? 3 : 1;
                    mover = card4(C, pile);
                    cov = card4(C, pile - dd);
                    play4(C, pile, pile - dd, mover);
                    len--;
                    present &= ~(1ull << cov);
                    used++;
                    if (len == 1) {
                        push_byte(via_lo, via_hi, pile << 1 | (leap ? 1 : 0));
                        win = true;
                        depth++;
                        phase = FINISH;
                    } else {
                        bool dead = (len <= (int)conn_gate) & !connected_test(present, C[0] & 0xFF, len, leftmost);
                        if (dead) {
                            undo = true;              // undone at once, below
                        } else {
                            push_byte(via_lo, via_hi, pile << 1 | (leap ? 1 : 0));
                            push_byte(cov_lo, cov_hi, cov);
                            ASLEEP(depth) = z;
                            depth++;
                            // The mover onto the covered card and then on to the card before it (or 3 before, after
                            // a slide) is the same as the covered card going there and the mover onto it, which the
                            // search tries from here: this position's masks at the covered card's pile say if it can.
                            // A sixth of the positions otherwise.
                            u32 y = pile - dd;
                            u32 cs = ((us | z) >> y) & 1, cl = dd == 1 ? ((ul | (z >> 16)) >> y) & 1 : 0;
                            z = sleep_s | sleep_l << 16 | cs << y | cl << (y + 16);
                            fresh = true;
                        }
                    }
                }
            }
            // One undo and one mask computation a turn, for the threads backing up and those going down alike, so that
            // a warp doing both runs each once and not twice.
            if (undo) {
                unplay4(C, pile, pile - dd, mover, cov);
                present |= 1ull << cov;
                len++;
            }
            if (fresh) {
                u32 ms, ml;
                masks4(C, len, ms, ml);
                us = ms & ~(z & 0xFFFF);
                ul = ml & ~(z >> 16);
            }
            WLAP(P_SEARCH);

            if (phase == FINISH) {
                atomicAdd(&deal_counts[deal], (u64)(moves + used) << 16 | 1u);
                if (win && atomicCAS(&won[deal], 0u, a + 1) == 0u)
                    write_search(lines, deal, moves, depth, via_lo, via_hi);
                win = false;
                phase = TAKE;
            }
            WLAP(P_RESULT);
            if (__all_sync(FULL, phase == DONE)) break;
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
extern "C" __global__ void write_lines(const u8* deals, const u64* deal_masks, u32 n_deals, u32 tries, u32 prefix,
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
        u32 deal = base + lane, a = 0, rng = 1, rc = RANKS_FULL;
        bool alive = deal < n_deals;
        if (alive) {
            a = __ldg(won + deal) - 1;
            alive = a != 0xFFFFFFFFu;
        }
        u32 R[RW];
        u64 l1 = 0, l3 = 0;
        int len = 0, moves = 0;
        if (alive) {
            len = rrow_load(R, deals + (u64)deal * 52, deal_masks + 3 * (u64)deal, l1, l3);
            moves = a < tries * n_deals ? (int)prefix : (int)prefix2;
            rng = share ? group_rng(seed, a - (a / n_deals) % SHARE_G * n_deals) : own_rng(seed, a);
        }
        u8* out = lines + (u64)deal * 51;
        for (int m = 0; m < (share ? share : all_moves); m++) {
            if (alive && m < moves) {
                int played = rrow_move<RW, 4, u64, (SHARING ? SHARED_DRAWS : DRAWS)>(R, len, l1, l3, rc, rng, leap_shift);
                if (played < 0) alive = false; else out[m] = (u8)played;
            }
        }
        if (alive && share) rng = own_rng(seed, a);
        for (int m = share; share && m < all_moves; m++) {
            if (alive && m < moves) {
                int played = rrow_move<RW, 4, u64, DRAWS>(R, len, l1, l3, rc, rng, leap_shift);
                if (played < 0) alive = false; else out[m] = (u8)played;
            }
        }
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
