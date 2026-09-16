//! Typed resource registration and all-or-nothing Mod resource loading.

use crate::LoadedMods;
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
};

/// Stable process-local identifier for one class of Mod resource.
///
/// The wrapped name is not validated; catalogs use exact string equality and
/// lexicographic ordering. The associated Rust type is established separately
/// by [`ResourceTypeCatalog::register`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResourceTypeId(pub &'static str);

/// Resource type that publishes the atomic voxel registry.
pub const ATOMIC_VOXEL_RESOURCE_TYPE: ResourceTypeId = ResourceTypeId("atomic-voxel");
/// Resource type that publishes the Web UI registry.
pub const WEB_UI_RESOURCE_TYPE: ResourceTypeId = ResourceTypeId("web-ui");
/// Resource type that publishes the semantic input-slot registry.
pub const INPUT_RESOURCE_TYPE: ResourceTypeId = ResourceTypeId("input");

type ErasedResource = Box<dyn Any + Send + Sync>;
type LoadResource = Box<dyn Fn(&LoadedMods) -> Result<ErasedResource, String> + Send + Sync>;

/// Type-erased loader paired with its expected Rust type.
struct ResourceTypeRegistration {
    dependencies: &'static [ResourceTypeId],
    load: LoadResource,
}

/// Registry of resource loaders keyed by stable resource type.
///
/// Registration order has no semantic effect. Loading owns Mod discovery,
/// dependency-closure selection, deterministic phases, phase-level error
/// aggregation, and all-or-nothing publication of returned values. Loader side
/// effects, if any, are outside the catalog and are not rolled back.
#[derive(Default)]
pub struct ResourceTypeCatalog {
    registrations: BTreeMap<ResourceTypeId, ResourceTypeRegistration>,
    duplicate: Option<ResourceTypeId>,
}

impl ResourceTypeCatalog {
    /// Creates an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a typed loader and its prerequisite resource types.
    ///
    /// This builder does not require dependencies to have been registered yet.
    /// Registering the same ID twice makes the whole catalog invalid; a later
    /// [`load`](Self::load) fails before Mod discovery or any loader invocation.
    pub fn register<T, E>(
        mut self,
        id: ResourceTypeId,
        dependencies: &'static [ResourceTypeId],
        load: fn(&LoadedMods) -> Result<T, E>,
    ) -> Self
    where
        T: Any + Send + Sync + 'static,
        E: fmt::Display + 'static,
    {
        let registration = ResourceTypeRegistration {
            dependencies,
            load: Box::new(move |mods| {
                load(mods)
                    .map(|resource| Box::new(resource) as ErasedResource)
                    .map_err(|error| error.to_string())
            }),
        };
        if self.registrations.insert(id, registration).is_some() {
            self.duplicate = Some(id);
        }
        self
    }

    /// Discovers Mods and loads the requested resource-type dependency closure.
    ///
    /// Types within a dependency phase run in ID order. Every loader in a
    /// failing phase is invoked so that independent failures can be returned
    /// together; later phases are skipped. Successfully staged values are
    /// returned only if every selected phase succeeds. This consumes the
    /// catalog and performs synchronous filesystem I/O during Mod discovery.
    /// The catalog discards staged values on failure but cannot roll back side
    /// effects performed inside loaders.
    ///
    /// # Errors
    ///
    /// Returns errors for duplicate or missing registrations, dependency
    /// cycles, Mod discovery, or loader failures. Aggregated errors are sorted
    /// by resource type ID.
    pub fn load(
        self,
        mods_root: impl AsRef<Path>,
        roots: &[ResourceTypeId],
    ) -> Result<LoadedResourceTypes, ResourceTypeLoadErrors> {
        if let Some(id) = self.duplicate {
            return Err(ResourceTypeLoadErrors::one(id, "duplicate Resource Type"));
        }
        let mods = LoadedMods::discover(mods_root)
            .map_err(|error| ResourceTypeLoadErrors::one(ResourceTypeId("mod-discovery"), error))?;
        let selected = self.selected_types(roots)?;
        let phases = self.loading_phases(&selected)?;
        let mut published = BTreeMap::new();

        for phase in phases {
            let mut staged = Vec::new();
            let mut errors = Vec::new();
            for id in phase {
                let registration = &self.registrations[&id];
                match (registration.load)(&mods) {
                    Ok(resource) => staged.push((id, resource)),
                    Err(message) => errors.push(ResourceTypeLoadError { id, message }),
                }
            }
            if !errors.is_empty() {
                errors.sort_by_key(|error| error.id);
                return Err(ResourceTypeLoadErrors(errors));
            }
            published.extend(staged);
        }

        Ok(LoadedResourceTypes { published })
    }

    fn selected_types(
        &self,
        roots: &[ResourceTypeId],
    ) -> Result<BTreeSet<ResourceTypeId>, ResourceTypeLoadErrors> {
        let mut selected = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(id) = pending.pop() {
            if !selected.insert(id) {
                continue;
            }
            let Some(registration) = self.registrations.get(&id) else {
                return Err(ResourceTypeLoadErrors::one(
                    id,
                    "Resource Type is not registered",
                ));
            };
            pending.extend(registration.dependencies.iter().copied());
        }
        Ok(selected)
    }

    fn loading_phases(
        &self,
        selected: &BTreeSet<ResourceTypeId>,
    ) -> Result<Vec<Vec<ResourceTypeId>>, ResourceTypeLoadErrors> {
        let mut remaining = selected.clone();
        let mut loaded = BTreeSet::new();
        let mut phases = Vec::new();
        while !remaining.is_empty() {
            let ready = remaining
                .iter()
                .copied()
                .filter(|id| {
                    self.registrations[id]
                        .dependencies
                        .iter()
                        .all(|dependency| loaded.contains(dependency))
                })
                .collect::<Vec<_>>();
            if ready.is_empty() {
                let id = *remaining.iter().next().expect("remaining is non-empty");
                return Err(ResourceTypeLoadErrors::one(
                    id,
                    "Resource Type dependency graph contains a cycle",
                ));
            }
            for id in &ready {
                let removed = remaining.remove(id);
                debug_assert!(removed, "ready Resource Type must still be pending");
                loaded.insert(*id);
            }
            phases.push(ready);
        }
        Ok(phases)
    }
}

/// Successfully loaded resources awaiting one-time typed extraction.
///
/// Values are owned by this collection. Each resource type can be taken at
/// most once; no borrowing or cloning contract is imposed on loaded values.
pub struct LoadedResourceTypes {
    published: BTreeMap<ResourceTypeId, ErasedResource>,
}

impl LoadedResourceTypes {
    /// Removes and downcasts one published resource.
    ///
    /// The value is removed before its concrete type is checked. A missing ID
    /// or wrong `T` therefore returns an error, and either failure leaves no
    /// retrievable value for that ID.
    pub fn take<T: Any + Send + Sync + 'static>(
        &mut self,
        id: ResourceTypeId,
    ) -> Result<T, ResourceTypeLoadErrors> {
        let published = self.published.remove(&id);
        let resource = published
            .ok_or_else(|| ResourceTypeLoadErrors::one(id, "Resource Type was not published"))?;
        resource
            .downcast::<T>()
            .map(|resource| *resource)
            .map_err(|_| {
                ResourceTypeLoadErrors::one(id, "published Resource Type has the wrong Rust type")
            })
    }
}

/// Failure to load one registered resource type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceTypeLoadError {
    /// Resource type whose registration or loader failed.
    pub id: ResourceTypeId,
    /// Human-readable context supplied by the catalog or loader.
    pub message: String,
}

/// Resource failures from one catalog load, ordered by resource type ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceTypeLoadErrors(pub Vec<ResourceTypeLoadError>);

// Aggregation preserves every independent loader failure.
impl ResourceTypeLoadErrors {
    fn one(id: ResourceTypeId, message: impl fmt::Display) -> Self {
        Self(vec![ResourceTypeLoadError {
            id,
            message: message.to_string(),
        }])
    }
}

impl fmt::Display for ResourceTypeLoadErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{}: {}", error.id.0, error.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ResourceTypeLoadErrors {}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: ResourceTypeId = ResourceTypeId("base");
    const ROOT: ResourceTypeId = ResourceTypeId("root");
    const BASE_DEPENDENCIES: &[ResourceTypeId] = &[];
    const ROOT_DEPENDENCIES: &[ResourceTypeId] = &[BASE];

    fn load_base(_: &LoadedMods) -> Result<u32, &'static str> {
        Ok(7)
    }

    fn load_root(_: &LoadedMods) -> Result<String, &'static str> {
        Ok("ready".into())
    }

    #[test]
    fn selection_loads_dependency_closure_and_publishes_typed_results() {
        let root = std::env::temp_dir().join(format!(
            "roundo-resource-type-catalog-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut loaded = ResourceTypeCatalog::new()
            .register(BASE, BASE_DEPENDENCIES, load_base)
            .register(ROOT, ROOT_DEPENDENCIES, load_root)
            .load(&root, &[ROOT])
            .unwrap();
        assert_eq!(loaded.take::<u32>(BASE).unwrap(), 7);
        assert_eq!(loaded.take::<String>(ROOT).unwrap(), "ready");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phase_errors_are_sorted_and_prevent_publication() {
        const A: ResourceTypeId = ResourceTypeId("a");
        const B: ResourceTypeId = ResourceTypeId("b");
        fn fail_a(_: &LoadedMods) -> Result<u32, &'static str> {
            Err("a failed")
        }
        fn fail_b(_: &LoadedMods) -> Result<u32, &'static str> {
            Err("b failed")
        }
        let root = std::env::temp_dir().join(format!(
            "roundo-resource-type-errors-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let errors = match ResourceTypeCatalog::new()
            .register(B, &[], fail_b)
            .register(A, &[], fail_a)
            .load(&root, &[B, A])
        {
            Ok(_) => panic!("failed phase must not publish resources"),
            Err(errors) => errors,
        };
        assert_eq!(
            errors.0.iter().map(|error| error.id).collect::<Vec<_>>(),
            vec![A, B]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
