import random, sys, time
from probe import moves
rng = random.Random(7)
for k in [int(x) for x in sys.argv[1:]]:
    deck = list(range(52)); rng.shuffle(deck)
    s = tuple(deck[:k]); seen = {s}; frontier = [s]; t0 = time.time()
    while frontier:
        nxt = []
        for x in frontier:
            for y in moves(x):
                if y not in seen: seen.add(y); nxt.append(y)
        frontier = nxt
        if time.time() - t0 > 40: print(f"k={k}: >{len(seen):,} (aborted)", flush=True); break
    else:
        print(f"k={k}: {len(seen):,} reachable states ({time.time()-t0:.1f}s)", flush=True)
