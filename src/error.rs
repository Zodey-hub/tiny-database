//! The error type of the crate.

use std::result;

use rkyv::rancor;

/// The result of a database operation.
pub type Result<T> = result::Result<T, Error>;

/// The error of a database operation.
///
/// New variants can come in a minor version, a `match` on it needs a `_` arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The database file could not be opened or created.
    #[error(transparent)]
    Database(#[from] redb::DatabaseError),
    /// A transaction could not be started.
    #[error(transparent)]
    Transaction(#[from] redb::TransactionError),
    /// A table could not be opened.
    #[error(transparent)]
    Table(#[from] redb::TableError),
    /// The database file could not be read or written.
    #[error(transparent)]
    Storage(#[from] redb::StorageError),
    /// The transaction could not be committed.
    #[error(transparent)]
    Commit(#[from] redb::CommitError),
    /// The record could not be serialized.
    #[error("cannot encode the record")]
    Encode(#[source] rancor::Error),
    /// The name of an index contains a dot, which would let two index tables have the same name.
    #[error("the index name {index:?} of the table {table:?} contains a dot")]
    InvalidIndexName {
        /// The table of the record type.
        table: &'static str,
        /// The name of the index.
        index: &'static str,
    },
    /// The stored bytes of a row are not a valid record, or an index entry points to a missing row.
    #[error("corrupt data in {table} id {id}")]
    Corrupt {
        /// The table of the row.
        table: &'static str,
        /// The id of the row.
        id: u64,
    },
}
