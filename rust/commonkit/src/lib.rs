//! commonkit: named shared-memory segments for numpy `.npy` arrays.
//!
//! One process publishes an array as a `.npy` file under a store root, and every other process
//! attaches the same file (mapped read-only, or copied). The on-disk contract is
//! `docs/contract.md` in the repository; the Python package `pycommonkit` is the reference
//! implementation, and `conformance/vectors/` is the arbiter.
//!
//! **The invariant:** a missing segment is never an error, only a slower path. Nothing on the
//! read, publish or reap path panics or returns an error for anything the contract names, except
//! the two loud paths: invalid key or scheme input, and a key collision on publish (contract §1.6).
//!
//! ```no_run
//! use commonkit::{ArrayRef, AttachMode, Descr, KeyScheme, Mode, Order, Variants, keys};
//!
//! let scheme = keys::register(KeyScheme::new(
//!     "demo", "v1", ["f32"], Variants::closed(["x0"])?, &[("f32", "<f4")])?)?;
//! let key = scheme.key("0123456789abcdef0123456789abcdef", "x0", None)?;
//! let store = commonkit::shared_store(Mode::ReadWrite, AttachMode::Mmap, 0);
//! let descr = Descr::parse("<f4")?;
//! let data = [0u8; 16];
//! let stored = store.publish(key.as_str(), ArrayRef::new(&descr, &[4], Order::C, &data)?)?;
//! if let Some(seg) = store.attach(key.as_str()) {
//!     let values: Option<&[f32]> = seg.as_slice();
//!     # let _ = (stored, values);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

/// The on-disk contract version this crate implements (contract §1.1).
pub const CONTRACT_VERSION: u32 = 1;

mod json;
pub mod keys;
#[cfg(feature = "mmap")]
mod map;
pub mod npy;
pub mod policy;
pub mod segment;
pub mod store;
mod sys;

pub use keys::{Key, KeyError, KeyScheme, Registry, Variants};
pub use npy::{Descr, Order};
pub use policy::{Policy, shared_store, shared_store_at};
pub use segment::{AttachMode, Element, Scalar, Segment};
pub use store::{ArrayRef, Counter, KeyCollision, Mode, Options, Stats, Store, Usage};
