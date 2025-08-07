//! This module provides implementations of a single-producer single-consumer queues.
//!
//! It contains two implementations:
//!
//! * [`const_bounded`]: A const bounded ring buffer.
//!   Use [`new_bounded`] or [`new_cache_padded_bounded`] or [`SPSCBoundedQueue`].
//! * [`unbounded`]: An unbounded ring buffer.
//!   Use [`new_unbounded`] or [`new_cache_padded_unbounded`].
//!
//! And it also contains the [`Producer`] and [`Consumer`] traits.
mod const_bounded;
mod consumer;
mod producer;
#[cfg(test)]
mod tests;
mod unbounded;

pub use const_bounded::*;
pub use consumer::*;
pub use producer::*;
pub use unbounded::*;