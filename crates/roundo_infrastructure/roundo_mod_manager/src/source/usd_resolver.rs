use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Cursor},
    path::Path,
    sync::{Arc, Mutex},
};

use openusd::{
    ar::{self, Asset, AssetBuffer, DefaultResolver, ResolvedPath, Resolver, SharedBuffer},
    usdz,
};

use super::usd_imports::ImportDirectory;
use crate::AssetKey;
use crate::usd::UsdError;

#[derive(Default)]
pub(crate) struct ReadState {
    pub files: BTreeMap<String, SharedBuffer>,
    pub assets: BTreeSet<AssetKey>,
    pub failure: Option<UsdError>,
}

pub(crate) struct ImportResolver {
    pub directory: Arc<ImportDirectory>,
    pub root: AssetKey,
    pub state: Arc<Mutex<ReadState>>,
    base: DefaultResolver,
}

impl ImportResolver {
    pub fn new(directory: Arc<ImportDirectory>, root: AssetKey) -> Self {
        Self {
            directory,
            root,
            state: Arc::new(Mutex::new(ReadState::default())),
            base: DefaultResolver::new(),
        }
    }

    fn reject(&self, error: UsdError) {
        let mut state = self.state.lock().expect("USD read state poisoned");
        if state.failure.is_none() {
            state.failure = Some(error);
        }
    }

    pub fn failure(&self) -> Option<UsdError> {
        self.state
            .lock()
            .expect("USD read state poisoned")
            .failure
            .clone()
    }

    fn authorize(&self, origin: &str, target: &str) -> Result<(), UsdError> {
        self.directory
            .authorize(self.directory.owner(origin)?, self.directory.owner(target)?)
    }

    fn observe(&self, resolved: &str) -> Result<(), UsdError> {
        let owner = self.directory.owner(resolved)?;
        self.directory.authorize(self.root.name().owner(), owner)?;
        let outer = ar::split_package_relative_path_outer(resolved)
            .map(|(outer, _)| outer)
            .unwrap_or_else(|| resolved.to_owned());
        if let Some(key) = self.directory.keys_by_path.get(&outer) {
            if key != &self.root {
                self.state
                    .lock()
                    .expect("USD read state poisoned")
                    .assets
                    .insert(key.clone());
            }
        }
        Ok(())
    }

    fn identifier(&self, path: &str, anchor: Option<&ResolvedPath>) -> Result<String, UsdError> {
        let origin = anchor.and_then(|anchor| anchor.to_str());
        let identified = if ar::is_uri(self, path) {
            let owner = match origin {
                Some(origin) => self.directory.owner(origin)?,
                None => self.root.name().owner(),
            };
            // Ar URI scheme dispatch is case-insensitive; normalize only the
            // approved scheme, never the case-sensitive resource identity.
            let (_, reference) = path.split_once(':').expect("recognized URI scheme");
            self.directory
                .asset(owner, &format!("asset:{reference}"))?
                .path
                .clone()
        } else {
            let identified = self.base.create_identifier(path, anchor);
            // ArDefaultResolver keeps missing search paths bare. This importer
            // has no CWD/search-root fallback: keep them beside their authoring
            // layer, so even an unresolved path has an unambiguous origin.
            if Path::new(&identified).is_relative() {
                let anchor = origin
                    .ok_or_else(|| UsdError::Reference(format!("unanchored source path {path}")))?;
                let outer = ar::split_package_relative_path_outer(anchor)
                    .map(|(outer, _)| outer)
                    .unwrap_or_else(|| anchor.to_owned());
                Path::new(&outer)
                    .parent()
                    .ok_or_else(|| UsdError::Reference(anchor.into()))?
                    .join(identified)
                    .to_str()
                    .ok_or_else(|| UsdError::Reference("USD paths must be UTF-8".into()))?
                    .to_owned()
            } else {
                identified
            }
        };
        let outer = ar::split_package_relative_path_outer(&identified)
            .map(|(outer, _)| outer)
            .unwrap_or_else(|| identified.clone());
        // Missing UDIM probes are normal. Existing targets, including package
        // files, must be authorized against the layer that authored the path.
        if let Ok(real) = std::fs::canonicalize(&outer) {
            let target = real
                .to_str()
                .ok_or_else(|| UsdError::Reference("USD paths must be UTF-8".into()))?;
            if let Some(origin) = origin {
                self.authorize(origin, target)?;
            }
        }
        Ok(identified)
    }

    fn resolved(&self, path: &str) -> Result<Option<ResolvedPath>, UsdError> {
        if path.is_empty() {
            return Ok(None);
        }
        if ar::is_uri(self, path) {
            let identifier = self.identifier(path, None)?;
            return self.resolved(&identifier);
        }
        if let Some((package, inner)) = ar::split_package_relative_path_outer(path) {
            let Some(package) = self.resolved(&package)? else {
                return Ok(None);
            };
            let package = package
                .to_str()
                .ok_or_else(|| UsdError::Reference("USD paths must be UTF-8".into()))?;
            if !usdz::package_contains(self, package, &inner) {
                return Ok(None);
            }
            let path = ar::join_package_relative_path(package, &inner);
            self.observe(&path)?;
            return Ok(Some(ResolvedPath::new(path)));
        }
        // Never ask DefaultResolver to search the process working directory.
        if !Path::new(path).is_absolute() {
            return Ok(None);
        }
        let real = match std::fs::canonicalize(path) {
            Ok(real) => real,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(UsdError::io(path, error)),
        };
        if !real.is_file() {
            return Ok(None);
        }
        let identifier = real
            .to_str()
            .ok_or_else(|| UsdError::Reference("USD paths must be UTF-8".into()))?;
        self.observe(identifier)?;
        Ok(Some(ResolvedPath::new(real)))
    }

    // The single byte-reading seam. Per-load snapshots guarantee stable replay;
    // there is deliberately no cross-load/global file cache or eviction policy.
    fn read_source(&self, path: &ResolvedPath) -> Result<SharedBuffer, UsdError> {
        let identifier = path
            .to_str()
            .ok_or_else(|| UsdError::Reference("USD paths must be UTF-8".into()))?;
        self.observe(identifier)?;
        if let Some(bytes) = self
            .state
            .lock()
            .expect("USD read state poisoned")
            .files
            .get(identifier)
            .cloned()
        {
            return Ok(bytes);
        }
        let bytes = if let Some((package, inner)) =
            ar::split_package_relative_path_outer(identifier)
        {
            usdz::package_entry(self, &package, &inner)
                .map_err(|error| UsdError::io(identifier, io::Error::other(error)))?
                .into_shared()
        } else {
            let bytes = super::read_file(path).map_err(|error| UsdError::io(identifier, error))?;
            SharedBuffer::new(bytes)
        };
        self.state
            .lock()
            .expect("USD read state poisoned")
            .files
            .insert(identifier.to_owned(), bytes.clone());
        Ok(bytes)
    }
}

impl Clone for ImportResolver {
    fn clone(&self) -> Self {
        Self {
            directory: Arc::clone(&self.directory),
            root: self.root.clone(),
            state: Arc::clone(&self.state),
            base: DefaultResolver::new(),
        }
    }
}

impl Resolver for ImportResolver {
    fn create_identifier(&self, path: &str, anchor: Option<&ResolvedPath>) -> String {
        match self.identifier(path, anchor) {
            Ok(identifier) => identifier,
            Err(error) => {
                self.reject(error);
                String::new()
            }
        }
    }

    fn resolve(&self, path: &str) -> Option<ResolvedPath> {
        match self.resolved(path) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.reject(error);
                None
            }
        }
    }

    fn resolve_for_new_asset(&self, _path: &str) -> Option<ResolvedPath> {
        // This is a read-only imported directory, not an authoring destination.
        None
    }

    fn open_asset(&self, path: &ResolvedPath) -> io::Result<Box<dyn Asset>> {
        match self.read_source(path) {
            Ok(bytes) => Ok(Box::new(Cursor::new(AssetBuffer::Shared(bytes)))),
            Err(error) => {
                self.reject(error.clone());
                Err(io::Error::other(error))
            }
        }
    }

    fn uri_schemes(&self) -> &[&str] {
        &["asset"]
    }

    fn identity(&self) -> String {
        format!("roundo-usd-snapshot:{:p}", Arc::as_ptr(&self.state))
    }
}
