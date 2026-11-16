//! Lossless configuration edits. Status comments are output, never reader policy.

use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use toml::Value as Data;
use toml_edit::{Decor, DocumentMut, Item, TableLike, Value};

/// The reader declares any deprecated paths in code. Such fields must still be
/// deserialized by this type; declaring a path does not enable an old alias.
/// `*` denotes an array element. No status is stored in configuration values.
pub(super) trait ReadConfig: Serialize + DeserializeOwned {
    const DEPRECATED_FIELDS: &'static [&'static [&'static str]] = &[];
}

pub(super) struct ConfigDocument {
    path: PathBuf,
    source: String,
    document: DocumentMut,
}

impl ConfigDocument {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read configuration {}: {error}", path.display()))?;
        let document = source
            .parse()
            .map_err(|error| format!("invalid configuration {}: {error}", path.display()))?;
        Ok(Self {
            path,
            source,
            document,
        })
    }

    pub fn read<T: ReadConfig>(&self) -> Result<T, String> {
        toml::from_str(&self.document.to_string())
            .map_err(|error| format!("invalid configuration {}: {error}", self.path.display()))
    }

    /// Compare consumed values, not the original document: unknown fields must
    /// never be mistaken for removals, and unchanged defaults are not inserted.
    pub fn update<T: ReadConfig>(&mut self, next: &T, field: Option<&str>) -> Result<(), String> {
        let previous: T = self.read()?;
        let before = data(&previous)?;
        let after = data(next)?;
        let original: Data =
            toml::from_str(&self.document.to_string()).map_err(|error| error.to_string())?;
        let template = toml_edit::ser::to_document(next).map_err(|error| error.to_string())?;
        if let Some(field) = field {
            let desired = after
                .get(field)
                .ok_or_else(|| format!("unknown writable field `{field}`"))?;
            if before.get(field) == Some(desired) {
                return Ok(());
            }
            let replacement = template
                .get(field)
                .ok_or_else(|| format!("missing serialized field `{field}`"))?;
            if let (Some(raw), Some(original), Some(previous)) = (
                self.document.get_mut(field),
                original.get(field),
                before.get(field),
            ) {
                merge_item(raw, original, previous, desired, replacement)
            } else {
                self.document.insert(field, replacement.clone());
                Ok(())
            }
        } else {
            merge_item(
                self.document.as_item_mut(),
                &original,
                &before,
                &after,
                template.as_item(),
            )
        }
    }

    /// Only annotations are added on reads. Values, including missing fields
    /// supplied by a reader's default, are never copied from the typed projection.
    pub fn save<T: ReadConfig>(mut self) -> Result<(), String> {
        let consumed: T = self.read()?;
        let fields = data(&consumed)?;
        annotate_item(
            self.document.as_item_mut(),
            Some(&fields),
            &mut Vec::new(),
            T::DEPRECATED_FIELDS,
        );
        let content = self.document.to_string();
        if content == self.source {
            return Ok(());
        }
        // Annotation must remain valid for inline tables and dotted keys too.
        toml::from_str::<Data>(&content).map_err(|error| {
            format!(
                "cannot format configuration {}: {error}",
                self.path.display()
            )
        })?;
        replace_file(&self.path, &self.source, &content)
            .map_err(|error| format!("cannot save configuration {}: {error}", self.path.display()))
    }
}

fn data<T: Serialize>(value: &T) -> Result<Data, String> {
    Data::try_from(value).map_err(|error| error.to_string())
}

fn annotate_item(
    item: &mut Item,
    fields: Option<&Data>,
    path: &mut Vec<String>,
    deprecated: &[&[&str]],
) {
    if let Some(table) = item.as_table_like_mut() {
        annotate_table(table, fields.and_then(Data::as_table), path, deprecated);
    } else if let Some(array) = item.as_array_of_tables_mut() {
        for (index, table) in array.iter_mut().enumerate() {
            let fields = fields
                .and_then(Data::as_array)
                .and_then(|items| items.get(index));
            path.push("*".into());
            annotate_table(table, fields.and_then(Data::as_table), path, deprecated);
            path.pop();
        }
    } else if let Some(value) = item.as_value_mut() {
        annotate_value(value, fields, path, deprecated);
    }
}

fn annotate_value(
    value: &mut Value,
    fields: Option<&Data>,
    path: &mut Vec<String>,
    deprecated: &[&[&str]],
) {
    match value {
        Value::InlineTable(table) => {
            annotate_table(table, fields.and_then(Data::as_table), path, deprecated)
        }
        Value::Array(array) => {
            for (index, value) in array.iter_mut().enumerate() {
                let fields = fields
                    .and_then(Data::as_array)
                    .and_then(|items| items.get(index));
                path.push("*".into());
                annotate_value(value, fields, path, deprecated);
                path.pop();
            }
        }
        _ => {}
    }
}

fn annotate_table(
    table: &mut dyn TableLike,
    fields: Option<&toml::Table>,
    path: &mut Vec<String>,
    deprecated: &[&[&str]],
) {
    for (mut key, item) in table.iter_mut() {
        let fields = fields.and_then(|fields| fields.get(key.get()));
        path.push(key.get().to_string());
        let marker = if fields.is_none() {
            Some("# invalid")
        } else if deprecated.iter().any(|prefix| {
            prefix.len() <= path.len() && prefix.iter().zip(path.iter()).all(|(a, b)| *a == b)
        }) {
            Some("# deprecated")
        } else {
            None
        };
        // Table headers have their own decor; key decor is used for assignments.
        if let Some(table) = item.as_table_mut() {
            set_marker(table.decor_mut(), marker);
        } else if let Some(array) = item.as_array_of_tables_mut() {
            for table in array.iter_mut() {
                set_marker(table.decor_mut(), marker);
            }
        } else {
            set_marker(key.leaf_decor_mut(), marker);
        }
        annotate_item(item, fields, path, deprecated);
        path.pop();
    }
}

fn set_marker(decor: &mut Decor, marker: Option<&str>) {
    let prefix = decor.prefix().and_then(|text| text.as_str()).unwrap_or("");
    // Exact standalone status lines are refreshed only as explanatory output.
    // Other comments and whitespace stay intact; comments never enable a field.
    let mut clean = String::new();
    for line in prefix.split_inclusive('\n') {
        if !matches!(line.trim(), "# invalid" | "# deprecated") {
            clean.push_str(line);
        }
    }
    if let Some(marker) = marker {
        let indent = clean.len() - clean.trim_end_matches([' ', '\t']).len();
        let tail = clean.split_off(clean.len() - indent);
        if !clean.is_empty() && !clean.ends_with('\n') {
            clean.push('\n');
        }
        clean.push_str(marker);
        clean.push('\n');
        clean.push_str(&tail);
    }
    if clean != prefix {
        decor.set_prefix(clean);
    }
}

fn merge_item(
    raw: &mut Item,
    original: &Data,
    before: &Data,
    after: &Data,
    template: &Item,
) -> Result<(), String> {
    if before == after {
        return Ok(());
    }
    if let (Some(table), Some(original), Some(before), Some(after), Some(template)) = (
        raw.as_table_like_mut(),
        original.as_table(),
        before.as_table(),
        after.as_table(),
        template.as_table_like(),
    ) {
        for key in before.keys() {
            if !after.contains_key(key) {
                table.remove(key);
            }
        }
        for (key, value) in after {
            if before.get(key) == Some(value) {
                continue;
            }
            let replacement = template
                .get(key)
                .ok_or_else(|| format!("missing serialized field `{key}`"))?;
            if let (Some(current), Some(original), Some(previous)) =
                (table.get_mut(key), original.get(key), before.get(key))
            {
                merge_item(current, original, previous, value, replacement)?;
            } else {
                let mut replacement = replacement.clone();
                if table.is_dotted() {
                    replacement.make_value();
                }
                table.insert(key, replacement);
            }
        }
        // An empty implicit map must still be emitted so required empty maps
        // remain distinguishable from missing fields after deleting their last entry.
        if let Some(table) = raw.as_table_mut() {
            if table.is_empty() {
                table.set_implicit(false);
            }
        }
        return Ok(());
    }
    if let (Some(original), Some(before), Some(after)) =
        (original.as_array(), before.as_array(), after.as_array())
    {
        let originals = sequence_items(raw)?;
        let replacements = sequence_items(template)?;
        // Binding replacement keeps the same vector index. Preserve that
        // entry's unknown fields instead of treating it as removal plus insertion.
        // Scalar sets use value identity because their reader may sort them.
        if before.len() == after.len() && before.iter().all(Data::is_table) {
            let mut entries = originals;
            for (index, entry) in entries.iter_mut().enumerate() {
                merge_item(
                    entry,
                    &original[index],
                    &before[index],
                    &after[index],
                    &replacements[index],
                )?;
            }
            return replace_sequence(raw, entries);
        }
        let mut used = vec![false; originals.len()];
        let mut entries = Vec::with_capacity(after.len());
        for (index, next) in after.iter().enumerate() {
            // Arrays edited here are binding edges or function sets. Their
            // consumed value is their identity; server edits use explicit indices.
            let schema = before.first().unwrap_or(next);
            let found = original
                .iter()
                .enumerate()
                .position(|(i, value)| !used[i] && project(value, schema) == *next);
            if let Some(found) = found {
                used[found] = true;
                entries.push(originals[found].clone());
            } else {
                entries.push(replacements[index].clone());
            }
        }
        return replace_sequence(raw, entries);
    }
    let mut replacement = template.clone();
    if let (Some(raw), Some(next)) = (raw.as_value(), replacement.as_value_mut()) {
        *next.decor_mut() = raw.decor().clone();
    }
    *raw = replacement;
    Ok(())
}

fn project(raw: &Data, fields: &Data) -> Data {
    match (raw, fields) {
        (Data::Table(raw), Data::Table(fields)) => Data::Table(
            fields
                .iter()
                .filter_map(|(key, shape)| {
                    raw.get(key)
                        .map(|value| (key.clone(), project(value, shape)))
                })
                .collect(),
        ),
        _ => raw.clone(),
    }
}

fn sequence_items(item: &Item) -> Result<Vec<Item>, String> {
    if let Some(array) = item.as_array_of_tables() {
        Ok(array.iter().cloned().map(Item::Table).collect())
    } else if let Some(array) = item.as_array() {
        Ok(array.iter().cloned().map(Item::Value).collect())
    } else {
        Err("expected a configuration array".into())
    }
}

fn replace_sequence(item: &mut Item, entries: Vec<Item>) -> Result<(), String> {
    if entries.is_empty() && item.is_array_of_tables() {
        *item = Item::Value(Value::Array(toml_edit::Array::new()));
        return Ok(());
    }
    if let Some(array) = item.as_array_of_tables_mut() {
        array.clear();
        for entry in entries {
            array.push(
                entry
                    .into_table()
                    .map_err(|_| "expected a table array entry".to_string())?,
            );
        }
    } else if let Some(array) = item.as_array_mut() {
        array.clear();
        for entry in entries {
            array.push_formatted(
                entry
                    .into_value()
                    .map_err(|_| "expected an array value".to_string())?,
            );
        }
    } else {
        return Err("expected a configuration array".into());
    }
    Ok(())
}

fn replace_file(path: &Path, expected: &str, content: &str) -> std::io::Result<()> {
    let staging = path.with_extension("toml.pending");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    let result = (|| {
        // All program writers take the same exclusive staging slot. A document
        // read before another writer committed must not overwrite newer values.
        if std::fs::read_to_string(path)? != expected {
            return Err(std::io::Error::other(
                "configuration changed while being read; reload before saving",
            ));
        }
        file.write_all(content.as_bytes())?;
        file.sync_all()
    })();
    drop(file); // Windows requires closed handles before rename or removal.
    let result = result.and_then(|()| std::fs::rename(&staging, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}
