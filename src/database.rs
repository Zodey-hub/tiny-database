//! The database handle and its operations.

use std::{
    collections::HashSet,
    ops::{Bound, RangeBounds},
    path::Path,
    result,
};

use redb::{
    MultimapTable, MultimapTableDefinition, MultimapTableHandle, ReadableDatabase,
    ReadableMultimapTable, ReadableTable, ReadableTableMetadata, TableDefinition, TableError,
    WriteTransaction,
};
use rkyv::rancor;

use crate::{Error, Index, Record, Result};

/// A database of typed records, stored in one file.
#[derive(Debug)]
pub struct Database {
    database: redb::Database,
}

impl Database {
    /// Opens the database file at `path`, and creates it if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the file cannot be opened, is not a database or is already open.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let database = redb::Database::create(path)?;
        Ok(Self { database })
    }

    /// Stores `record`, replacing the record with the same id and its index entries.
    ///
    /// # Errors
    ///
    /// - [`Error::Encode`] if the record cannot be serialized.
    /// - [`Error::Corrupt`] if the stored bytes of the replaced record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of an index of the type contains a dot.
    /// - One of the redb variants if the database cannot be written.
    pub fn insert<T: Record>(&self, record: &T) -> Result<()> {
        self.insert_many([record])
    }

    /// Stores each of `records` in one transaction, like [`Self::insert`]: either all of them are stored, or none.
    ///
    /// One commit is much faster than one per record, the file is synced only once. A later record replaces an earlier one with the same id.
    ///
    /// The records are borrowed, from a slice (`&users`) or an iterator (`users.iter().filter(...)`).
    ///
    /// # Errors
    ///
    /// - [`Error::Encode`] if a record cannot be serialized.
    /// - [`Error::Corrupt`] if the stored bytes of a replaced record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of an index of the type contains a dot.
    /// - One of the redb variants if the database cannot be written.
    pub fn insert_many<'a, T: Record + 'a>(
        &self,
        records: impl IntoIterator<Item = &'a T>,
    ) -> Result<()> {
        let transaction = self.database.begin_write()?;

        for record in records {
            let bytes = rkyv::to_bytes::<rancor::Error>(record).map_err(Error::Encode)?;
            replace_row(&transaction, record, &bytes)?;
        }

        transaction.commit()?;

        Ok(())
    }

    /// Returns the record with `id`, or `None` if there is none.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes are not a valid `T`.
    /// - One of the redb variants if the database cannot be read.
    pub fn get<T: Record>(&self, id: u64) -> Result<Option<T>> {
        self.read_row::<T, _>(id, |archived| deserialize(id, archived))
    }

    /// Calls `read` on the archived record with `id` without deserializing it, and returns its result, or `None` if there is no such record.
    ///
    /// The archived record borrows the stored bytes, so it cannot leave the closure: return the values that are needed from it.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes are not a valid `T`.
    /// - One of the redb variants if the database cannot be read.
    pub fn view<T: Record, R>(
        &self,
        id: u64,
        read: impl FnOnce(&T::Archived) -> R,
    ) -> Result<Option<R>> {
        self.read_row::<T, _>(id, |archived| Ok(read(archived)))
    }

    /// Returns whether there is a record with `id`, without reading it.
    ///
    /// # Errors
    ///
    /// Returns one of the redb variants if the database cannot be read.
    pub fn contains<T: Record>(&self, id: u64) -> Result<bool> {
        let transaction = self.database.begin_read()?;

        let Some(rows) = existing(transaction.open_table(rows_definition::<T>()))? else {
            return Ok(false);
        };

        Ok(rows.get(id)?.is_some())
    }

    /// Removes the record with `id` and its index entries, and returns whether it existed.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of an index of the type contains a dot.
    /// - One of the redb variants if the database cannot be written.
    pub fn delete<T: Record>(&self, id: u64) -> Result<bool> {
        let deleted = self.delete_many::<T>([id])?;
        Ok(deleted == 1)
    }

    /// Removes the records with `ids` in one transaction, like [`Self::delete`], and returns how many of them existed: either all of them are removed, or none.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of an index of the type contains a dot.
    /// - One of the redb variants if the database cannot be written.
    pub fn delete_many<T: Record>(&self, ids: impl IntoIterator<Item = u64>) -> Result<u64> {
        let transaction = self.database.begin_write()?;

        let mut deleted = 0;
        for id in ids {
            if remove_row::<T>(&transaction, id)?.is_some() {
                deleted += 1;
            }
        }

        transaction.commit()?;

        Ok(deleted)
    }

    /// Returns the records that have `key` in `index`, sorted by id.
    ///
    /// The key is encoded with [`key::enc`](crate::key::enc), the same way as in [`Record::index_keys`].
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a matching record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of `index` contains a dot.
    /// - One of the redb variants if the database cannot be read.
    pub fn find<T: Record>(&self, index: T::Index, key: &[u8]) -> Result<Vec<T>> {
        self.collect_index(index, (Bound::Included(key), Bound::Included(key)))
    }

    /// Calls `visit` on the archived records that have `key` in `index` without deserializing them, in the order of [`Self::find`].
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a matching record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of `index` contains a dot.
    /// - One of the redb variants if the database cannot be read.
    pub fn view_find<T: Record>(
        &self,
        index: T::Index,
        key: &[u8],
        mut visit: impl FnMut(&T::Archived),
    ) -> Result<()> {
        let bounds = (Bound::Included(key), Bound::Included(key));

        self.scan_index::<T>(index, bounds, |_id, archived| {
            visit(archived);
            Ok(())
        })
    }

    /// Returns the records that have a key of `index` within `range`, sorted by index key, then by id.
    ///
    /// The bounds are encoded keys, owned or borrowed: `&low..=&high`, `low..`, `..`.
    /// A record with several keys in the range is returned once, at its smallest key.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a matching record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of `index` contains a dot.
    /// - One of the redb variants if the database cannot be read.
    pub fn range<T: Record>(
        &self,
        index: T::Index,
        range: impl RangeBounds<Vec<u8>>,
    ) -> Result<Vec<T>> {
        let start = range.start_bound().map(Vec::as_slice);
        let end = range.end_bound().map(Vec::as_slice);

        self.collect_index(index, (start, end))
    }

    /// Calls `visit` on the archived records that have a key of `index` within `range` without deserializing them, in the order of [`Self::range`].
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a matching record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of `index` contains a dot.
    /// - One of the redb variants if the database cannot be read.
    pub fn view_range<T: Record>(
        &self,
        index: T::Index,
        range: impl RangeBounds<Vec<u8>>,
        mut visit: impl FnMut(&T::Archived),
    ) -> Result<()> {
        let start = range.start_bound().map(Vec::as_slice);
        let end = range.end_bound().map(Vec::as_slice);

        self.scan_index::<T>(index, (start, end), |_id, archived| {
            visit(archived);
            Ok(())
        })
    }

    /// Returns every record of the type, sorted by id.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a record are not a valid `T`.
    /// - One of the redb variants if the database cannot be read.
    pub fn all<T: Record>(&self) -> Result<Vec<T>> {
        let mut records = Vec::new();

        self.scan_rows::<T>(|id, archived| {
            records.push(deserialize(id, archived)?);
            Ok(())
        })?;

        Ok(records)
    }

    /// Calls `visit` on every archived record of the type without deserializing them, sorted by id.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a record are not a valid `T`.
    /// - One of the redb variants if the database cannot be read.
    pub fn view_all<T: Record>(&self, mut visit: impl FnMut(&T::Archived)) -> Result<()> {
        self.scan_rows::<T>(|_id, archived| {
            visit(archived);
            Ok(())
        })
    }

    /// Returns the number of records of the type, without reading them.
    ///
    /// # Errors
    ///
    /// Returns one of the redb variants if the database cannot be read.
    pub fn count<T: Record>(&self) -> Result<u64> {
        let transaction = self.database.begin_read()?;

        let Some(rows) = existing(transaction.open_table(rows_definition::<T>()))? else {
            return Ok(0);
        };

        Ok(rows.len()?)
    }

    /// Rebuilds every index of the type from its stored records, after a change of [`Record::index_keys`].
    ///
    /// The index tables of the type are dropped first, so an index that `index_keys` no longer returns disappears; a table is of the type if its name starts with `T::TABLE` and a dot. Either the whole rebuild is done or nothing changes.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a record are not a valid `T`.
    /// - [`Error::InvalidIndexName`] if the name of an index of the type contains a dot.
    /// - One of the redb variants if the database cannot be written.
    pub fn rebuild_indexes<T: Record>(&self) -> Result<()> {
        let transaction = self.database.begin_write()?;

        // the same prefix as in `index_table_name`, with the dot, so that the indexes of a table named `users2` stay
        let prefix = format!("{}.", table_name::<T>());
        let index_tables: Vec<String> = transaction
            .list_multimap_tables()?
            .map(|table| table.name().to_owned())
            .filter(|name| name.starts_with(&prefix))
            .collect();

        for name in &index_tables {
            let definition = MultimapTableDefinition::<&[u8], u64>::new(name);
            transaction.delete_multimap_table(definition)?;
        }

        let rows = transaction.open_table(rows_definition::<T>())?;

        for row in rows.iter()? {
            let (id, bytes) = row?;
            let id = id.value();
            let record = decode::<T>(id, bytes.value())?;

            for (index, key) in record.index_keys() {
                let mut entries = open_index::<T>(&transaction, &index)?;
                entries.insert(key.as_slice(), id)?;
            }
        }

        // the table borrows the transaction, it has to be closed before the commit
        drop(rows);
        transaction.commit()?;

        Ok(())
    }

    /// Replaces every record of `Old` with the `New` record that `convert` makes of it, with the index entries of `New`, in one transaction.
    ///
    /// For a change of the record type: `Old` reads the stored bytes as they are, `New` is the type from now on. Either every record is converted or none.
    /// The two types normally share their [`Record::TABLE`]; if they do not, the records move to the table of `New` and the table of `Old` is left empty.
    ///
    /// # Errors
    ///
    /// - [`Error::Corrupt`] if the stored bytes of a record are not a valid `Old`, or `convert` gives an id that a record not yet converted still has.
    /// - [`Error::Encode`] if a new record cannot be serialized.
    /// - [`Error::InvalidIndexName`] if the name of an index of the type contains a dot.
    /// - One of the redb variants if the database cannot be written.
    pub fn migrate<Old: Record, New: Record>(
        &self,
        mut convert: impl FnMut(Old) -> New,
    ) -> Result<()> {
        let transaction = self.database.begin_write()?;

        // a table cannot change while it is being iterated, only the ids are collected first
        let ids = row_ids::<Old>(&transaction)?;

        for id in ids {
            let record = remove_row::<Old>(&transaction, id)?.ok_or_else(|| corrupt::<Old>(id))?;
            let new_record = convert(record);
            let bytes = rkyv::to_bytes::<rancor::Error>(&new_record).map_err(Error::Encode)?;
            replace_row(&transaction, &new_record, &bytes)?;
        }

        transaction.commit()?;

        Ok(())
    }

    /// Calls `read` on the archived row `id`, or returns `None` if there is no such row.
    fn read_row<T: Record, R>(
        &self,
        id: u64,
        read: impl FnOnce(&T::Archived) -> Result<R>,
    ) -> Result<Option<R>> {
        let transaction = self.database.begin_read()?;

        let Some(rows) = existing(transaction.open_table(rows_definition::<T>()))? else {
            return Ok(None);
        };
        let Some(row) = rows.get(id)? else {
            return Ok(None);
        };

        let value = with_archived::<T, _>(id, row.value(), read)?;

        Ok(Some(value))
    }

    /// Loads the records that have a key of `index` within `bounds`, in the order of the index.
    fn collect_index<T: Record>(
        &self,
        index: T::Index,
        bounds: (Bound<&[u8]>, Bound<&[u8]>),
    ) -> Result<Vec<T>> {
        let mut records = Vec::new();

        self.scan_index::<T>(index, bounds, |id, archived| {
            records.push(deserialize(id, archived)?);
            Ok(())
        })?;

        Ok(records)
    }

    /// Calls `visit` with the id and the archived row of the records that have a key of `index` within `bounds`, in the order of the index.
    fn scan_index<T: Record>(
        &self,
        index: T::Index,
        bounds: (Bound<&[u8]>, Bound<&[u8]>),
        mut visit: impl FnMut(u64, &T::Archived) -> Result<()>,
    ) -> Result<()> {
        let index_table = index_table_name::<T>(&index)?;
        let definition = MultimapTableDefinition::<&[u8], u64>::new(&index_table);

        let transaction = self.database.begin_read()?;

        let Some(entries) = existing(transaction.open_multimap_table(definition))? else {
            return Ok(());
        };

        let mut ids = Vec::new();
        for entry in entries.range(bounds)? {
            let (_key, ids_of_key) = entry?;

            for id in ids_of_key {
                ids.push(id?.value());
            }
        }

        // a record with several keys within the bounds has an entry under each of them, but the ids under one key are distinct
        let single_key = matches!(bounds, (Bound::Included(s), Bound::Included(e)) if s == e);
        if !single_key {
            let mut seen = HashSet::with_capacity(ids.len());
            ids.retain(|id| seen.insert(*id));
        }

        let rows = transaction.open_table(rows_definition::<T>())?;

        for id in ids {
            // an index entry always points to a row, they are written in one transaction
            let row = rows.get(id)?.ok_or_else(|| corrupt::<T>(id))?;

            with_archived::<T, _>(id, row.value(), |archived| visit(id, archived))?;
        }

        Ok(())
    }

    /// Calls `visit` with the id and the archived row of every record of the type, sorted by id.
    fn scan_rows<T: Record>(
        &self,
        mut visit: impl FnMut(u64, &T::Archived) -> Result<()>,
    ) -> Result<()> {
        let transaction = self.database.begin_read()?;

        let Some(rows) = existing(transaction.open_table(rows_definition::<T>()))? else {
            return Ok(());
        };

        for row in rows.iter()? {
            let (id, bytes) = row?;
            let id = id.value();

            with_archived::<T, _>(id, bytes.value(), |archived| visit(id, archived))?;
        }

        Ok(())
    }
}

/// Removes the row of `id` and its index entries, and returns the removed record, or `None` if there was no such row.
fn remove_row<T: Record>(transaction: &WriteTransaction, id: u64) -> Result<Option<T>> {
    let mut rows = transaction.open_table(rows_definition::<T>())?;

    let Some(old_row) = rows.remove(id)? else {
        return Ok(None);
    };
    let old_record = decode::<T>(id, old_row.value())?;

    for (index, key) in old_record.index_keys() {
        let mut entries = open_index::<T>(transaction, &index)?;

        entries.remove(key.as_slice(), id)?;
    }

    Ok(Some(old_record))
}

/// Stores the serialized `record`, replacing the row with the same id, and updates the index entries that changed.
fn replace_row<T: Record>(transaction: &WriteTransaction, record: &T, bytes: &[u8]) -> Result<()> {
    let id = record.id();
    let mut rows = transaction.open_table(rows_definition::<T>())?;

    let old_keys = match rows.insert(id, bytes)? {
        Some(old_row) => decode::<T>(id, old_row.value())?.index_keys(),
        None => Vec::new(),
    };

    let new_keys = record.index_keys();

    // an entry that both the old and the new record have stays as it is
    for (index, key) in &old_keys {
        if !contains(&new_keys, index, key) {
            open_index::<T>(transaction, index)?.remove(key.as_slice(), id)?;
        }
    }

    for (index, key) in &new_keys {
        if !contains(&old_keys, index, key) {
            open_index::<T>(transaction, index)?.insert(key.as_slice(), id)?;
        }
    }

    Ok(())
}

/// Returns whether `entries` contain the entry of `key` in `index`.
fn contains<I: Index>(entries: &[(I, Vec<u8>)], index: &I, key: &[u8]) -> bool {
    entries
        .iter()
        .any(|(other_index, other_key)| other_index.name() == index.name() && other_key == key)
}

/// Turns a table that does not exist into `None`.
fn existing<Table>(opened: result::Result<Table, TableError>) -> Result<Option<Table>> {
    match opened {
        Ok(table) => Ok(Some(table)),
        // redb creates a table on the first write to it, before that there is nothing to read
        Err(TableError::TableDoesNotExist(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Returns the definition of the table of `T`: id -> rkyv bytes.
fn rows_definition<T: Record>() -> TableDefinition<'static, u64, &'static [u8]> {
    TableDefinition::new(table_name::<T>())
}

/// Returns [`Record::TABLE`] of `T`, and fails the build if it contains a dot.
///
/// Every operation reaches the table name through this function, so the check runs for each record type in use.
const fn table_name<T: Record>() -> &'static str {
    // with a dot in it the index tables of two types could have the same name, or one of them the name of a rows table
    const {
        assert!(
            has_no_dot(T::TABLE),
            "the `TABLE` of a record type contains a dot"
        );
    };

    T::TABLE
}

/// Returns whether `name` contains no dot, the separator of the table and the index in the name of an index table.
const fn has_no_dot(name: &str) -> bool {
    let mut rest = name.as_bytes();
    while let [first, tail @ ..] = rest {
        if *first == b'.' {
            return false;
        }
        rest = tail;
    }

    true
}

/// Validates the stored bytes of the row `id`, and calls `read` on the archived record.
fn with_archived<T: Record, R>(
    id: u64,
    bytes: &[u8],
    read: impl FnOnce(&T::Archived) -> Result<R>,
) -> Result<R> {
    // redb returns the bytes at an arbitrary offset of its page, the `unaligned` feature of rkyv lets them be read in place
    // the rkyv error is dropped, it has no details in a release build
    let archived =
        rkyv::access::<T::Archived, rancor::Error>(bytes).map_err(|_| corrupt::<T>(id))?;

    read(archived)
}

/// Returns the error of the row `id` that is not a valid `T`, or is missing under an index entry.
fn corrupt<T: Record>(id: u64) -> Error {
    Error::Corrupt {
        table: T::TABLE,
        id,
    }
}

/// Deserializes the validated archived record of the row `id`.
fn deserialize<T: Record>(id: u64, archived: &T::Archived) -> Result<T> {
    // the rkyv error is dropped, it has no details in a release build
    rkyv::deserialize::<T, rancor::Error>(archived).map_err(|_| corrupt::<T>(id))
}

/// Validates and deserializes the stored bytes of the row `id`.
fn decode<T: Record>(id: u64, bytes: &[u8]) -> Result<T> {
    with_archived::<T, _>(id, bytes, |archived| deserialize(id, archived))
}

/// Returns the name of the multimap of `index`, or [`Error::InvalidIndexName`] if the name of the index contains a dot.
fn index_table_name<T: Record>(index: &T::Index) -> Result<String> {
    let table = table_name::<T>();
    let index = index.name();

    // a dot here would let the index tables of two indexes, or of two types, have the same name
    if !has_no_dot(index) {
        return Err(Error::InvalidIndexName { table, index });
    }

    Ok(format!("{table}.{index}"))
}

/// Opens the multimap of an index of `T` for writing: encoded key -> ids.
fn open_index<'txn, T: Record>(
    transaction: &'txn WriteTransaction,
    index: &T::Index,
) -> Result<MultimapTable<'txn, &'static [u8], u64>> {
    let index_table = index_table_name::<T>(index)?;
    let definition = MultimapTableDefinition::new(&index_table);

    let entries = transaction.open_multimap_table(definition)?;

    Ok(entries)
}

/// Returns the ids of every row of `T`, in order.
fn row_ids<T: Record>(transaction: &WriteTransaction) -> Result<Vec<u64>> {
    let rows = transaction.open_table(rows_definition::<T>())?;

    let mut ids = Vec::new();
    for row in rows.iter()? {
        let (id, _bytes) = row?;
        ids.push(id.value());
    }

    Ok(ids)
}
