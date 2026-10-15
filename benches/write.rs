//! Writing: the typed model in the version it declares and converted to
//! the other, and the lossless tree, on the fixtures and a generated
//! dataset. `cargo bench --bench write`.

mod common;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use oxidged::tree::Tree;
use oxidged::{Dataset, GedcomVersion, GedcomWriter};
use std::hint::black_box;

fn bench_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("write");
    for (name, text) in common::inputs() {
        let data = Dataset::parse(text.as_str());
        let tree = Tree::parse(text.as_str());
        let writer = GedcomWriter::new();
        let size = writer.write_to_string(&data).unwrap().len();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("dataset", &name), &data, |b, d| {
            b.iter(|| writer.write_to_string(black_box(d)).unwrap());
        });
        let other = if data.version().is_v7() {
            GedcomVersion::V5_5_1
        } else {
            GedcomVersion::V7_0
        };
        let converting = GedcomWriter::new().gedcom_version(other);
        group.bench_with_input(
            BenchmarkId::new("dataset-converted", &name),
            &data,
            |b, d| {
                b.iter(|| converting.write_to_string(black_box(d)).unwrap());
            },
        );
        group.bench_with_input(BenchmarkId::new("tree", &name), &tree, |b, t| {
            b.iter(|| {
                let mut out = Vec::with_capacity(size);
                writer.write_tree(&mut out, black_box(t)).unwrap();
                out
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_write);
criterion_main!(benches);
