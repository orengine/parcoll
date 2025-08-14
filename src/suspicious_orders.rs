//! This module provides the orderings which are not tested but seem correct.
use std::sync::atomic::Ordering;

#[cfg(feature = "untested_memory_ordering")]
/// We guess it should be [`Ordering::Relaxed`], but it is not tested.
///
/// If the `untested_memory_ordering` feature is disabled, it will be [`Ordering::Acquire`].
pub(crate) const SUSPICIOUS_RELAXED_ACQUIRE: Ordering = Ordering::Relaxed;

#[cfg(not(feature = "untested_memory_ordering"))]
/// We guess it should be [`Ordering::Relaxed`], but it is not tested.
///
/// If the `untested_memory_ordering` feature is disabled, it will be [`Ordering::Acquire`].
pub(crate) const SUSPICIOUS_RELAXED_ACQUIRE: Ordering = Ordering::Acquire;