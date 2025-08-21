//! This module contains the implementation of the MPMC queue.
//!
//! [`const_bounded`]: A const bounded ring buffer.
//! Use [`new_bounded`] or [`new_cache_padded_bounded`]
//! or [`MPMCBoundedQueue`].
mod const_bounded;
#[cfg(test)]
mod tests;

pub use const_bounded::*;
