//! Allocation evidence for the isolated benchmark executables, never installed by the server library.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Install as a binary's global allocator to measure allocations including injected regressions.
pub struct MeasuredAllocator;
fn allocated(bytes: usize) {
    INSTALLED.store(true, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
// SAFETY: each operation forwards the original pointer/layout to System and updates counters
// only after a successful allocation. The allocator performs no allocations itself.
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            if size >= layout.size() {
                allocated(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        pointer
    }
}
pub fn allocation_bytes() -> Result<(usize, usize), String> {
    if !INSTALLED.load(Ordering::Relaxed) {
        return Err("native benchmark requires MeasuredAllocator allocation instrumentation".into());
    }
    Ok((LIVE.load(Ordering::Relaxed), PEAK.load(Ordering::Relaxed)))
}

pub fn peak_rss_bytes() -> Result<u64, String> {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage initializes the supplied correctly sized rusage on success.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
            let usage = unsafe { usage.assume_init() };
            let bytes = usage.ru_maxrss.max(0) as u64;
            return Ok(if cfg!(target_os = "macos") { bytes } else { bytes * 1024 });
        }
        return Err(std::io::Error::last_os_error().to_string());
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[repr(C)]
        struct Counters {
            cb: u32,
            faults: u32,
            peak_working_set: usize,
            working_set: usize,
            peak_paged_pool: usize,
            paged_pool: usize,
            peak_nonpaged_pool: usize,
            nonpaged_pool: usize,
            pagefile: usize,
            peak_pagefile: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
        }
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn GetProcessMemoryInfo(process: *mut c_void, counters: *mut Counters, size: u32) -> i32;
        }
        let mut counters = std::mem::MaybeUninit::<Counters>::zeroed();
        let size = std::mem::size_of::<Counters>() as u32;
        // SAFETY: the integer-only structure is zero-initialized; set its required byte count.
        unsafe { (*counters.as_mut_ptr()).cb = size };
        // SAFETY: process is the current-process pseudo handle and the buffer has the declared size.
        let result = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), size) };
        if result != 0 {
            return Ok(unsafe { counters.assume_init() }.peak_working_set as u64);
        }
        return Err(std::io::Error::last_os_error().to_string());
    }
    #[cfg(not(any(unix, windows)))]
    Err("native peak resident memory is unavailable on this platform".into())
}
