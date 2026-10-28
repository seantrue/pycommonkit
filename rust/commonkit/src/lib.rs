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
