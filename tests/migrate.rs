//! The conversion of the stored records from one record type to another.

use redb::TableDefinition;
use rkyv::{Archive, Deserialize, Serialize};
use tempfile::TempDir;
use tiny_database::{Database, Error, Index, Record, Result, key};

/// The first version of a user: its name is indexed as it is.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
struct UserV1 {
    id: u64,
    name: String,
}

impl UserV1 {
    fn new(id: u64, name: &str) -> Self {
        Self {
            id,
            name: name.to_owned(),
        }
    }
}

/// The second version of a user: a new field, and its name is indexed in lower case.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
struct UserV2 {
    id: u64,
    name: String,
    email: Option<String>,
}

/// The same user as [`UserV2`], in another table.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Member {
    id: u64,
    name: String,
}

enum UserIndex {
    Name,
}

impl Index for UserIndex {
    fn name(&self) -> &'static str {
        match self {
            Self::Name => "name",
        }
    }
}

impl Record for UserV1 {
    const TABLE: &'static str = "users";
    type Index = UserIndex;

    fn id(&self) -> u64 {
        self.id
    }

    fn index_keys(&self) -> Vec<(UserIndex, Vec<u8>)> {
        vec![(UserIndex::Name, key::enc(&self.name))]
    }
}

impl Record for UserV2 {
    const TABLE: &'static str = "users";
    type Index = UserIndex;

    fn id(&self) -> u64 {
        self.id
    }

    fn index_keys(&self) -> Vec<(UserIndex, Vec<u8>)> {
        vec![(UserIndex::Name, key::enc(self.name.to_lowercase()))]
    }
}

impl Record for Member {
    const TABLE: &'static str = "members";
    type Index = UserIndex;

    fn id(&self) -> u64 {
        self.id
    }

    fn index_keys(&self) -> Vec<(UserIndex, Vec<u8>)> {
        vec![(UserIndex::Name, key::enc(&self.name))]
    }
}

/// Opens a new database with the users `Alice` (1) and `Bob` (2) of the first version; the directory is deleted when the returned guard is dropped.
fn db_with_old_users() -> Result<(TempDir, Database)> {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open(directory.path().join("test.redb"))?;
    let users = [UserV1::new(1, "Alice"), UserV1::new(2, "Bob")];
    db.insert_many(&users)?;

    Ok((directory, db))
}

/// The second version of `user`, without an email.
fn to_v2(user: UserV1) -> UserV2 {
    UserV2 {
        id: user.id,
        name: user.name,
        email: None,
    }
}

#[test]
fn migrate_converts_every_record_and_its_index_entries() -> Result<()> {
    let (_directory, db) = db_with_old_users()?;

    db.migrate::<UserV1, UserV2>(to_v2)?;
    let everyone = db.all::<UserV2>()?;
    let by_new_key = db.find::<UserV2>(UserIndex::Name, &key::enc("alice"))?;
    let by_old_key = db.find::<UserV2>(UserIndex::Name, &key::enc("Alice"))?;

    assert_eq!(
        everyone,
        [to_v2(UserV1::new(1, "Alice")), to_v2(UserV1::new(2, "Bob"))]
    );
    assert_eq!(by_new_key, [to_v2(UserV1::new(1, "Alice"))]);
    assert_eq!(by_old_key, [], "the old index entries are removed");
    Ok(())
}

#[test]
fn migrate_that_fails_on_a_later_record_converts_none() -> Result<()> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.redb");
    let db = Database::open(&path)?;
    db.insert(&UserV1::new(1, "Alice"))?;
    drop(db);
    let database = redb::Database::create(&path)?;
    let transaction = database.begin_write()?;
    {
        let mut rows = transaction.open_table(TableDefinition::<u64, &[u8]>::new("users"))?;
        rows.insert(2, [0xFF; 3].as_slice())?;
    }
    transaction.commit()?;
    drop(database);
    let db = Database::open(&path)?;

    let error = db.migrate::<UserV1, UserV2>(to_v2).unwrap_err();
    let alice = db.get::<UserV1>(1)?;
    let alices = db.find::<UserV1>(UserIndex::Name, &key::enc("Alice"))?;

    assert!(matches!(error, Error::Corrupt { id: 2, .. }), "{error:?}");
    assert_eq!(alice, Some(UserV1::new(1, "Alice")));
    assert_eq!(alices, [UserV1::new(1, "Alice")]);
    Ok(())
}

#[test]
fn migrate_to_another_table_moves_the_records() -> Result<()> {
    let (_directory, db) = db_with_old_users()?;

    db.migrate::<UserV1, Member>(|user| Member {
        id: user.id,
        name: user.name,
    })?;
    let members = db.find::<Member>(UserIndex::Name, &key::enc("Bob"))?;
    let old_users = db.count::<UserV1>()?;
    let old_bobs = db.find::<UserV1>(UserIndex::Name, &key::enc("Bob"))?;

    assert_eq!(ids(&members), [2]);
    assert_eq!(old_users, 0);
    assert_eq!(old_bobs, []);
    Ok(())
}

fn ids(members: &[Member]) -> Vec<u64> {
    members.iter().map(|member| member.id).collect()
}
