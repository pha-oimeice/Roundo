use std::{io, path::PathBuf, sync::Arc};

use thiserror::Error;

/// Failures of native USD import, source resolution and composition discovery.
#[derive(Clone, Debug, Error)]
pub enum UsdError {
    /// Invalid/ambiguous import metadata; no partial directory was published.
    #[error("invalid USD import: {0}")]
    Import(String),
    /// An asset URI is malformed or names a resource outside the import directory.
    #[error("invalid USD asset reference: {0}")]
    Reference(String),
    /// The originating Mod cannot access the referenced source.
    #[error("USD source access rejected: {0}")]
    Access(String),
    /// The source could not be inspected or read.
    #[error("cannot read USD source {path:?}: {source}")]
    Source {
        /// Source path, including an archive entry where applicable.
        path: PathBuf,
        /// Original filesystem or package I/O error.
        #[source]
        source: Arc<io::Error>,
    },
    /// Original upstream parser, dependency-discovery or query error.
    #[error(transparent)]
    OpenUsd(Arc<openusd::Error>),
    /// Resolved closure was incomplete; no successful prototype was published.
    #[error("unresolved USD dependencies: {0:?}")]
    Unresolved(Vec<String>),
    /// Traversal found composition diagnostics, even though opening succeeded.
    #[error("USD composition failed: {0:?}")]
    Composition(Vec<String>),
    /// The same source had different bytes within one root's dependency acquisition.
    #[error("USD source changed during dependency acquisition: {0}")]
    SourceChanged(String),
}

impl UsdError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Source {
            path: path.into(),
            source: Arc::new(source),
        }
    }
}

impl From<openusd::Error> for UsdError {
    fn from(error: openusd::Error) -> Self {
        Self::OpenUsd(Arc::new(error))
    }
}
