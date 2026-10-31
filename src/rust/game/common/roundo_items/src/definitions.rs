//! Immutable Mod-owned item definitions; loading does not create item instances.

use crate::DeclaredItemData;
use roundo_mod_loader::{LoadedMods, ModId, ResourceName};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    schema_version: u32,
    #[serde(default)]
    item: Vec<RegistryItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryItem {
    name: String,
    data: toml::Table,
}

/// Immutable initial-value definitions indexed by canonical Mod resource names.
///
/// Definition names are not item IDs, typed store schemas or access grants.
/// Business values are retained without executing strings or resolving references.
#[derive(Clone, Debug)]
pub struct ItemDefinitions {
    entries: BTreeMap<ResourceName, toml::Table>,
}

impl ItemDefinitions {
    /// Creates an explicitly empty definition directory without filesystem access.
    pub fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Loads each discovered Mod's optional `game_data/item/registry.toml`.
    ///
    /// Only a metadata `NotFound` means no definitions. A present non-file or
    /// any other metadata/read error is fatal. The candidate directory is
    /// returned only after every Mod succeeds; this performs no Mod discovery,
    /// override arbitration, runtime installation or identity issuance.
    pub fn load(mods: &LoadedMods) -> Result<Self, ItemDefinitionLoadError> {
        let mut definitions = Self::empty();
        for loaded in mods.iter() {
            let path = loaded.root.join("game_data/item/registry.toml");
            let io_error = |error| ItemDefinitionLoadError {
                owner: loaded.id.clone(),
                path: path.clone(),
                kind: ItemDefinitionLoadErrorKind::Io(error),
            };
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(io_error(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "item registry is not a regular file",
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(io_error(error)),
            }
            // Even NotFound here is fatal: the metadata check found a file.
            let source = fs::read_to_string(&path).map_err(io_error)?;
            let candidate = Self::from_registry(&loaded.id, &path, &source)?;
            definitions.entries.extend(candidate.entries);
        }
        Ok(definitions)
    }

    /// Parses one registry from memory. `path` is diagnostic context, not read.
    ///
    /// Version/name/data are required, except the declaration list may be
    /// omitted. Both structural levels deny unknown fields; business data does
    /// not. Only schema version 1 is supported. No partial directory is returned
    /// on error, and declaration indices in errors start at 1.
    pub fn from_registry(
        owner: &ModId,
        path: impl AsRef<Path>,
        source: &str,
    ) -> Result<Self, ItemDefinitionLoadError> {
        let error = |kind| ItemDefinitionLoadError {
            owner: owner.clone(),
            path: path.as_ref().to_path_buf(),
            kind,
        };
        let registry: RegistryFile = toml::from_str(source)
            .map_err(|parse| error(ItemDefinitionLoadErrorKind::Parse(parse)))?;
        if registry.schema_version != 1 {
            return Err(error(
                ItemDefinitionLoadErrorKind::UnsupportedSchemaVersion {
                    found: registry.schema_version,
                },
            ));
        }
        let mut entries = BTreeMap::new();
        let mut first_declarations = BTreeMap::new();
        for (index, item) in registry.item.into_iter().enumerate() {
            let declaration = index + 1;
            let identity = ResourceName::new(owner.clone(), item.name.clone()).map_err(|_| {
                error(ItemDefinitionLoadErrorKind::InvalidLocalName {
                    declaration,
                    name: item.name.clone(),
                })
            })?;
            if let Some(first_declaration) = first_declarations.get(&item.name) {
                return Err(error(ItemDefinitionLoadErrorKind::DuplicateLocalName {
                    declaration,
                    first_declaration: *first_declaration,
                    name: item.name,
                }));
            }
            first_declarations.insert(item.name, declaration);
            entries.insert(identity, item.data);
        }
        Ok(Self { entries })
    }

    /// Number of definitions, not item instances.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the directory contains no definitions.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Reads an exact canonical definition without searching or mutating it.
    pub fn get(&self, name: &ResourceName) -> Option<&toml::Table> {
        self.entries.get(name)
    }

    /// Clones one known definition into an independent owned native payload.
    ///
    /// Performs no I/O, identity issuance, runtime publication or authorization.
    pub fn materialize(
        &self,
        name: &ResourceName,
    ) -> Result<DeclaredItemData, UnknownItemDefinition> {
        let data = self
            .get(name)
            .ok_or_else(|| UnknownItemDefinition { name: name.clone() })?;
        Ok(DeclaredItemData::from_definition(
            name.clone(),
            data.clone(),
        ))
    }

    /// Iterates definitions in canonical resource-name order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&ResourceName, &toml::Table)> {
        self.entries.iter()
    }
}

/// Materialization requested a name absent from the definition directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownItemDefinition {
    pub name: ResourceName,
}

impl fmt::Display for UnknownItemDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown item definition: {}", self.name)
    }
}

impl Error for UnknownItemDefinition {}

/// A registry failure with its canonical Mod owner and diagnostic source path.
#[derive(Debug)]
pub struct ItemDefinitionLoadError {
    /// Mod whose registry failed.
    pub owner: ModId,
    /// Registry path, also supplied for pure in-memory parsing.
    pub path: PathBuf,
    /// Underlying I/O, TOML or declaration failure.
    pub kind: ItemDefinitionLoadErrorKind,
}

/// The precise cause of a definition load failure.
#[derive(Debug)]
pub enum ItemDefinitionLoadErrorKind {
    /// Metadata/read failure, including a present non-file or invalid UTF-8.
    Io(io::Error),
    /// TOML syntax/structure failure retaining the parser's original location.
    Parse(toml::de::Error),
    /// Only schema version 1 is understood.
    UnsupportedSchemaVersion { found: u32 },
    /// Invalid local resource name at a 1-based declaration index.
    InvalidLocalName { declaration: usize, name: String },
    /// Repeated local name, with both 1-based declaration indices.
    DuplicateLocalName {
        declaration: usize,
        first_declaration: usize,
        name: String,
    },
}

impl fmt::Display for ItemDefinitionLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}): ", self.owner, self.path.display())?;
        match &self.kind {
            ItemDefinitionLoadErrorKind::Io(error) => write!(formatter, "{error}"),
            ItemDefinitionLoadErrorKind::Parse(error) => write!(formatter, "{error}"),
            ItemDefinitionLoadErrorKind::UnsupportedSchemaVersion { found } => {
                write!(formatter, "unsupported schema_version {found} (expected 1)")
            }
            ItemDefinitionLoadErrorKind::InvalidLocalName { declaration, name } => {
                write!(
                    formatter,
                    "declaration {declaration}: invalid local name {name:?}"
                )
            }
            ItemDefinitionLoadErrorKind::DuplicateLocalName {
                declaration,
                first_declaration,
                name,
            } => write!(
                formatter,
                "declaration {declaration}: duplicate local name {name:?} (first declaration {first_declaration})"
            ),
        }
    }
}

impl Error for ItemDefinitionLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            ItemDefinitionLoadErrorKind::Io(error) => Some(error),
            ItemDefinitionLoadErrorKind::Parse(error) => Some(error),
            _ => None,
        }
    }
}
