#![deny(clippy::all)]
#![deny(clippy::assertions_on_result_states)]
#![deny(clippy::match_wild_err_arm)]
#![deny(clippy::allow_attributes_without_reason)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
#![warn(clippy::cargo)]
#![allow(clippy::multiple_crate_versions, reason = "They were set by dev-dependencies")]
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
pub mod backoff;
pub(crate) mod batch_receiver;
pub mod buffer_version;
pub mod cache_padded;
mod consumer;
pub mod hints;
mod light_arc;
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

pub use batch_receiver::BatchReceiver;
pub use consumer::{Consumer, LockFreeConsumer};
pub use light_arc::LightArc;
pub use lock_free_errors::*;
pub use mutex_vec_queue::MutexVecQueue;
pub use producer::{LockFreeProducer, Producer};
