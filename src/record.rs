//! The traits of the record types and of their indexes.

use rkyv::{
    Archive, Deserialize, Serialize,
    api::high::{HighDeserializer, HighSerializer, HighValidator},
    bytecheck::CheckBytes,
    rancor,
    ser::allocator::ArenaHandle,
    util::AlignedVec,
};

/// A record type: the rows of one table.
///
/// The type derives rkyv's `Archive`, `Serialize` and `Deserialize`, which provide the supertraits.
pub trait Record:
    Sized
    + for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>
    + Archive<
        Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>
                      + Deserialize<Self, HighDeserializer<rancor::Error>>,
    >
{
    /// Table name. Unique per database, cannot contain '.'.
    const TABLE: &'static str;

    /// The indexes of this type, [`NoIndex`] if it has none.
    type Index: Index;

    /// Returns the id of the record, which the caller chooses.
    fn id(&self) -> u64;

    /// Returns the index entries of the record: pairs of an index and a key encoded with [`key::enc`](crate::key::enc).
    ///
    /// A multi-valued field returns a pair for each of its values, an index without a pair gets no entry.
    /// After a change of the keys it returns, call [`Database::rebuild_indexes`](crate::Database::rebuild_indexes) before any other write of the type.
    fn index_keys(&self) -> Vec<(Self::Index, Vec<u8>)>;
}

/// The indexes of a record type: an enum with a variant for each index.
pub trait Index {
    /// Returns the name of the index, which is also the suffix of its index table. Unique per record type, and without a dot.
    fn name(&self) -> &'static str;
}

/// The [`Record::Index`] of a record type without indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoIndex {}

impl Index for NoIndex {
    fn name(&self) -> &'static str {
        match *self {}
    }
}
