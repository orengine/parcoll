//! This module provides implementations of single-producer multi-consumer queues.
//!
//! It contains two implementations:
//!
//! * [`const_bounded`]: A const bounded ring buffer.
//!   Use [`new_bounded`] or [`new_cache_padded_bounded`] or [`SPMCBoundedQueue`].
//! * [`unbounded`]: An unbounded ring buffer.
//!   Use one of the [`new_unbounded`], [`new_cache_padded_unbounded`],
//!   [`new_unbounded_with_sync_cell`] and [`new_cache_padded_unbounded_with_sync_cell`].
mod const_bounded;
#[cfg(test)]
mod tests;
mod unbounded;

pub use const_bounded::*;
pub use unbounded::*;
