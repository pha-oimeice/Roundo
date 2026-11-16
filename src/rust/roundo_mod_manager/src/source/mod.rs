//! Private, loader-owned file namespace and source bindings.
//! Only Mod initialization constructs directories/bindings. Asset handlers get
//! immutable bound sources, not discovery or registration capabilities.

pub(crate) mod usd_imports;
pub(crate) mod usd_resolver;

use crate::{LoadedMods, ModId};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub(crate) const UI_REGISTRY_PATH: &str = "assets/ui/registry.toml";

// One loader-owned byte-reading seam for configuration, UI and USD. No shared
// cross-asset cache is installed yet; adding one does not change consumer APIs.
pub(crate) fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) struct SourceDirectory {
    pub mods: Arc<LoadedMods>,
    pub roots: BTreeMap<ModId, PathBuf>,
}

impl SourceDirectory {
    pub fn new(mods: Arc<LoadedMods>) -> Result<Arc<Self>, String> {
        let mut roots: BTreeMap<ModId, PathBuf> = BTreeMap::new();
        for installed in mods.iter() {
            let root = std::fs::canonicalize(&installed.root)
                .map_err(|error| format!("{}: {error}", installed.root.display()))?;
            if !root.is_dir() {
                return Err(format!("Mod {} root is not a directory", installed.id));
            }
            if roots
                .values()
                .any(|other| root.starts_with(other) || other.starts_with(&root))
            {
                return Err(format!("overlapping Mod root for {}", installed.id));
            }
            roots.insert(installed.id.clone(), root);
        }
        Ok(Arc::new(Self { mods, roots }))
    }

    pub fn bind_ui(
        &self,
        owner: &ModId,
        relative: &str,
        page: bool,
    ) -> Result<Arc<BoundFile>, String> {
        let relative = Path::new(relative);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!(
                "UI entry must be relative to its resource directory: {relative:?}"
            ));
        }
        let base = Path::new(UI_REGISTRY_PATH)
            .parent()
            .expect("UI registry directory");
        let bound = self.bind(owner, &base.join(relative))?;
        let root = std::fs::canonicalize(self.roots[owner].join(base))
            .map_err(|error| error.to_string())?;
        if !bound.path().starts_with(root) {
            return Err(format!(
                "UI entry escapes its resource directory: {relative:?}"
            ));
        }
        if page
            && bound
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("toml")
        {
            return Err(format!("UI page must use .toml: {relative:?}"));
        }
        Ok(bound)
    }

    pub fn bind(&self, owner: &ModId, relative: &Path) -> Result<Arc<BoundFile>, String> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!(
                "source must be relative to its Mod root: {relative:?}"
            ));
        }
        let root = self
            .roots
            .get(owner)
            .ok_or_else(|| format!("unknown Mod {owner}"))?;
        let path = std::fs::canonicalize(root.join(relative))
            .map_err(|error| format!("{}: {error}", root.join(relative).display()))?;
        if !path.starts_with(root) || !path.is_file() {
            return Err(format!("source is not a file inside Mod {owner}: {path:?}"));
        }
        Ok(Arc::new(BoundFile { path }))
    }
}

#[derive(Debug)]
pub(crate) struct BoundFile {
    path: PathBuf,
}

impl BoundFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
    // The byte-reading seam for non-USD bound sources. A future shared cache
    // belongs to this source layer, not to consumers or directory scanners.
    pub fn read(&self) -> Result<Vec<u8>, String> {
        read_file(&self.path).map_err(|error| format!("{}: {error}", self.path.display()))
    }
}
