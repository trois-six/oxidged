//! Memory and lookups: the bytes a dataset keeps, and finding records
//! linearly and through the index. `cargo bench --bench memory`.
//!
//! With `MODEL_RSS=<pipeline>` (`dataset`, `stream` or `tree`), it instead
//! reads the file `MODEL_FILE` (a generated one of `MODEL_PEOPLE`
//! individuals, 330 000 by default, about 80 MB, when unset) once with that
//! pipeline and prints the input size, the time and the peak resident set
//! (Linux `VmHWM`): run it once per pipeline, each in its own process.
//! `MODEL_WRITE=0` leaves the writing out.

mod common;

use criterion::{Criterion, criterion_group};
use oxidged::tree::Tree;
use oxidged::{Dataset, GedcomBuilder, GedcomStreamParser, GedcomWriter, IndexedDataset};
use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// The system allocator, counting the bytes allocated and not freed.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size, Ordering::Relaxed);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The bytes the reading of each input keeps, against its size.
fn bench_kept(c: &mut Criterion) {
    let mut group = c.benchmark_group("kept");
    for (name, text) in common::inputs() {
        let before = LIVE.load(Ordering::Relaxed);
        let data = Dataset::parse(text.clone());
        let kept = LIVE.load(Ordering::Relaxed).saturating_sub(before);
        eprintln!(
            "{name}: {} bytes read, {kept} kept ({:.2}x)",
            text.len(),
            kept as f64 / text.len() as f64
        );
        drop(data);
        group.bench_function(&name, |b| {
            b.iter(|| Dataset::parse(black_box(text.clone())).record_count());
        });
    }
    group.finish();
}

/// Finding 50 individuals linearly and through the index, and indexing.
fn bench_lookups(c: &mut Criterion) {
    let mut group = c.benchmark_group("lookups");
    let data = Dataset::parse(common::generated(20_000));
    let xrefs: Vec<String> = data
        .individuals()
        .iter()
        .step_by(400)
        .filter_map(|i| i.xref())
        .map(|x| data.store().xref(x).to_string())
        .collect();
    group.bench_function("linear-50", |b| {
        b.iter(|| {
            for x in &xrefs {
                black_box(data.find_individual(x.as_str()));
            }
        });
    });
    let indexed = IndexedDataset::new(data.clone());
    group.bench_function("indexed-50", |b| {
        b.iter(|| {
            for x in &xrefs {
                black_box(indexed.find_individual(x.as_str()));
            }
        });
    });
    group.bench_function("index-creation", |b| {
        b.iter(|| IndexedDataset::new(black_box(data.clone())));
    });
    group.finish();
}

/// The peak resident set of this process, in bytes (Linux).
fn peak_rss() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// One pipeline, one read, measured: see the module documentation.
fn measure(pipeline: &str) {
    let path = std::env::var("MODEL_FILE").unwrap_or_else(|_| {
        let people = std::env::var("MODEL_PEOPLE")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(330_000);
        let path = std::env::temp_dir().join(format!("oxidged-model-{people}.ged"));
        if !path.exists() {
            fs::write(&path, common::generated(people)).expect("write the generated file");
        }
        path.to_string_lossy().into_owned()
    });
    let size = fs::metadata(&path).expect("the file").len() as usize;
    let before = peak_rss().unwrap_or(0);
    let start = Instant::now();
    let kept: Box<dyn std::any::Any> = match pipeline {
        "tree" => Box::new(Tree::from_bytes(fs::read(&path).expect("read the file"))),
        "dataset" => Box::new(
            GedcomBuilder::new()
                .build_from_bytes(fs::read(&path).expect("read the file"))
                .unwrap(),
        ),
        "stream" => {
            let file = std::io::BufReader::new(fs::File::open(&path).expect("open the file"));
            let records = GedcomStreamParser::new(file)
                .unwrap()
                .map(Result::unwrap)
                .count();
            Box::new(records)
        }
        other => panic!("unknown pipeline {other}"),
    };
    let elapsed = start.elapsed().as_secs_f64();
    let peak = peak_rss().unwrap_or(0);
    let live = LIVE.load(Ordering::Relaxed);
    black_box(&kept);
    let mb = size as f64 / 1e6;
    println!(
        "{pipeline}: read {mb:.1} MB in {elapsed:.3} s ({:.0} MB/s); peak RSS {:.0} MB = {:.2}x the input (before reading: {:.0} MB); kept {:.0} MB = {:.2}x the input",
        mb / elapsed,
        peak as f64 / 1e6,
        peak as f64 / size as f64,
        before as f64 / 1e6,
        live as f64 / 1e6,
        live as f64 / size as f64,
    );
    if let Some(data) = kept.downcast_ref::<Dataset>() {
        println!(
            "{pipeline}: of which the store (input, pieces, identifiers) {:.0} MB",
            data.store().heap_size() as f64 / 1e6
        );
    }
    // `MODEL_WRITE=0` measures the reading alone (for a heap profiler).
    if std::env::var("MODEL_WRITE").is_ok_and(|w| w == "0") {
        return;
    }
    let writer = GedcomWriter::new();
    let start = Instant::now();
    let written = if let Some(tree) = kept.downcast_ref::<Tree>() {
        let mut out = Vec::new();
        writer.write_tree(&mut out, tree).unwrap();
        out.len()
    } else if let Some(data) = kept.downcast_ref::<Dataset>() {
        writer.write_to_string(data).unwrap().len()
    } else {
        return;
    };
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{pipeline}: wrote {:.1} MB in {elapsed:.3} s ({:.0} MB/s); peak RSS {:.0} MB",
        written as f64 / 1e6,
        written as f64 / 1e6 / elapsed,
        peak_rss().unwrap_or(0) as f64 / 1e6,
    );
}

criterion_group!(benches, bench_kept, bench_lookups);

fn main() {
    if let Ok(pipeline) = std::env::var("MODEL_RSS") {
        measure(&pipeline);
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
