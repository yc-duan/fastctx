//! Streaming adapters for legacy collections. A growing JSON map is never
//! decoded into one in-memory object just to migrate its independent entries.

use super::*;
use serde::de::{self, Deserializer as _, MapAccess, Visitor};
use std::{marker::PhantomData, path::Path};

pub(crate) fn map<T: DeserializeOwned + Serialize>(
    directory: &ConversationDir,
    family: &'static str,
    path: &Path,
) -> io::Result<()> {
    ensure_file(directory, family, path.to_path_buf(), move |path, push| {
        let file = match fastctx_platform::disk::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        struct Entries<'a, T> {
            path: &'a Path,
            push: &'a mut dyn FnMut(Source) -> io::Result<()>,
            value: PhantomData<T>,
        }
        impl<'de, T: DeserializeOwned + Serialize> Visitor<'de> for Entries<'_, T> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of independent records")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    let row = Source::new(
                        key,
                        self.path.to_path_buf(),
                        serde_json::to_vec(&value).map_err(de::Error::custom)?,
                    )
                    .map_err(de::Error::custom)?;
                    (self.push)(row).map_err(de::Error::custom)?;
                }
                Ok(())
            }
        }
        serde_json::Deserializer::from_reader(io::BufReader::new(file))
            .deserialize_map(Entries::<T> { path, push, value: PhantomData })
            .map_err(io::Error::other)
    })
}

pub(crate) fn map_field<T: DeserializeOwned + Serialize>(
    directory: &ConversationDir,
    family: &'static str,
    path: &Path,
    field: &str,
) -> io::Result<()> {
    use serde::de::DeserializeSeed;
    let field = field.to_string();
    ensure_file(directory, family, path.to_path_buf(), move |path, push| {
        let file = match fastctx_platform::disk::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        struct Entries<'a, T> {
            path: &'a Path,
            push: &'a mut dyn FnMut(Source) -> io::Result<()>,
            marker: PhantomData<T>,
        }
        impl<'de, T: DeserializeOwned + Serialize> DeserializeSeed<'de> for Entries<'_, T> {
            type Value = ();
            fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> std::result::Result<(), D::Error> {
                deserializer.deserialize_map(self)
            }
        }
        impl<'de, T: DeserializeOwned + Serialize> Visitor<'de> for Entries<'_, T> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("independent map entries")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut rows: A) -> std::result::Result<(), A::Error> {
                while let Some((key, value)) = rows.next_entry::<String, T>()? {
                    let row = Source::new(
                        key,
                        self.path.to_path_buf(),
                        serde_json::to_vec(&value).map_err(de::Error::custom)?,
                    )
                    .map_err(de::Error::custom)?;
                    (self.push)(row).map_err(de::Error::custom)?;
                }
                Ok(())
            }
        }
        struct Field<'a, T> {
            field: &'a str,
            entries: Entries<'a, T>,
        }
        impl<'de, T: DeserializeOwned + Serialize> Visitor<'de> for Field<'_, T> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a legacy record with a map field")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut rows: A) -> std::result::Result<(), A::Error> {
                let mut entries = Some(self.entries);
                while let Some(key) = rows.next_key::<String>()? {
                    if key == self.field {
                        rows.next_value_seed(
                            entries.take().ok_or_else(|| de::Error::custom("duplicate legacy map field"))?,
                        )?;
                    } else {
                        rows.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(())
            }
        }
        serde_json::Deserializer::from_reader(io::BufReader::new(file))
            .deserialize_map(Field { field: &field, entries: Entries::<T> { path, push, marker: PhantomData } })
            .map_err(io::Error::other)
    })
}

pub(crate) fn field<T: DeserializeOwned>(path: &Path, field: &str) -> io::Result<Option<T>> {
    let file = match fastctx_platform::disk::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    struct Field<'a, T> {
        field: &'a str,
        marker: PhantomData<T>,
    }
    impl<'de, T: DeserializeOwned> Visitor<'de> for Field<'_, T> {
        type Value = Option<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a legacy object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut rows: A) -> std::result::Result<Self::Value, A::Error> {
            let mut value = None;
            while let Some(key) = rows.next_key::<String>()? {
                if key == self.field {
                    if value.is_some() {
                        return Err(de::Error::custom("duplicate legacy field"));
                    }
                    value = Some(rows.next_value()?);
                } else {
                    rows.next_value::<de::IgnoredAny>()?;
                }
            }
            Ok(value)
        }
    }
    serde_json::Deserializer::from_reader(io::BufReader::new(file))
        .deserialize_map(Field::<T> { field, marker: PhantomData })
        .map_err(io::Error::other)
}

pub(crate) fn directory<T: DeserializeOwned + Serialize>(
    directory: &ConversationDir,
    family: &'static str,
    folder: &Path,
) -> io::Result<()> {
    let folder = folder.to_path_buf();
    ensure_each(directory, family, move |push| {
        let entries = match fastctx_platform::disk::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json")
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let key = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| crate::record_migration::errors::invalid("invalid legacy record name"))?
                    .to_string();
                let value: T = serde_json::from_slice(&fastctx_platform::disk::read(&path)?)?;
                push(Source::new(key, path.clone(), serde_json::to_vec(&value)?)?)
            })?;
        }
        Ok(())
    })
}
pub(crate) fn single<T: DeserializeOwned + Serialize>(
    directory: &ConversationDir,
    family: &'static str,
    path: &Path,
) -> io::Result<()> {
    ensure_file(directory, family, path.to_path_buf(), |path, push| match fastctx_platform::disk::read(path) {
        Ok(bytes) => {
            let value: T = serde_json::from_slice(&bytes)?;
            push(Source::new("current".into(), path.to_path_buf(), serde_json::to_vec(&value)?)?)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    })
}

/// Streams either a top-level array or one named array field without retaining
/// the enclosing object. Unknown fields are skipped by the decoder.
pub(crate) fn array<T: DeserializeOwned>(
    path: &Path,
    field: Option<&str>,
    push: &mut dyn FnMut(T) -> io::Result<()>,
) -> io::Result<()> {
    use serde::de::{DeserializeSeed, SeqAccess};
    let file = match fastctx_platform::disk::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    struct Elements<'a, T> {
        push: &'a mut dyn FnMut(T) -> io::Result<()>,
    }
    impl<'de, T: DeserializeOwned> DeserializeSeed<'de> for Elements<'_, T> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
            d.deserialize_seq(self)
        }
    }
    impl<'de, T: DeserializeOwned> Visitor<'de> for Elements<'_, T> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an array of records")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> std::result::Result<(), A::Error> {
            while let Some(value) = values.next_element::<T>()? {
                (self.push)(value).map_err(de::Error::custom)?;
            }
            Ok(())
        }
    }
    struct Field<'a, T> {
        name: &'a str,
        elements: Elements<'a, T>,
    }
    impl<'de, T: DeserializeOwned> Visitor<'de> for Field<'_, T> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an object with an array field")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
            let mut elements = Some(self.elements);
            while let Some(key) = map.next_key::<String>()? {
                if key == self.name {
                    map.next_value_seed(elements.take().ok_or_else(|| de::Error::custom("duplicate array field"))?)?;
                } else {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }
    let mut decoder = serde_json::Deserializer::from_reader(io::BufReader::new(file));
    if let Some(name) = field {
        decoder.deserialize_map(Field { name, elements: Elements { push } })
    } else {
        decoder.deserialize_seq(Elements { push })
    }
    .map_err(io::Error::other)?;
    decoder.end().map_err(io::Error::other)
}
