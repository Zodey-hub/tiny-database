# tiny-database

A small embedded record store built on `redb` (storage) and `rkyv` (serialization).
It keeps typed records under a `u64` id in one file, with secondary indexes for equality, range and string prefix lookups.
A query language, joins, migrations and unique constraints are left to the caller.

## Example

A `User` with an index on each field, and a composite index on both.
The key of a composite index is the concatenation of the encoded fields: it sorts by the first field, and by the second one within it, so one `range` answers "equal on the first field, within a range on the second".

Every read comes in two forms:

- `get`, `find`, `range` and `all` deserialize every row into an owned record.
- Their `view` counterparts (`view`, `view_find`, `view_range`, `view_all`) validate the stored bytes and pass the archived record (`ArchivedUser`) to a closure instead, so a field can be read without allocating the others. The archived record cannot leave the closure: a string of it reads as a `&str`, an integer wider than a byte converts with `to_native()`.

`count` returns the number of records of a type without reading any of them.

The rows are read in place, with no copy: the crate enables the `unaligned` feature of rkyv for this, which applies to every use of rkyv in the program.

```rust,no_run
use tiny_database::rkyv::{Archive, Deserialize, Serialize};
use tiny_database::{Database, Index, Record, key};

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq)]
#[rkyv(crate = tiny_database::rkyv)]
struct User {
    id: u64,
    name: String,
    age: u8,
}

enum UserIndex {
    Name,
    Age,
    NameAge,
}

impl Index for UserIndex {
    fn name(&self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Age => "age",
            Self::NameAge => "name_age",
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
        vec![
            (UserIndex::Name, key::enc(&self.name)),
            (UserIndex::Age, key::enc(self.age)),
            (UserIndex::NameAge, name_age_key(&self.name, self.age)),
        ]
    }
}

/// Returns the key of a name and an age in the `NameAge` index.
fn name_age_key(name: &str, age: u8) -> Vec<u8> {
    [key::enc(name), key::enc(age)].concat()
}

fn main() -> tiny_database::Result<()> {
    let db = Database::open("app.redb")?;

    db.insert(&User { id: 1, name: "Alice".to_owned(), age: 30 })?;
    db.insert(&User { id: 2, name: "Alice".to_owned(), age: 65 })?;
    db.insert(&User { id: 3, name: "Bob".to_owned(), age: 25 })?;

    // deserialized: by id
    let first = db.get::<User>(1)?;

    // deserialized: equality on one field
    let alices = db.find::<User>(UserIndex::Name, &key::enc("Alice"))?;

    // deserialized: a range of one field
    let young = db.range::<User>(UserIndex::Age, key::enc(18u8)..=key::enc(30u8))?;

    // deserialized: the names that start with a prefix
    let als = db.range::<User>(UserIndex::Name, key::prefix_range("Al"))?;

    // deserialized: equality on the first field of the composite index, a range on the second one
    let young_alices = name_age_key("Alice", 18)..=name_age_key("Alice", 30);
    let young_alices = db.range::<User>(UserIndex::NameAge, young_alices)?;

    // zero copy: only the name is copied out of the row
    let first_name = db.view::<User, _>(1, |user| user.name.to_string())?;

    // zero copy: the ids of the old users, without building any of them
    let mut old_ids = Vec::new();
    db.view_range::<User>(UserIndex::Age, key::enc(60u8).., |user| {
        old_ids.push(user.id.to_native());
    })?;

    assert_eq!(first.map(|user| user.age), Some(30));
    assert_eq!(alices.len(), 2);
    assert_eq!(young.len(), 2);
    assert_eq!(als.len(), 2);
    assert_eq!(young_alices.len(), 1);
    assert_eq!(first_name.as_deref(), Some("Alice"));
    assert_eq!(old_ids, [2]);

    Ok(())
}
```

The `NameAge` index alone also answers "equal on the name": ask for the whole range of the age, `name_age_key("Alice", u8::MIN)..=name_age_key("Alice", u8::MAX)`.
