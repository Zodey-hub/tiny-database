#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

mod database;
mod error;
pub mod key;
mod record;

pub use database::Database;
pub use error::{Error, Result};
pub use record::{Index, NoIndex, Record};
pub use rkyv;
