// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// The global allocator of the firmwares: a TLSF heap that remembers a failed allocation, so the
// panic handler can signal it, and counts the bytes in use for the status line

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use embedded_alloc::TlsfHeap;

// TLSF keeps fragmentation low, which matters for the large temporary path blocks
#[doc(hidden)]
pub static HEAP: TlsfHeap = TlsfHeap::empty();

static SIZE: AtomicUsize = AtomicUsize::new(0);
// set when an allocation fails
static OUT_OF_MEMORY: AtomicBool = AtomicBool::new(false);
// heap bytes in use and the highest value so far
// (load/store only, the RP2040 has no atomic read-modify-write, the firmwares are single threaded)
static USED: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Global allocator that remembers a failed allocation and counts the heap use
struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { HEAP.alloc(layout) };
        if ptr.is_null() {
            OUT_OF_MEMORY.store(true, Ordering::Relaxed);
        } else {
            let used = USED.load(Ordering::Relaxed) + layout.size();
            USED.store(used, Ordering::Relaxed);
            if used > PEAK.load(Ordering::Relaxed) {
                PEAK.store(used, Ordering::Relaxed);
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        USED.store(USED.load(Ordering::Relaxed) - layout.size(), Ordering::Relaxed);
        unsafe { HEAP.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

/// Gives the heap a static block of `$size` bytes, before anything is allocated (before Slint is
/// used). A macro, because the size of the static block must be a constant of the firmware.
#[macro_export]
macro_rules! init_heap {
    ($size:expr) => {{
        static mut HEAP_MEMORY: [core::mem::MaybeUninit<u8>; $size] =
            [core::mem::MaybeUninit::uninit(); $size];
        // SAFETY: the block is only used by the heap, which is initialized once, here
        unsafe { $crate::heap::init(&raw mut HEAP_MEMORY as usize, $size) }
    }};
}

#[doc(hidden)]
/// # Safety
/// Called once, with a block of memory used by nothing else
pub unsafe fn init(start: usize, size: usize) {
    SIZE.store(size, Ordering::Relaxed);
    unsafe { HEAP.init(start, size) }
}

/// Whether an allocation has failed
pub fn out_of_memory() -> bool {
    OUT_OF_MEMORY.load(Ordering::Relaxed)
}

/// The most heap bytes requested at once so far, without the overhead of the allocator
pub fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Size of the heap in bytes
pub fn size() -> usize {
    SIZE.load(Ordering::Relaxed)
}
