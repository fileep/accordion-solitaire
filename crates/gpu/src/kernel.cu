// Ladder attempts on the GPU (PLAN.md Phase 5 track F, Phase 8): one attempt per thread. An attempt plays `prefix`
// random moves on its position, then a first-win depth-first search of what is left (at most 16 piles) with sleep
// sets and the connectivity dead-end test, up to `budget` positions. The rules are the Rust fast engine's
// (crates/core/src/fast.rs); every win is replayed by the CPU's reference engine before it counts, and the
// `playouts` kernel at the end lets the tests (crates/gpu/tests) check this rules code against the Rust engines.
//
// Everything stays on chip. The prefix plays on a 64-byte row in shared memory (stored byte-major, so a warp's 32
// threads touch 32 different banks), starting from legal-move masks the host works out once per position. The
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
// replays its prefix from the same random numbers to write its line.
//
// Threads are persistent: each keeps taking the next attempt from a global counter. Attempt i is try i / deals of
// deal i % deals, so a deal's tries are spread over the run; once some thread has won a deal, its later tries are
// skipped. Two rungs share one launch: the first `tries` tries of a deal use `prefix` random moves, the next
// `tries2` use `prefix2`.
//
// There are two attempt kernels with the same arguments and results. `attempts` runs each thread's attempts start to
// finish; its threads soon drift apart, and a warp then runs them nearly one at a time. `attempts_stepwise`, the
// default, moves every thread one step (a prefix move or a search position) each turn of its loop, so the threads
// doing the same thing step together: 1.7 times the deals per second (docs/BENCHMARKS.md, "After Phase 8 — where
// the GPU's time goes" and "the GPU's bit operations").
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
// 0x1FFF, so adding 0x7FFF sets its bit 15 exactly when it isn't empty, and never carries into the next.
__device__ __forceinline__ u64 suits_with(u64 present, u64 ranks) {
    return ((present & (ranks * 0x0001000100010001ull)) + 0x7FFF7FFF7FFF7FFFull) & 0x8000800080008000ull;
}

// The ranks the suits flagged in `suits` (bit 15 of their lanes) hold between them.
__device__ __forceinline__ u64 ranks_of(u64 present, u64 suits) {
    u64 r = present & (suits - (suits >> 15));     // 0x7FFF in each flagged lane
    r |= r >> 32;
    return (r | r >> 16) & ROW;
}

// fast.rs connected: the suits present are linked, two suits when they share a rank. Branch-free, so a warp never
// waits for its slowest thread (the search loop it replaced, one suit a turn until nothing changed, cost about 18%
// of the kernel's time): from the lowest suit present, three steps reach every suit linked to it.
__device__ __forceinline__ bool connected(u64 present) {
    u64 all = (present + 0x7FFF7FFF7FFF7FFFull) & 0x8000800080008000ull;
    u64 ranks = (present >> ((__ffsll(all) - 16) & 63)) & ROW;     // the lowest suit's (none present: 0)
    u64 reached = suits_with(present, ranks);
    reached = suits_with(present, ranks_of(present, reached));
    reached = suits_with(present, ranks_of(present, reached));
    return reached == all;
}

// fast.rs connected_without_leftmost: pile 0 never moves, so the other top cards must be connected among themselves
// and pile 0's card `left` must match one of them.
__device__ __forceinline__ bool connected_without_leftmost(u64 present, u32 left, int len) {
    u64 rest = present & ~(1ull << left);
    bool matched = (((rest >> (16 * (left >> 4))) & ROW) | ((rest >> (left & 0x0F)) & 0x0001000100010001ull)) != 0;
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
    PROF(u64 pf[P_SLOTS - 3] = {0}; const u64 t_start = now_ns(); u64 c0 = clock64(), c1;)
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
        for (int i = 0; i < P_SLOTS - 3; i++) atomicAdd(&profile[i], pf[i]);
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
    PROF(u64 pf[P_SLOTS - 3] = {0}; const u64 t_start = now_ns(); bool searched = false; u64 c0 = clock64(), c1;)
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
        for (int i = 0; i < P_SLOTS - 3; i++) atomicAdd(&profile[i], pf[i]);
        const u64 t_end = now_ns();
        atomicAdd(&profile[P_BUSY_NS], t_end - t_start);
        atomicMax(&profile[P_END_NS], t_end);
        atomicMax(&profile[P_START_NS_INV], ~t_start);
    )
}

// Random playouts for the tests (crates/gpu/tests), one per thread, recording what the code above computes at every
// step for the CPU to check against its engines. While the position has more than RES piles it plays on the row (the
// prefix's code); from RES piles down, in registers (the search's code), where every move is also undone and redone.
// Per step, the position's slide and leap masks (two u64), the move played from it (pile << 1 | leap, or 0xFF when
// there is none, which ends the playout), and flags: bit 0 connected(present), bit 1 the last move broke a link
// (breaks_link), bit 2 undoing the move did not restore the position, bit 3 connected_without_leftmost.
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
    bool regs = false, link = false;
    for (int step = 0; step < 52; step++) {
        const u64 at = (u64)p * 52 + step;
        if (!regs && len <= RES) {
            load_residual(s_row, t, len, lo, hi);
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
        u8 flags = (connected(present) ? 1 : 0) | (link ? 2 : 0) | (connected_without_leftmost(present, left, len) ? 8 : 0);
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
                len--;
            } else {
                covered = row_move(s_row, t, len, l1, l3, pile, leap);
            }
            present &= ~(1ull << covered);
            link = breaks_link(present, covered);
        }
        flags_out[at] = flags;
        moves_out[at] = mv;
        if (mv == 0xFF) break;
    }
}
