//! Benchmarks for parsing, lookups, and updates.

#![allow(missing_docs)]

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use env_lib::Env;

fn sample(n: usize) -> String {
    let mut text = String::from("# generated\n\n");
    for i in 0..n {
        let _ = writeln!(text, "KEY_{i}=value-{i}");
        if i % 5 == 0 {
            let _ = writeln!(text, "QUOTED_{i}=\"two words \\n and an escape\"  # note");
        }
    }
    text
}

fn bench_parse(c: &mut Criterion) {
    let small = sample(10);
    let large = sample(1_000);
    let mut g = c.benchmark_group("parse");
    g.bench_function("10_entries", |b| {
        b.iter(|| Env::from_text(black_box(&small)));
    });
    g.bench_function("1000_entries", |b| {
        b.iter(|| Env::from_text(black_box(&large)));
    });
    g.finish();
}

fn bench_get(c: &mut Criterion) {
    let Ok(env) = Env::from_text(&sample(100)) else {
        return;
    };
    let mut g = c.benchmark_group("get");
    g.bench_function("hit", |b| {
        b.iter(|| env.get(black_box("KEY_50")).map(|v| v.len()));
    });
    g.bench_function("hit_prefixed_name", |b| {
        b.iter(|| env.get(black_box("JG_KEY_50")).map(|v| v.len()));
    });
    g.bench_function("miss_falls_back_to_process", |b| {
        b.iter(|| env.get(black_box("NOT_THERE")).map(|v| v.len()));
    });
    g.bench_function("parse_u32", |b| {
        b.iter(|| env.parse::<u32>(black_box("KEY_50")));
    });
    g.finish();
}

fn bench_set(c: &mut Criterion) {
    let mut g = c.benchmark_group("set");
    g.bench_function("update_existing", |b| {
        let Ok(mut env) = Env::from_text(&sample(100)) else {
            return;
        };
        let mut n = 0u32;
        b.iter(|| {
            n = n.wrapping_add(1);
            env.set(black_box("KEY_50"), n.to_string())
        });
    });
    g.finish();
}

criterion_group!(benches, bench_parse, bench_get, bench_set);
criterion_main!(benches);
