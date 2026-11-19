//! The behavior of the public API, each test on a database in its own temporary directory.

use redb::{
    MultimapTableDefinition, ReadableDatabase, ReadableMultimapTable, ReadableTable,
    TableDefinition,
};
use rkyv::{Archive, Deserialize, Serialize};
use tempfile::TempDir;
use tiny_database::{Database, Error, Index, Record, Result, key};

/// The table of the users as the library stores it, to change it behind the back of the library.
const USER_ROWS: TableDefinition<u64, &[u8]> = TableDefinition::new("users");

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
struct User {
    id: u64,
    name: String,
    age: u8,
    tags: Vec<String>,
}

impl User {
    fn new(id: u64, name: &str, age: u8, tags: &[&str]) -> Self {
        let tags = tags.iter().map(|&tag| tag.to_owned()).collect();

        Self {
            id,
            name: name.to_owned(),
            age,
            tags,
        }
    }
}

enum UserIndex {
    Name,
    Age,
    Tag,
}

impl Index for UserIndex {
    fn name(&self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Age => "age",
            Self::Tag => "tag",
        }
    }
}

impl Record for User {
    const TABLE: &'static str = "users";
    type Index = UserIndex;

    fn id(&self) -> u64 {
        self.id
    }

    fn index_keys(&self) -> Vec<(UserIndex, Vec<u8>)> {
        let mut keys = vec![
            (UserIndex::Name, key::enc(&self.name)),
            (UserIndex::Age, key::enc(self.age)),
        ];
        let tag_keys = self.tags.iter().map(|tag| (UserIndex::Tag, key::enc(tag)));
        keys.extend(tag_keys);

        keys
    }
}

/// Opens a new database, its directory is deleted when the returned guard is dropped.
fn temp_db() -> (TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open(directory.path().join("test.redb")).unwrap();

    (directory, db)
}

fn ids(users: &[User]) -> Vec<u64> {
    users.iter().map(|user| user.id).collect()
}

#[test]
fn insert_get_overwrite_delete() -> Result<()> {
    let (_directory, db) = temp_db();
    let alice = User::new(1, "Alice", 30, &[]);
    let older_alice = User::new(1, "Alice", 31, &[]);

    db.insert(&alice)?;
    let inserted = db.get::<User>(1)?;
    db.insert(&older_alice)?;
    let overwritten = db.get::<User>(1)?;
    let deleted = db.delete::<User>(1)?;
    let after_delete = db.get::<User>(1)?;
    let deleted_again = db.delete::<User>(1)?;

    assert_eq!(inserted, Some(alice));
    assert_eq!(overwritten, Some(older_alice));
    assert!(deleted);
    assert_eq!(after_delete, None);
    assert!(!deleted_again);
    Ok(())
}

#[test]
fn a_new_database_has_no_records() -> Result<()> {
    // no table exists before the first insert, that has to read as empty and not as an error
    let (_directory, db) = temp_db();

    let by_id = db.get::<User>(1)?;
    let everyone = db.all::<User>()?;
    let found = db.find::<User>(UserIndex::Name, &key::enc("Alice"))?;
    let ranged = db.range::<User>(UserIndex::Age, ..)?;
    let viewed = db.view::<User, _>(1, |user| user.age)?;
    let mut visited = 0;
    db.view_all::<User>(|_user| visited += 1)?;
    db.view_range::<User>(UserIndex::Age, .., |_user| visited += 1)?;
    let deleted = db.delete::<User>(1)?;

    assert_eq!(by_id, None);
    assert_eq!(viewed, None);
    assert_eq!(visited, 0);
    assert_eq!(everyone, []);
    assert_eq!(found, []);
    assert_eq!(ranged, []);
    assert!(!deleted);
    Ok(())
}

#[test]
fn table_of_another_kind_is_an_error() -> Result<()> {
    // only a missing table reads as empty, any other failure to open it has to surface
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let database = redb::Database::create(&path)?;
    let transaction = database.begin_write()?;
    transaction.open_multimap_table(MultimapTableDefinition::<&[u8], u64>::new("users"))?;
    transaction.commit()?;
    drop(database);
    let db = Database::open(&path)?;

    let error = db.get::<User>(1).unwrap_err();

    assert!(matches!(error, Error::Table(_)), "{error:?}");
    Ok(())
}

#[test]
fn find_returns_all_matching_records() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(3, "Alice", 41, &[]))?;
    db.insert(&User::new(1, "Alice", 30, &[]))?;
    db.insert(&User::new(2, "Bob", 30, &[]))?;
    db.insert(&User::new(4, "Alice Smith", 30, &[]))?;

    let alices = db.find::<User>(UserIndex::Name, &key::enc("Alice"))?;
    let thirty_year_olds = db.find::<User>(UserIndex::Age, &key::enc(30u8))?;
    let carols = db.find::<User>(UserIndex::Name, &key::enc("Carol"))?;

    assert_eq!(ids(&alices), [1, 3]);
    assert_eq!(ids(&thirty_year_olds), [1, 2, 4]);
    assert_eq!(carols, []);
    Ok(())
}

#[test]
fn find_follows_an_update_of_the_field() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(1, "Alice", 30, &["admin", "staff"]))?;
    db.insert(&User::new(2, "Alice", 41, &[]))?;

    db.insert(&User::new(1, "Carol", 30, &["staff"]))?;
    let alices_after_first_update = db.find::<User>(UserIndex::Name, &key::enc("Alice"))?;
    let carols = db.find::<User>(UserIndex::Name, &key::enc("Carol"))?;
    let admins = db.find::<User>(UserIndex::Tag, &key::enc("admin"))?;
    let staff = db.find::<User>(UserIndex::Tag, &key::enc("staff"))?;
    db.insert(&User::new(2, "Dave", 41, &[]))?;
    let alices_after_second_update = db.find::<User>(UserIndex::Name, &key::enc("Alice"))?;

    assert_eq!(ids(&alices_after_first_update), [2]);
    assert_eq!(ids(&carols), [1]);
    assert_eq!(admins, []);
    assert_eq!(ids(&staff), [1]);
    assert_eq!(alices_after_second_update, []);
    Ok(())
}

#[test]
fn delete_removes_the_index_entries() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(1, "Alice", 30, &["admin"]))?;
    db.insert(&User::new(2, "Alice", 41, &["admin"]))?;

    db.delete::<User>(1)?;
    let alices = db.find::<User>(UserIndex::Name, &key::enc("Alice"))?;
    let admins = db.find::<User>(UserIndex::Tag, &key::enc("admin"))?;
    let thirty_year_olds = db.find::<User>(UserIndex::Age, &key::enc(30u8))?;
    let of_any_age = db.range::<User>(UserIndex::Age, ..)?;

    assert_eq!(ids(&alices), [2]);
    assert_eq!(ids(&admins), [2]);
    assert_eq!(thirty_year_olds, []);
    assert_eq!(ids(&of_any_age), [2]);
    Ok(())
}

#[test]
fn range_returns_the_right_subset() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(1, "Erin", 66, &[]))?;
    db.insert(&User::new(2, "Dave", 65, &[]))?;
    db.insert(&User::new(3, "Carol", 30, &[]))?;
    db.insert(&User::new(4, "Bob", 18, &[]))?;
    db.insert(&User::new(5, "Alice", 17, &[]))?;
    db.insert(&User::new(6, "Frank", 30, &[]))?;
    let (age_18, age_65) = (key::enc(18u8), key::enc(65u8));

    let inclusive = db.range::<User>(UserIndex::Age, &age_18..=&age_65)?;
    let exclusive_end = db.range::<User>(UserIndex::Age, &age_18..&age_65)?;
    let from = db.range::<User>(UserIndex::Age, &age_65..)?;
    let up_to = db.range::<User>(UserIndex::Age, ..&age_18)?;
    let everything = db.range::<User>(UserIndex::Age, ..)?;
    let reversed = db.range::<User>(UserIndex::Age, &age_65..=&age_18)?;
    let by_name = db.range::<User>(UserIndex::Name, key::enc("B")..key::enc("D"))?;

    // sorted by the index key, then by id
    assert_eq!(ids(&inclusive), [4, 3, 6, 2]);
    assert_eq!(ids(&exclusive_end), [4, 3, 6]);
    assert_eq!(ids(&from), [2, 1]);
    assert_eq!(ids(&up_to), [5]);
    assert_eq!(ids(&everything), [5, 4, 3, 6, 2, 1]);
    assert_eq!(reversed, []);
    assert_eq!(ids(&by_name), [4, 3]);
    Ok(())
}

#[test]
fn range_returns_a_record_with_several_keys_once() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(1, "Alice", 30, &["b", "d"]))?;
    db.insert(&User::new(2, "Bob", 30, &["c"]))?;
    db.insert(&User::new(3, "Carol", 30, &["a", "e"]))?;

    let tagged = db.range::<User>(UserIndex::Tag, key::enc("b")..)?;

    // each record at its smallest key within the range: b, c, e
    assert_eq!(ids(&tagged), [1, 2, 3]);
    Ok(())
}

#[test]
fn all_returns_everything() -> Result<()> {
    let (_directory, db) = temp_db();
    let users = [
        User::new(1, "Alice", 30, &["admin"]),
        User::new(2, "Bob", 18, &[]),
        User::new(300, "Carol", 65, &["staff", "admin"]),
    ];
    for user in users.iter().rev() {
        db.insert(user)?;
    }

    let everyone = db.all::<User>()?;

    assert_eq!(everyone, users);
    Ok(())
}

#[test]
fn view_reads_the_fields_of_the_archived_record() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(1, "Alice", 30, &["admin", "staff"]))?;

    let fields = db.view::<User, _>(1, |user| {
        let last_tag = user.tags.last().map(|tag| tag.to_string());

        (
            user.id.to_native(),
            user.name.to_string(),
            user.age,
            last_tag,
        )
    })?;
    let missing = db.view::<User, _>(2, |user| user.age)?;

    let expected = (1, "Alice".to_owned(), 30, Some("staff".to_owned()));
    assert_eq!(fields, Some(expected));
    assert_eq!(missing, None);
    Ok(())
}

#[test]
fn views_visit_the_records_in_the_order_of_the_queries() -> Result<()> {
    let (_directory, db) = temp_db();
    db.insert(&User::new(3, "Alice", 41, &["a", "e"]))?;
    db.insert(&User::new(2, "Bob", 30, &["c"]))?;
    db.insert(&User::new(1, "Alice", 30, &["b", "d"]))?;
    let mut alices = Vec::new();
    let mut tagged = Vec::new();
    let mut everyone = Vec::new();

    db.view_find::<User>(UserIndex::Name, &key::enc("Alice"), |user| {
        alices.push(user.id.to_native());
    })?;
    db.view_range::<User>(UserIndex::Tag, key::enc("b").., |user| {
        tagged.push(user.id.to_native());
    })?;
    db.view_all::<User>(|user| everyone.push(user.id.to_native()))?;

    assert_eq!(alices, [1, 3]);
    // each record once, at its smallest key within the range: b, c, e
    assert_eq!(tagged, [1, 2, 3]);
    assert_eq!(everyone, [1, 2, 3]);
    Ok(())
}

#[test]
fn data_survives_reopening() -> Result<()> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let alice = User::new(1, "Alice", 30, &["admin"]);
    let db = Database::open(&path)?;
    db.insert(&alice)?;
    drop(db);

    let reopened = Database::open(&path)?;
    let by_id = reopened.get::<User>(1)?;
    let admins = reopened.find::<User>(UserIndex::Tag, &key::enc("admin"))?;

    assert_eq!(by_id.as_ref(), Some(&alice));
    assert_eq!(admins, [alice]);
    Ok(())
}

#[test]
fn insert_that_fails_on_an_index_stores_nothing() -> Result<()> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let database = redb::Database::create(&path)?;
    let transaction = database.begin_write()?;
    transaction.open_table(TableDefinition::<u64, &[u8]>::new("users.name"))?;
    transaction.commit()?;
    drop(database);
    let db = Database::open(&path)?;

    let error = db.insert(&User::new(1, "Alice", 30, &[])).unwrap_err();
    let stored = db.get::<User>(1)?;

    assert!(matches!(error, Error::Table(_)), "{error:?}");
    // the row is written before its index entries, the failed transaction has to take it back
    assert_eq!(stored, None);
    Ok(())
}

#[test]
fn corrupt_row_is_an_error() -> Result<()> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let db = Database::open(&path)?;
    db.insert(&User::new(1, "Alice", 30, &[]))?;
    drop(db);
    let database = redb::Database::create(&path)?;
    let transaction = database.begin_write()?;
    {
        let mut rows = transaction.open_table(USER_ROWS)?;
        rows.insert(1, [0xFF; 7].as_slice())?;
    }
    transaction.commit()?;
    drop(database);
    let db = Database::open(&path)?;

    let results = [
        db.get::<User>(1).map(drop),
        db.all::<User>().map(drop),
        db.find::<User>(UserIndex::Name, &key::enc("Alice"))
            .map(drop),
        db.insert(&User::new(1, "Bob", 18, &[])),
        db.delete::<User>(1).map(drop),
        // a view reads the stored bytes in place, it has to validate them like a deserializing read
        db.view::<User, _>(1, |_user| ()).map(drop),
        db.view_all::<User>(|_user| ()),
        db.view_find::<User>(UserIndex::Name, &key::enc("Alice"), |_user| ()),
    ];

    for result in results {
        let error = result.unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "{error:?}");
        assert_eq!(error.to_string(), "corrupt data in users id 1");
    }
    Ok(())
}

#[test]
fn index_entry_without_a_row_is_an_error() -> Result<()> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let db = Database::open(&path)?;
    db.insert(&User::new(1, "Alice", 30, &[]))?;
    drop(db);
    let database = redb::Database::create(&path)?;
    let transaction = database.begin_write()?;
    {
        let mut rows = transaction.open_table(USER_ROWS)?;
        rows.remove(1)?;
    }
    transaction.commit()?;
    drop(database);
    let db = Database::open(&path)?;

    let error = db
        .find::<User>(UserIndex::Name, &key::enc("Alice"))
        .unwrap_err();

    assert!(matches!(error, Error::Corrupt { .. }), "{error:?}");
    assert_eq!(error.to_string(), "corrupt data in users id 1");
    Ok(())
}

#[test]
fn index_entries_are_stored_in_a_multimap_per_index() -> Result<()> {
    // the layout of the file is a contract, a change of it orphans the existing databases
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let db = Database::open(&path)?;
    db.insert(&User::new(7, "Alice", 30, &[]))?;
    db.insert(&User::new(9, "Alice", 41, &[]))?;
    drop(db);
    let by_name = MultimapTableDefinition::<&[u8], u64>::new("users.name");

    let database = redb::Database::create(&path)?;
    let entries = database.begin_read()?.open_multimap_table(by_name)?;
    let mut ids_of_alice = Vec::new();
    for id in entries.get(key::enc("Alice").as_slice())? {
        ids_of_alice.push(id?.value());
    }

    assert_eq!(ids_of_alice, [7, 9]);
    Ok(())
}

#[test]
fn rows_are_stored_as_unaligned_rkyv_bytes() -> Result<()> {
    // the layout of the file is a contract, and a view reads a row in place at any offset of a page
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let db = Database::open(&path)?;
    db.insert(&User::new(1, "Alice", 30, &["admin"]))?;
    drop(db);

    let database = redb::Database::create(&path)?;
    let rows = database.begin_read()?.open_table(USER_ROWS)?;
    let row = rows.get(1)?.unwrap();

    #[rustfmt::skip]
    let expected = [
        // the tag "admin", stored inline
        0x61, 0x64, 0x6D, 0x69, 0x6E, 0xFF, 0xFF, 0xFF,
        // id
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // the name "Alice", stored inline
        0x41, 0x6C, 0x69, 0x63, 0x65, 0xFF, 0xFF, 0xFF,
        // age, with no padding after it
        0x1E,
        // tags: the relative offset of the first one, then their count
        0xE7, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00,
    ];
    assert_eq!(row.value(), expected);
    Ok(())
}
