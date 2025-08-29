//! # `ParColl`
//!
//! This crate provides optimized collections which can be used in concurrent runtimes.
//!
//! It provides optimized ring-based [`SPSC`](spsc)
//! ([`const bounded`](spsc::new_bounded) or [`unbounded`](spsc::new_unbounded)),
//! [`SPMC`](spmc) ([`const bounded`](spmc::new_bounded) or [`unbounded`](spmc::new_unbounded)),
//! and [`const bounded`](mpmc::new_bounded) [`MPMC`](mpmc) queue.
//!
//! All queues are lock-free (or lock-free with the proper generics),
//! generalized and can be either be cache-padded or not.
//!
//! ```rust
//! use parcoll::{Consumer, Producer};
//!
//! fn mpmc() {
//!     let (producer, consumer) = parcoll::mpmc::new_cache_padded_bounded::<_, 256>();
//!     let producer2 = producer.clone();
//!     let consumer2 = consumer.clone(); // You can clone the consumer
//!
//!     producer.maybe_push(1).unwrap();
//!     producer.maybe_push(2).unwrap();
//!
//!     let mut slice = [std::mem::MaybeUninit::uninit(); 3];
//!     let popped = consumer.pop_many(&mut slice);
//!
//!     assert_eq!(popped, 2);
//!     assert_eq!(unsafe { slice[0].assume_init() }, 1);
//!     assert_eq!(unsafe { slice[1].assume_init() }, 2);
//! }
//!
//! fn spsc_unbounded() {
//!     let (producer, consumer) = parcoll::spsc::new_cache_padded_unbounded();
//!
//!     producer.maybe_push(1).unwrap();
//!     producer.maybe_push(2).unwrap();
//!
//!     let mut slice = [std::mem::MaybeUninit::uninit(); 3];
//!     let popped = consumer.pop_many(&mut slice);
//!
//!     assert_eq!(popped, 2);
//!     assert_eq!(unsafe { slice[0].assume_init() }, 1);
//!     assert_eq!(unsafe { slice[1].assume_init() }, 2);
//! }
//!
//! fn spmc() {
//!     let (producer1, consumer1) = parcoll::spmc::new_bounded::<_, 256>();
//!     let (producer2, consumer2) = parcoll::spmc::new_bounded::<_, 256>();
//!
//!     for i in 0..100 {
//!         producer1.maybe_push(i).unwrap();
//!     }
//!
//!     consumer1.steal_into(&producer2);
//!
//!     assert_eq!(producer2.len(), 50);
//!     assert_eq!(consumer1.len(), 50);
//! }
//! ```

#![deny(clippy::all)]
#![deny(clippy::assertions_on_result_states)]
#![deny(clippy::match_wild_err_arm)]
#![deny(clippy::allow_attributes_without_reason)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
#![warn(clippy::cargo)]
#![allow(
    clippy::redundant_feature_names,
    reason = "It is impossible to write feature-dependencies without it"
)]
#![allow(
    clippy::multiple_crate_versions,
    reason = "They were set by dev-dependencies"
)]
#![allow(async_fn_in_trait, reason = "It improves readability.")]
#![allow(
    clippy::missing_const_for_fn,
    reason = "Since we cannot make a constant function non-constant after its release,
    we need to look for a reason to make it constant, and not vice versa."
)]
#![allow(clippy::inline_always, reason = "We write highly optimized code.")]
#![allow(
    clippy::must_use_candidate,
    reason = "It is better to developer think about it."
)]
#![allow(
    clippy::module_name_repetitions,
    reason = "This is acceptable most of the time."
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "Unless the error is something special,
    the developer should document it."
)]
#![allow(clippy::redundant_pub_crate, reason = "It improves readability.")]
#![allow(clippy::struct_field_names, reason = "It improves readability.")]
#![allow(
    clippy::module_inception,
    reason = "It is fine if a file in has the same mane as a module."
)]
#![allow(clippy::if_not_else, reason = "It improves readability.")]
#![allow(
    rustdoc::private_intra_doc_links,
    reason = "It allows to create more readable docs."
)]
#![allow(
    clippy::result_unit_err,
    reason = "The function's doc should explain what it returns."
)]
pub(crate) mod batch_receiver;
pub mod buffer_version;
pub(crate) mod cache_padded;
mod consumer;
mod lock_free_errors;
#[cfg(all(parcoll_loom, test))]
mod loom;
pub(crate) mod loom_bindings;
pub mod mpmc;
pub mod multi_consumer;
pub mod multi_producer;
pub(crate) mod mutex_vec_queue;
pub mod naive_rw_lock;
pub mod number_types;
mod producer;
pub mod single_consumer;
pub mod single_producer;
pub mod spmc;
pub mod spmc_producer;
pub mod spsc;
pub(crate) mod suspicious_orders;
pub mod sync_cell;
#[cfg(not(parcoll_loom))]
mod test_lock;

pub use batch_receiver::{BatchReceiver, LockFreeBatchReceiver};
pub use consumer::{Consumer, LockFreeConsumer};
pub use lock_free_errors::*;
pub use mutex_vec_queue::MutexVecQueue;
pub use producer::{LockFreeProducer, Producer};
pub use orengine_utils::light_arc::LightArc;
pub use orengine_utils;
pub use orengine_utils::VecQueue;