//! Reading: the lexer alone (the lossless tree), the typed model in memory
//! and streamed, on the fixtures and a generated dataset.
//! `cargo bench --bench read`.

mod common;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use oxidged::tree::Tree;
use oxidged::{Dataset, GedcomBuilder, GedcomStreamParser};
use std::hint::black_box;

fn bench_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("read");
    for (name, text) in common::inputs() {
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::new("tree", &name), &text, |b, t| {
            b.iter(|| Tree::parse(black_box(t.clone())));
        });
        group.bench_with_input(BenchmarkId::new("dataset", &name), &text, |b, t| {
            b.iter(|| Dataset::parse(black_box(t.clone())));
        });
        group.bench_with_input(BenchmarkId::new("builder-bytes", &name), &text, |b, t| {
            b.iter(|| {
                GedcomBuilder::new()
                    .build_from_bytes(black_box(t.as_bytes()))
                    .unwrap()
            });
        });
        group.bench_with_input(BenchmarkId::new("stream", &name), &text, |b, t| {
            b.iter(|| {
                GedcomStreamParser::new(black_box(t.as_bytes()))
                    .unwrap()
                    .map(Result::unwrap)
                    .count()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_read);
criterion_main!(benches);
