//! Micro-benchmarks for the fast engine's per-node operations (PLAN.md Phase 2). Each benchmark runs over the
//! same few thousand positions taken from random playouts, and reports throughput per position or move.
//!
//!   cargo bench -p accordion-core

use accordion_core::fast::{Position, connected};
use accordion_core::rng::Xoshiro256StarStar;
use accordion_core::{Deal, Move};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

/// (position, the random legal move played from it) along 200 random playouts.
fn playout_steps() -> Vec<(Position, Move)> {
    let mut out = Vec::new();
    for seed in 0..200 {
        let mut pos = Position::from_deal(&Deal::from_seed(seed));
        let mut rng = Xoshiro256StarStar::from_seed(seed);
        while let Some(mv) = pos.random_move(&mut rng) {
            out.push((pos, mv));
            pos = pos.play(mv);
        }
    }
    out
}

fn engine(c: &mut Criterion) {
    let steps = playout_steps();
    let small: Vec<Position> = steps.iter().map(|s| s.0).filter(|p| p.len() <= 19).collect();
    let mut g = c.benchmark_group("engine");

    g.throughput(Throughput::Elements(steps.len() as u64));
    g.bench_function("apply (move + incremental masks, hash, trigger)", |b| {
        b.iter(|| {
            for (pos, mv) in &steps {
                let mut next = *pos;
                next.apply(mv.pile as usize, mv.leap);
                black_box(&next);
            }
        })
    });
    g.bench_function("hash_after (child hash only, for prefetch)", |b| {
        b.iter(|| steps.iter().fold(0u64, |acc, (pos, mv)| acc ^ pos.hash_after(mv.pile as usize, mv.leap)))
    });
    g.bench_function("random_move (popcnt + pdep)", |b| {
        let mut rng = Xoshiro256StarStar::from_seed(1);
        b.iter(|| {
            for (pos, _) in &steps {
                black_box(pos.random_move(&mut rng));
            }
        })
    });
    g.bench_function("full_masks (recompute from scratch)", |b| {
        b.iter(|| steps.iter().fold(0u64, |acc, (pos, _)| acc ^ pos.full_masks().0))
    });
    g.bench_function("full_hash (recompute from scratch)", |b| {
        b.iter(|| steps.iter().fold(0u64, |acc, (pos, _)| acc ^ pos.full_hash()))
    });

    g.throughput(Throughput::Elements(small.len() as u64));
    g.bench_function("connected (<= 19 piles)", |b| {
        b.iter(|| small.iter().filter(|p| connected(black_box(p.present()))).count())
    });
    g.bench_function("left_leaf_pair (<= 19 piles)", |b| {
        b.iter(|| small.iter().filter(|p| black_box(*p).left_leaf_pair()).count())
    });
    g.finish();
}

criterion_group!(benches, engine);
criterion_main!(benches);
