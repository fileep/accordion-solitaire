// Ladder attempts on the GPU (PLAN.md Phase 5 track F, Phase 8): one attempt per thread. An attempt plays `prefix`
// random moves on its position, then a first-win depth-first search of what is left (at most 16 piles) with sleep
// sets and the connectivity dead-end test, up to `budget` positions. The rules are the Rust fast engine's
// (crates/core/src/fast.rs); every win is replayed by the CPU's reference engine before it counts, and the
// `playouts` kernel at the end lets the tests (crates/gpu/tests) check this rules code against the Rust engines.
//
// Everything stays on chip. The prefix plays on a 64-byte row in shared memory (stored byte-major, so a warp's 32
// threads touch 32 different banks). The search keeps its at most 16 cards in two 64-bit registers, recomputes the
// legal-move masks with byte-parallel bit tricks, and undoes moves instead of copying positions. Its moves and
// covered cards are byte stacks in registers; the only per-depth state in shared memory is the asleep moves.
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

#define ONES 0x0101010101010101ull
#define HIGHS 0x8080808080808080ull

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
// The position of the k-th set bit of m (k from 0), by the hardware's find-n-th-set instruction.
__device__ __forceinline__ int select_bit64(u64 m, u32 k) {
    for (u32 i = 0; i < k; i++) m &= m - 1;
    return __ffsll((long long)m) - 1;
}
__device__ __forceinline__ int select_bit32(u32 m, u32 k) {
    for (u32 i = 0; i < k; i++) m &= m - 1;
    return __ffs((int)m) - 1;
}

// Covering `covered` broke the last rank its suit shared with another suit (fast.rs breaks_link).
__device__ __forceinline__ bool breaks_link(u64 present, u32 covered) {
    u32 s = covered >> 4, r = covered & 0x0F;
    u64 row = (present >> (16 * s)) & ROW;
    bool broken = false;
    #pragma unroll
    for (u32 s2 = 0; s2 < 4; s2++) {
        u64 other = (present >> (16 * s2)) & ROW;
        broken |= (s2 != s) & (((other >> r) & 1) == 1) & ((other & row) == 0);
    }
    return broken & (row != 0);
}

__device__ bool connected(u64 present) {
    u64 rows[4] = {present & ROW, (present >> 16) & ROW, (present >> 32) & ROW, (present >> 48) & ROW};
    u32 nonempty = 0;
    #pragma unroll
    for (int s = 0; s < 4; s++) if (rows[s]) nonempty |= 1u << s;
    if (!nonempty) return true;
    u32 reached = 1u << (__ffs(nonempty) - 1);
    for (;;) {
        u64 ranks = 0;
        #pragma unroll
        for (int s = 0; s < 4; s++) if ((reached >> s) & 1) ranks |= rows[s];
        u32 next = reached;
        #pragma unroll
        for (int s = 0; s < 4; s++) if (rows[s] & ranks) next |= 1u << s;
        if (next == reached) return reached == nonempty;
        reached = next;
    }
}

// fast.rs connected_without_leftmost: pile 0 never moves, so the other top cards must be connected among themselves
// and pile 0's card `left` must match one of them.
__device__ bool connected_without_leftmost(u64 present, u32 left, int len) {
    if (len < 2) return true;
    u64 rest = present & ~(1ull << left);
    bool matched = (((rest >> (16 * (left >> 4))) & ROW) | ((rest >> (left & 0x0F)) & 0x0001000100010001ull)) != 0;
    return matched && connected(rest);
}

// ---- The row: piles 0.. at row[OFF..], SENTINEL around them, and the legal-move masks l1 (slides) and l3 (leaps).

// Loads a position onto the row; returns its pile count and sets `present` and the masks.
__device__ __forceinline__ int row_load(u8 (*row)[BLOCK], int t, const u8* cards, u64& present, u64& l1, u64& l3) {
    for (int i = 0; i < 64; i++) row[i][t] = SENTINEL;
    present = 0;
    int len = 0;
    while (len < 52 && cards[len] != SENTINEL) {
        u8 c = cards[len];
        row[OFF + len][t] = c;
        present |= 1ull << c;
        len++;
    }
    l1 = 0; l3 = 0;
    for (int i = 1; i < len; i++) {
        u32 c = row[OFF + i][t];
        l1 |= (u64)codes_match(c, row[OFF + i - 1][t]) << i;
        l3 |= (u64)codes_match(c, row[OFF + i - 3][t]) << i;
    }
    return len;
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
// 0x80 in every byte of t (bytes <= 0x7F) that is zero.
__device__ __forceinline__ u64 zero_bytes(u64 t) { return ~((t + 0x7F7F7F7F7F7F7F7Full) | t) & HIGHS; }
// Bit j set for every byte j of m that has 0x80 set.
__device__ __forceinline__ u32 gather(u64 m) { return (u32)((((m >> 7) & ONES) * 0x0102040810204080ull) >> 56); }
__device__ __forceinline__ u32 match_mask(u64 a_lo, u64 a_hi, u64 b_lo, u64 b_hi) {
    u64 d_lo = a_lo ^ b_lo, d_hi = a_hi ^ b_hi;
    u64 m_lo = zero_bytes(d_lo & 0x0F0F0F0F0F0F0F0Full) | zero_bytes(d_lo & 0x7070707070707070ull);
    u64 m_hi = zero_bytes(d_hi & 0x0F0F0F0F0F0F0F0Full) | zero_bytes(d_hi & 0x7070707070707070ull);
    return gather(m_lo) | (gather(m_hi) << 8);
}
// Legal-move masks of the residual: bit i of s, pile i may slide; of l, leap.
__device__ __forceinline__ void masks(u64 lo, u64 hi, int len, u32& s, u32& l) {
    u64 p1_lo = (lo << 8) | 0x7Full, p1_hi = (hi << 8) | (lo >> 56);
    u64 p3_lo = (lo << 24) | 0x7F7F7Full, p3_hi = (hi << 24) | (lo >> 40);
    u32 valid = (1u << len) - 1;
    s = match_mask(lo, hi, p1_lo, p1_hi) & valid & ~1u;
    l = match_mask(lo, hi, p3_lo, p3_hi) & valid & ~7u;
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
    u32 n_deals, u32 tries, u32 prefix, u32 tries2, u32 prefix2, u32 budget, u32 conn_gate,
    u32 leftmost,                      // 1: the connectivity test leaves pile 0's card out (stronger)
    u64 seed,
    u32* next_attempt,                 // global work counter, starts at 0
    u32* won,                          // per position: 0, or 1 once some attempt has won it
    u8* lines,                         // 51 bytes per position
    u64* deal_counts)                  // per position: positions generated << 16 | attempts run
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
    for (;;) {
        u32 a = atomicAdd(next_attempt, 1u);
        if (a >= total) break;
        u32 deal = a % n_deals;
        const u32 k_moves = (a / n_deals < tries) ? prefix : prefix2;
        if (*((volatile u32*)&won[deal])) continue;
        u32 rng = seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (a + 1)));
        const u32 prefix_rng = rng;

        // The position and its random prefix.
        u64 present, l1, l3;
        int len = row_load(s_row, t, deals + (u64)deal * 52, present, l1, l3);
        int moves = 0;
        while (moves < (int)k_moves) {
            u32 n1 = __popcll(l1), n3 = __popcll(l3);
            if (n1 + n3 == 0) break;
            u32 k = below(rng, n1 + n3);
            bool leap = k >= n1;
            int pile = leap ? select_bit64(l3, k - n1) : select_bit64(l1, k);
            u32 covered = row_move(s_row, t, len, l1, l3, pile, leap);
            present &= ~(1ull << covered);
            moves++;
        }
        // Stuck before the residual got small, or already disconnected: lost anyway.
        bool linked = leftmost ? connected_without_leftmost(present, s_row[OFF][t], len) : connected(present);
        bool search = len <= RES && !(len > 1 && len <= (int)conn_gate && !linked);

        bool win = search && len == 1;
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
                int pile = leap ? select_bit32(ul, k - n1) : select_bit32(us, k);
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
        atomicAdd(&deal_counts[deal], (u64)(moves + used) << 16 | 1u);
        if (win && atomicCAS(&won[deal], 0u, 1u) == 0u) {
            u8* out = lines + (u64)deal * 51;
            // The search's moves, first to last: the first is deepest in the stack.
            for (int d = 0; d < depth; d++) out[moves + d] = (u8)card_at(via_lo, via_hi, depth - 1 - d);
            // Replay the prefix: the same random numbers on the same position choose the same moves.
            u32 r = prefix_rng;
            int n = row_load(s_row, t, deals + (u64)deal * 52, present, l1, l3);
            for (int i = 0; i < moves; i++) {
                u32 n1 = __popcll(l1), n3 = __popcll(l3);
                u32 k = below(r, n1 + n3);
                bool leap = k >= n1;
                int pile = leap ? select_bit64(l3, k - n1) : select_bit64(l1, k);
                row_move(s_row, t, n, l1, l3, pile, leap);
                out[i] = (u8)(pile << 1 | (leap ? 1 : 0));
            }
        }
    }
}

// Random playouts for the tests (crates/gpu/tests), one per thread, recording what the code above computes at every
// step for the CPU to check against its engines. While the position has more than RES piles it plays on the row (the
// prefix's code); from RES piles down, in registers (the search's code), where every move is also undone and redone.
// Per step, the position's slide and leap masks (two u64), the move played from it (pile << 1 | leap, or 0xFF when
// there is none, which ends the playout), and flags: bit 0 connected(present), bit 1 the last move broke a link
// (breaks_link), bit 2 undoing the move did not restore the position, bit 3 connected_without_leftmost.
extern "C" __global__ void __launch_bounds__(BLOCK) playouts(
    const u8* __restrict__ positions, u32 n, u64 seed,
    u64* masks_out,                    // 52 steps × 2 per playout
    u8* moves_out, u8* flags_out)      // 52 steps per playout
{
    __shared__ u8 s_row[64][BLOCK];
    const int t = threadIdx.x;
    const u32 p = blockIdx.x * BLOCK + t;
    if (p >= n) return;
    u32 rng = seed_u32(seed ^ (0x9E3779B97F4A7C15ull * (p + 1)));
    u64 present, l1, l3, lo = 0, hi = 0;
    int len = row_load(s_row, t, positions + (u64)p * 52, present, l1, l3);
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
