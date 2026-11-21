//! GEDZIP entries streamed in bounded memory: a media file far larger than
//! the memory its writing and its reading take. The allocations of the
//! test's own thread are counted, so tests running beside it do not count.
#![cfg(feature = "gedzip")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{self, Cursor, Read};

use oxidged::Dataset;
use oxidged::gedzip::{GedzipReader, GedzipWriter};

/// The system allocator, counting the bytes each thread holds and the most
/// it has held.
struct Counting;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

fn allocated(size: usize) {
    let _ = LIVE.try_with(|live| {
        live.set(live.get() + size);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
    });
}

fn freed(size: usize) {
    let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(size)));
}

// SAFETY: every call is forwarded to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated(layout.size());
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        freed(layout.size());
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        freed(layout.size());
        allocated(new_size);
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most memory `f` holds at once beyond what was held before it, in
/// bytes, and what it returns.
fn peak_of<T>(f: impl FnOnce() -> T) -> (usize, T) {
    let before = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(before));
    let value = f();
    (PEAK.with(Cell::get) - before, value)
}

const SIZE: u64 = 16 << 20;
const BOUND: usize = 2 << 20;

#[test]
fn a_large_media_file_streams_in_bounded_memory() {
    // A text file, deflated, so that the archive itself stays small.
    let (written, archive) = peak_of(|| {
        let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
        writer
            .write_dataset(&Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n"))
            .unwrap();
        let size = writer
            .add_media_file_from("media/register.txt", &mut io::repeat(b'a').take(SIZE))
            .unwrap();
        assert_eq!(size, SIZE);
        writer.finish().unwrap().into_inner()
    });
    assert!(written < BOUND, "writing held {written} bytes");
    assert!(archive.len() < BOUND, "{}", archive.len());

    let mut reader = GedzipReader::new(Cursor::new(archive))
        .unwrap()
        .max_entry_size(SIZE);
    let (read, copied) = peak_of(|| {
        let mut entry = reader.open_media_file("media/register.txt").unwrap();
        io::copy(&mut entry, &mut io::sink()).unwrap()
    });
    assert_eq!(copied, SIZE);
    assert!(read < BOUND, "reading held {read} bytes");

    // Read whole, the file is held whole.
    let (whole, bytes) = peak_of(|| reader.read_media_file("media/register.txt").unwrap());
    assert_eq!(bytes.len() as u64, SIZE);
    assert!(whole >= SIZE as usize, "{whole}");
}
