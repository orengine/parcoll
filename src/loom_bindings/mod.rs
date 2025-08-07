//! This module abstracts over `loom` and `std::sync` depending on whether we
//! are running tests or not.

#![allow(
    unused,
    reason = "It should implement the whole API, even if it is unused for now."
)]

#[cfg(not(all(test, parcoll_loom)))]
mod std;
#[cfg(not(all(test, parcoll_loom)))]
pub use self::std::*;

#[cfg(all(test, parcoll_loom))]
mod mocked;
#[cfg(all(test, parcoll_loom))]
pub use self::mocked::*;
