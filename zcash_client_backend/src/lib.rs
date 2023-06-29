//! *A crate for implementing Zcash light clients.*
//!
//! `zcash_client_backend` contains Rust structs and traits for creating shielded Zcash
//! light clients.

// Catch documentation errors caused by code changes.
#![deny(broken_intra_doc_links)]
// Temporary until we have addressed all Result<T, ()> cases.
#![allow(clippy::result_unit_err)]

pub mod address;
pub mod data_api;
mod decrypt;
pub mod encoding;
pub mod keys;
pub mod proto;
pub mod wallet;
pub mod welding_rig;
pub mod zip321;

pub use decrypt::{decrypt_transaction, DecryptedOutput};

#[macro_export]
macro_rules! try_f {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(e) => return Box::new(futures01::future::err(e.into())),
        }
    };
}
