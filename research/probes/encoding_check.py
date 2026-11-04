# Checks the planned engine encoding and incremental updates against from-scratch recomputation:
#   card code   = suit << 4 | rank   (rank 0..12 = A..K, suit 0..3), padding sentinel 0x7F
#   match(a, b) = ((a ^ b) & 0x0F) == 0  or  ((a ^ b) & 0x70) == 0
#   present     = u64 with bit (card code) set; suit s is bits 16s..16s+12
#   l1 / l3     = u64 masks; bit i set iff pile i may move 1 / 3 to the left
#   hash        = XOR of Z[left][right] over adjacent pairs, with sentinels at both ends
# After each move: masks are shifted down above i and only a small window is recomputed;
# the hash is updated with 3 (slide) or 7 (leap) XORs. Random play compares both against full recomputation.
import random

SENT = 0x7F
CARDS = [s << 4 | r for s in range(4) for r in range(13)]

def match(a, b):
    x = a ^ b
    return (x & 0x0F) == 0 or (x & 0x70) == 0

def full_masks(c):
    l1 = l3 = 0
    for i in range(1, len(c)):
        if match(c[i], c[i - 1]): l1 |= 1 << i
        if i >= 3 and match(c[i], c[i - 3]): l3 |= 1 << i
    return l1, l3

rng = random.Random(1)
START, END = 64, 65                      # sentinel ids for the hash table
Z = {(a, b): rng.getrandbits(64) for a in CARDS + [START] for b in CARDS + [END]}

def full_hash(c):
    seq = [START] + c + [END]
    h = 0
    for a, b in zip(seq, seq[1:]): h ^= Z[(a, b)]
    return h

def bit(m, j): return (m >> j) & 1
def setbit(m, j, v): return (m & ~(1 << j)) | (v << j)

def apply(c, l1, l3, h, i, d):
    n = len(c)
    a = c[i - d - 1] if i - d - 1 >= 0 else START     # card left of the target
    t, mv = c[i - d], c[i]                            # target top card, moving card
    nxt = c[i + 1] if i + 1 < n else END              # card right of the mover
    if d == 1:
        h ^= Z[(a, t)] ^ Z[(t, mv)] ^ Z[(a, mv)]                          # 3 XORs
        nc = c[:i - 1] + [mv] + c[i + 1:]
    else:
        x, y = c[i - 2], c[i - 1]
        h ^= Z[(a, t)] ^ Z[(t, x)] ^ Z[(y, mv)] ^ Z[(mv, nxt)] \
             ^ Z[(a, mv)] ^ Z[(mv, x)] ^ Z[(y, nxt)]                     # 7 XORs
        nc = c[:i - 3] + [mv] + c[i - 2:i] + c[i + 1:]
    low = (1 << i) - 1                                # bits below i stay, bits above shift down one
    l1 = (l1 & low) | ((l1 >> 1) & ~low)
    l3 = (l3 & low) | ((l3 >> 1) & ~low)
    m = len(nc)
    # minimal recompute windows derived in RESEARCH 6.3
    w1, w3 = ((i - 1,), (i - 1, i, i + 1)) if d == 1 else ((i - 3, i - 2, i), (i - 3, i, i + 1, i + 2))
    for j in w1:
        if 1 <= j < m: l1 = setbit(l1, j, int(match(nc[j], nc[j - 1])))
    for j in w3:
        if 3 <= j < m: l3 = setbit(l3, j, int(match(nc[j], nc[j - 3])))
    full = (1 << m) - 1
    # piles 0 can't slide and piles 0-2 can't leap: the shift can carry a stale bit into those slots
    return nc, l1 & full & ~0b1, l3 & full & ~0b111, h

def pick_random_move(l1, l3, r):
    # uniform pick over all set bits; in Rust: pdep(1 << r, mask) then tzcnt (O(1) on BMI2)
    n1, n3 = bin(l1).count("1"), bin(l3).count("1")
    k = r.randrange(n1 + n3)
    mask, d = (l1, 1) if k < n1 else (l3, 3)
    k = k if k < n1 else k - n1
    for _ in range(k): mask &= mask - 1             # clear k lowest bits (stand-in for pdep)
    return (mask & -mask).bit_length() - 1, d

assert all(not match(SENT, c) for c in CARDS), "sentinel must never match a card"
assert sum(match(a, b) for a in CARDS for b in CARDS if a != b) == 52 * 15, "each card matches exactly 15 others"

games = moves = 0
for g in range(20000):
    deck = CARDS[:]; rng.shuffle(deck)
    c = deck; l1, l3 = full_masks(c); h = full_hash(c)
    while l1 | l3:
        i, d = pick_random_move(l1, l3, rng)
        c, l1, l3, h = apply(c, l1, l3, h, i, d)
        assert (l1, l3) == full_masks(c), (g, i, d)
        assert h == full_hash(c), (g, i, d)
        moves += 1
    games += 1
print(f"OK: {games:,} random games, {moves:,} moves; incremental masks and hash matched full recomputation every time")
