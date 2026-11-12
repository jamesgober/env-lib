//! Parsing memory stays proportional to the file, not to its line count.
//! This binary installs a counting allocator, so it holds this test only.

#![allow(clippy::unwrap_used, missing_docs, unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded to the system allocator unchanged; the
// counters are only bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            let _previous = PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        let _previous = CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn peak_bytes<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let result = f();
    (result, PEAK.load(Ordering::Relaxed).saturating_sub(base))
}

#[test]
fn test_a_file_of_blank_lines_costs_about_its_size() {
    // The largest file `Env` reads, all newlines: it used to take about
    // 170 MiB to parse, one entry-sized element per line.
    let text = "\n".repeat(1024 * 1024 - 1);
    let (env, peak) = peak_bytes(|| env_lib::Env::from_text(&text).unwrap());
    assert!(env.is_empty());
    assert!(peak < 4 * 1024 * 1024, "peak {peak} bytes");
    // Comments are the same.
    let comments = "# note\n".repeat(140_000);
    let (_env, peak) = peak_bytes(|| env_lib::Env::from_text(&comments).unwrap());
    assert!(peak < 4 * 1024 * 1024, "peak {peak} bytes");
}
