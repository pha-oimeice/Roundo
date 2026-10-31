//! Client-owned server list, probe batches and versioned snapshots.
//!
//! The in-memory ClientConfig contains the only mutable list. Commands cannot edit it
//! without invalidating the probe batch and advancing the snapshot revision.
//! Persistence errors do not roll back an accepted in-memory edit.

use roundo_user_config::{ClientConfig, ClientSettingsConfig, ServerEntry};
use schemars::JsonSchema;
use serde::Serialize;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const CLIENT_CONFIG_FILE: &str = "roundo-client-config.toml";

/// One authoritative client configuration and its server-list runtime state.
/// Settings remain independently editable; no caller receives mutable list access.
#[derive(bevy::prelude::Resource)]
pub(crate) struct ClientConfigStore {
    config: ClientConfig,
    config_file: String,
    probes: Arc<Mutex<ProbeBatch>>,
    probe: Box<dyn ServerProbe>,
}

/// Internal network seam: start returns without waiting for remote I/O. The
/// completion is one-shot and retains only its batch's publication eligibility.
pub(crate) trait ServerProbe: Send + Sync + 'static {
    fn start(
        &self,
        server: ServerEntry,
        settings: roundo_user_config::ClientNetworkConfig,
        completion: ProbeCompletion,
    );
}

pub(crate) struct ProbeCompletion {
    probes: Arc<Mutex<ProbeBatch>>,
    batch_id: u64,
    index: usize,
    server: ServerEntry,
}
impl ProbeCompletion {
    pub(crate) fn complete(self, result: ProbeOutput) {
        let mut batch = self.probes.lock().unwrap_or_else(|p| p.into_inner());
        if batch.batch != self.batch_id {
            return;
        }
        if let Some(entry) = batch
            .entries
            .get_mut(self.index)
            .filter(|e| e.server == self.server)
        {
            entry.result = result;
            batch.revision += 1;
        }
    }
}

struct QuicProbe;
impl ServerProbe for QuicProbe {
    fn start(
        &self,
        server: ServerEntry,
        settings: roundo_user_config::ClientNetworkConfig,
        completion: ProbeCompletion,
    ) {
        std::thread::spawn(move || completion.complete(probe_server(&server, &settings)));
    }
}

#[derive(Default)]
struct ProbeBatch {
    batch: u64,
    revision: u64,
    entries: Vec<ProbeEntry>,
}
struct ProbeEntry {
    server: ServerEntry,
    result: ProbeOutput,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct ProbeOutput {
    pub(crate) status: String,
    pub(crate) message: Option<String>,
}
impl ProbeOutput {
    fn status(status: &str) -> Self {
        Self {
            status: status.into(),
            message: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct ServerListEntryOutput {
    pub(crate) index: usize,
    pub(crate) name: String,
    pub(crate) address: String,
    pub(crate) probe: ProbeOutput,
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct ServerListOutput {
    pub(crate) servers: Vec<ServerListEntryOutput>,
}

/// The revision and data are captured under the same probe lock. A later worker
/// result belongs to a later snapshot, never to the publisher's acknowledged one.
pub(crate) struct ServerListSnapshot {
    pub(crate) revision: u64,
    pub(crate) data: ServerListOutput,
}

#[derive(Debug)]
pub(crate) enum ServerListError {
    InvalidServer(&'static str),
    IndexOutOfRange,
    Save(String),
}

impl ClientConfigStore {
    pub(crate) fn new(config: ClientConfig) -> Self {
        Self::with_config_file(config, CLIENT_CONFIG_FILE)
    }

    /// Uses the normal executable-root writer; an absolute filename selects an
    /// explicit file; persistence still uses the normal writer and failure semantics.
    pub(crate) fn with_config_file(config: ClientConfig, file: impl Into<String>) -> Self {
        Self::with_probe(config, file, QuicProbe)
    }

    pub(crate) fn with_probe(
        config: ClientConfig,
        file: impl Into<String>,
        probe: impl ServerProbe,
    ) -> Self {
        Self {
            config,
            config_file: file.into(),
            probes: Arc::default(),
            probe: Box::new(probe),
        }
    }

    pub(crate) fn settings(&self) -> &ClientSettingsConfig {
        &self.config.settings
    }

    pub(crate) fn settings_mut(&mut self) -> &mut ClientSettingsConfig {
        &mut self.config.settings
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        roundo_user_config::save_config(&self.config_file, &self.config)
    }

    /// Preserves the existing persist-before-publish policy for binding replacement.
    /// The candidate includes the current server list; no stale config clone can
    /// replace a newer list or its probe state.
    pub(crate) fn replace_settings(
        &mut self,
        settings: ClientSettingsConfig,
    ) -> Result<(), String> {
        let mut candidate = self.config.clone();
        candidate.settings = settings;
        roundo_user_config::save_config(&self.config_file, &candidate)?;
        self.config.settings = candidate.settings;
        Ok(())
    }

    /// Selection is read-only. Connection/retry ownership remains in network.
    pub(crate) fn server(&self, index: usize) -> Result<&ServerEntry, ServerListError> {
        self.config
            .servers
            .get(index)
            .ok_or(ServerListError::IndexOutOfRange)
    }

    pub(crate) fn add_server(&mut self, server: ServerEntry) -> Result<usize, ServerListError> {
        validate(&server, "name and address are required")?;
        let index = self.config.servers.len();
        self.config.servers.push(server);
        self.server_list_changed()?;
        Ok(index)
    }

    pub(crate) fn edit_server(
        &mut self,
        index: usize,
        server: ServerEntry,
    ) -> Result<(), ServerListError> {
        validate(&server, "index and a complete server are required")?;
        let existing = self
            .config
            .servers
            .get_mut(index)
            .ok_or(ServerListError::IndexOutOfRange)?;
        *existing = server;
        self.server_list_changed()
    }

    pub(crate) fn delete_server(&mut self, index: usize) -> Result<(), ServerListError> {
        if index >= self.config.servers.len() {
            return Err(ServerListError::IndexOutOfRange);
        }
        self.config.servers.remove(index);
        self.server_list_changed()
    }

    fn server_list_changed(&self) -> Result<(), ServerListError> {
        {
            let mut batch = self.probe_batch();
            batch.batch += 1;
            batch.revision += 1;
            batch.entries.clear();
        }
        // Invalidation is an in-memory fact, not a consequence of disk success.
        self.save().map_err(ServerListError::Save)
    }

    /// Schedules one probe per current server and returns before DNS/TLS/QUIC I/O.
    /// Edits and refreshes invalidate previous batches. No edit starts a new probe.
    /// As before, workers are detached and cannot be cancelled by this store.
    pub(crate) fn refresh_servers(&self) -> u64 {
        let batch_id = {
            let mut batch = self.probe_batch();
            batch.batch += 1;
            batch.revision += 1;
            batch.entries = self
                .config
                .servers
                .iter()
                .cloned()
                .map(|server| ProbeEntry {
                    server,
                    result: ProbeOutput::status("probing"),
                })
                .collect();
            batch.batch
        };
        for (index, server) in self.config.servers.iter().cloned().enumerate() {
            let completion = ProbeCompletion {
                probes: Arc::clone(&self.probes),
                batch_id,
                index,
                server: server.clone(),
            };
            self.probe
                .start(server, self.config.network.clone(), completion);
        }
        batch_id
    }

    /// Returns None if the supplied revision is still current. Passing None as
    /// `since` forces an initial/re-subscription snapshot even for an empty list.
    pub(crate) fn server_list_snapshot(&self, since: Option<u64>) -> Option<ServerListSnapshot> {
        let batch = self.probe_batch();
        if since == Some(batch.revision) {
            return None;
        }
        let servers = self
            .config
            .servers
            .iter()
            .enumerate()
            .map(|(index, server)| {
                let probe = batch
                    .entries
                    .get(index)
                    .filter(|e| e.server == *server)
                    .map(|e| e.result.clone())
                    .unwrap_or_else(|| ProbeOutput::status("unknown"));
                ServerListEntryOutput {
                    index,
                    name: server.name.clone(),
                    address: server.address.clone(),
                    probe,
                }
            })
            .collect();
        Some(ServerListSnapshot {
            revision: batch.revision,
            data: ServerListOutput { servers },
        })
    }

    fn probe_batch(&self) -> MutexGuard<'_, ProbeBatch> {
        self.probes.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn validate(server: &ServerEntry, message: &'static str) -> Result<(), ServerListError> {
    if server.name.trim().is_empty() || server.address.trim().is_empty() {
        Err(ServerListError::InvalidServer(message))
    } else {
        Ok(())
    }
}

fn probe_server(
    server: &ServerEntry,
    settings: &roundo_user_config::ClientNetworkConfig,
) -> ProbeOutput {
    let config = match crate::network::network_config(server, settings) {
        Ok(config) => config,
        Err(message) => {
            return ProbeOutput {
                status: "error".into(),
                message: Some(message),
            };
        }
    };
    match roundo_networking::probe_quic_endpoint(
        config.quic_address,
        &config.server_name,
        config.certificate_policy,
        Duration::from_secs(2),
    ) {
        Ok(_) => ProbeOutput::status("reachable"),
        Err(error) => ProbeOutput {
            status: "unreachable".into(),
            message: Some(error.to_string()),
        },
    }
}
