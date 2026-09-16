use std::{borrow::Borrow, collections::HashMap, fmt, hash::Hash};

/// Marks a value as a definition that can be stored in a [`Registry`].
pub trait RegistryDefinitionTrait {}

/// A registry of uniquely keyed definitions.
///
/// Cloning copies the current map; later mutations are independent. Iteration
/// order is deliberately unspecified because storage is hash-based.
#[derive(Debug, Clone)]
pub struct Registry<K, V>
where
    K: Eq + Hash,
    V: RegistryDefinitionTrait,
{
    entries: HashMap<K, V>,
}

/// The entry rejected by [`Registry::register`] because its key was already registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryInsertError<K, V> {
    pub key: K,
    pub definition: V,
}

impl<K, V> fmt::Display for RegistryInsertError<K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("registry key is already registered")
    }
}

impl<K, V> std::error::Error for RegistryInsertError<K, V>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
}

impl<K, V> Registry<K, V>
where
    K: Eq + Hash,
    V: RegistryDefinitionTrait,
{
    /// Creates an empty registry with no reserved keys.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Registers a definition without replacing an existing entry.
    ///
    /// On duplicate key, the existing definition remains unchanged and the
    /// error returns ownership of both rejected arguments.
    pub fn register(&mut self, key: K, definition: V) -> Result<(), RegistryInsertError<K, V>> {
        if self.entries.contains_key(&key) {
            return Err(RegistryInsertError { key, definition });
        }

        self.entries.insert(key, definition);
        Ok(())
    }

    /// Borrows the definition for `key`, or returns `None` when absent.
    ///
    /// The reference remains valid until the registry is mutably borrowed.
    pub fn definition<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let definition = self.entries.get(key);
        definition
    }

    /// Returns whether a definition is registered for `key`.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.contains_key(key)
    }

    /// Iterates over entries in an unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
    }

    /// Returns the number of registered keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no keys are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K, V> Default for Registry<K, V>
where
    K: Eq + Hash,
    V: RegistryDefinitionTrait,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestDefinition(u32);

    impl RegistryDefinitionTrait for TestDefinition {}

    #[test]
    fn registers_and_reads_definition() {
        let mut registry = Registry::new();

        assert!(registry.is_empty());
        registry
            .register("example".to_owned(), TestDefinition(1))
            .unwrap();

        assert_eq!(registry.definition("example"), Some(&TestDefinition(1)));
        assert!(registry.contains_key("example"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn duplicate_registration_returns_rejected_entry_without_replacing_original() {
        let mut registry = Registry::new();
        registry
            .register("duplicate".to_owned(), TestDefinition(1))
            .unwrap();

        let error = registry
            .register("duplicate".to_owned(), TestDefinition(2))
            .unwrap_err();

        assert_eq!(
            error,
            RegistryInsertError {
                key: "duplicate".to_owned(),
                definition: TestDefinition(2),
            }
        );
        assert_eq!(registry.definition("duplicate"), Some(&TestDefinition(1)));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn iterates_over_registered_entries() {
        let mut registry = Registry::new();
        registry
            .register("example".to_owned(), TestDefinition(1))
            .unwrap();

        let entries = registry.iter().collect::<Vec<_>>();

        assert_eq!(entries, vec![(&"example".to_owned(), &TestDefinition(1))]);
    }

    #[test]
    fn default_creates_an_empty_registry() {
        let registry = Registry::<String, TestDefinition>::default();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
