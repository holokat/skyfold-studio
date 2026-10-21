//! Parallel helpers.
//!
//! Every parallel pass in the crate is "gather" shaped: one task computes one
//! output from inputs that no task writes during the pass. Outputs indexed by
//! particle (rather than by position in a work list) are written through
//! [`SharedMut`], whose callers guarantee that each index is written by exactly
//! one task. Reductions use fixed-size chunks summed in order, so results are
//! bit-identical whatever the thread count.

use rayon::prelude::*;
use std::marker::PhantomData;

/// Below this many items a pass runs on the calling thread.
pub(crate) const SEQ_THRESHOLD: usize = 1024;

/// Constraint color groups smaller than this are solved on the calling
/// thread: a projection costs tens of nanoseconds, less than a fork-join.
pub(crate) const GROUP_SEQ_THRESHOLD: usize = 8192;

/// Below this many items a pass with only a few nanoseconds of work per item
/// (integration, keying) runs on the calling thread: forking would cost more.
pub(crate) const CHEAP_SEQ_THRESHOLD: usize = 32_768;

/// Rayon minimum split length for per-particle passes.
pub(crate) const MIN_LEN: usize = 128;

/// A mutable slice that several tasks may write at disjoint indices.
pub(crate) struct SharedMut<'a, T> {
    ptr: *mut T,
    len: usize,
    _marker: PhantomData<&'a mut [T]>,
}

// SAFETY: access is only through the unsafe methods below, whose contracts
// require disjoint indices across tasks.
unsafe impl<T: Send> Send for SharedMut<'_, T> {}
unsafe impl<T: Send> Sync for SharedMut<'_, T> {}

impl<'a, T> SharedMut<'a, T> {
    pub fn new(slice: &'a mut [T]) -> Self {
        Self {
            ptr: slice.as_mut_ptr(),
            len: slice.len(),
            _marker: PhantomData,
        }
    }

    /// # Safety
    /// No other task may access index `i` while the returned reference lives.
    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self, i: usize) -> &mut T {
        debug_assert!(i < self.len);
        // SAFETY: in bounds (checked in debug) and exclusive per the contract.
        unsafe { &mut *self.ptr.add(i) }
    }

    /// # Safety
    /// No other task may access `[start, start + len)` while the slice lives.
    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slice(&self, start: usize, len: usize) -> &mut [T] {
        debug_assert!(start + len <= self.len);
        // SAFETY: in bounds (checked in debug) and exclusive per the contract.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(start), len) }
    }

    /// Read index `i`.
    ///
    /// # Safety
    /// No task may be writing index `i` concurrently.
    #[inline(always)]
    pub unsafe fn read(&self, i: usize) -> &T {
        debug_assert!(i < self.len);
        // SAFETY: in bounds and not concurrently written per the contract.
        unsafe { &*self.ptr.add(i) }
    }
}

/// Run `f(i)` for every index in `items`, in parallel when there are enough.
pub(crate) fn for_each_index<F>(items: &[u32], f: F)
where
    F: Fn(usize) + Sync + Send,
{
    if items.len() < SEQ_THRESHOLD {
        for &i in items {
            f(i as usize);
        }
    } else {
        items
            .par_iter()
            .with_min_len(MIN_LEN)
            .for_each(|&i| f(i as usize));
    }
}

/// Like [`for_each_index`] for passes of a few nanoseconds per item.
pub(crate) fn for_each_index_cheap<F>(items: &[u32], f: F)
where
    F: Fn(usize) + Sync + Send,
{
    if items.len() < CHEAP_SEQ_THRESHOLD {
        for &i in items {
            f(i as usize);
        }
    } else {
        items
            .par_iter()
            .with_min_len(4096)
            .for_each(|&i| f(i as usize));
    }
}

/// Like [`for_each_range`] for passes of a few nanoseconds per item.
pub(crate) fn for_each_range_cheap<F>(n: usize, f: F)
where
    F: Fn(usize) + Sync + Send,
{
    if n < CHEAP_SEQ_THRESHOLD {
        (0..n).for_each(f);
    } else {
        (0..n).into_par_iter().with_min_len(4096).for_each(f);
    }
}

/// Run `f(i)` for every `i` in `0..n`, in parallel when `n` is large enough.
pub(crate) fn for_each_range<F>(n: usize, f: F)
where
    F: Fn(usize) + Sync + Send,
{
    if n < SEQ_THRESHOLD {
        (0..n).for_each(f);
    } else {
        (0..n).into_par_iter().with_min_len(MIN_LEN).for_each(f);
    }
}

/// Deterministic parallel sum: fixed 1024-item chunks, partial sums added in order.
pub(crate) fn chunked_sum<T, F>(n: usize, zero: T, f: F) -> T
where
    T: Copy + Send + Sync + std::ops::Add<Output = T>,
    F: Fn(usize) -> T + Sync + Send,
{
    const CHUNK: usize = 1024;
    let chunks = n.div_ceil(CHUNK);
    let partial: Vec<T> = if chunks <= 1 {
        vec![(0..n).fold(zero, |acc, i| acc + f(i))]
    } else {
        (0..chunks)
            .into_par_iter()
            .map(|c| {
                let end = ((c + 1) * CHUNK).min(n);
                (c * CHUNK..end).fold(zero, |acc, i| acc + f(i))
            })
            .collect()
    };
    partial.into_iter().fold(zero, |acc, v| acc + v)
}
