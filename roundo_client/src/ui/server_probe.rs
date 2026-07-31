use crate::{config::ServerEntry, network::resolve_socket_address};
use bevy::prelude::Resource;
use std::{
    net::TcpStream,
    sync::{
        Mutex,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1200);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EndpointReachability {
    NotChecked,
    Checking,
    Online { latency_ms: u128 },
    Offline(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ServerReachabilityKind {
    NotChecked,
    Checking,
    Online,
    Offline,
}

pub(super) struct ServerReachabilitySummary {
    pub(super) kind: ServerReachabilityKind,
    pub(super) label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ServerReachability {
    pub(super) game: EndpointReachability,
    pub(super) https: EndpointReachability,
}

impl ServerReachability {
    pub(super) fn summary(&self) -> ServerReachabilitySummary {
        let mut errors = Vec::new();
        for status in [&self.game, &self.https] {
            let EndpointReachability::Offline(error) = status else {
                continue;
            };
            if !errors.contains(&error.as_str()) {
                errors.push(error.as_str());
            }
        }
        if !errors.is_empty() {
            return ServerReachabilitySummary {
                kind: ServerReachabilityKind::Offline,
                label: format!("offline ({})", errors.join("; ")),
            };
        }
        if matches!(self.game, EndpointReachability::Checking)
            || matches!(self.https, EndpointReachability::Checking)
        {
            return ServerReachabilitySummary {
                kind: ServerReachabilityKind::Checking,
                label: "checking...".to_string(),
            };
        }
        if let (
            EndpointReachability::Online {
                latency_ms: game_latency,
            },
            EndpointReachability::Online {
                latency_ms: https_latency,
            },
        ) = (&self.game, &self.https)
        {
            return ServerReachabilitySummary {
                kind: ServerReachabilityKind::Online,
                label: format!("online ({} ms)", game_latency.max(https_latency)),
            };
        }
        ServerReachabilitySummary {
            kind: ServerReachabilityKind::NotChecked,
            label: "not checked".to_string(),
        }
    }

    fn not_checked() -> Self {
        Self {
            game: EndpointReachability::NotChecked,
            https: EndpointReachability::NotChecked,
        }
    }

    fn checking() -> Self {
        Self {
            game: EndpointReachability::Checking,
            https: EndpointReachability::Checking,
        }
    }
}

struct ProbeResult {
    generation: u64,
    index: usize,
    reachability: ServerReachability,
}

#[derive(Resource)]
pub(super) struct ServerProbeManager {
    generation: u64,
    statuses: Vec<ServerReachability>,
    sender: Sender<ProbeResult>,
    receiver: Mutex<Receiver<ProbeResult>>,
}

impl Default for ServerProbeManager {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            generation: 0,
            statuses: Vec::new(),
            sender,
            receiver: Mutex::new(receiver),
        }
    }
}

impl ServerProbeManager {
    pub(super) fn refresh(&mut self, servers: &[ServerEntry]) {
        self.generation = self.generation.wrapping_add(1);
        self.statuses = vec![ServerReachability::checking(); servers.len()];
        let generation = self.generation;
        let network = crate::config::network_config();
        let game_port = network.endpoint.game_port;
        let https_port = network.endpoint.https_port;

        for (index, server) in servers.iter().cloned().enumerate() {
            let sender = self.sender.clone();
            std::thread::spawn(move || {
                let reachability = ServerReachability {
                    game: check_endpoint(&server.game_addr, game_port, "game address"),
                    https: check_endpoint(&server.https_addr, https_port, "HTTPS address"),
                };
                let _ = sender.send(ProbeResult {
                    generation,
                    index,
                    reachability,
                });
            });
        }
    }

    pub(super) fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let result = {
                let receiver = self
                    .receiver
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                receiver.try_recv()
            };
            match result {
                Ok(result) => {
                    if result.generation == self.generation
                        && let Some(status) = self.statuses.get_mut(result.index)
                    {
                        *status = result.reachability;
                        changed = true;
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changed
    }

    pub(super) fn get(&self, index: usize) -> Option<&ServerReachability> {
        self.statuses.get(index)
    }

    pub(super) fn record_added(&mut self, index: usize) {
        self.invalidate_pending();
        if index <= self.statuses.len() {
            self.statuses
                .insert(index, ServerReachability::not_checked());
        }
    }

    pub(super) fn record_edited(&mut self, index: usize) {
        self.invalidate_pending();
        if let Some(status) = self.statuses.get_mut(index) {
            *status = ServerReachability::not_checked();
        }
    }

    pub(super) fn record_deleted(&mut self, index: usize) {
        self.invalidate_pending();
        if index < self.statuses.len() {
            self.statuses.remove(index);
        }
    }

    fn invalidate_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        for status in &mut self.statuses {
            if status.game == EndpointReachability::Checking {
                status.game = EndpointReachability::NotChecked;
            }
            if status.https == EndpointReachability::Checking {
                status.https = EndpointReachability::NotChecked;
            }
        }
    }
}

fn check_endpoint(address: &str, default_port: u16, field_name: &str) -> EndpointReachability {
    let socket_address = match resolve_socket_address(address, default_port, field_name) {
        Ok((socket_address, _)) => socket_address,
        Err(error) => return EndpointReachability::Offline(error),
    };
    let started = Instant::now();
    match TcpStream::connect_timeout(&socket_address, CONNECT_TIMEOUT) {
        Ok(_) => EndpointReachability::Online {
            latency_ms: started.elapsed().as_millis(),
        },
        Err(error) => EndpointReachability::Offline(error.kind().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EndpointReachability, ServerProbeManager, ServerReachability, ServerReachabilityKind,
        check_endpoint,
    };
    use std::net::TcpListener;

    #[test]
    fn detects_reachable_tcp_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let result = check_endpoint(
            &listener.local_addr().unwrap().to_string(),
            1,
            "test address",
        );

        assert!(matches!(result, EndpointReachability::Online { .. }));
    }

    #[test]
    fn invalid_endpoint_is_reported_without_rejecting_an_entry() {
        let result = check_endpoint(":", 1, "test address");

        assert!(matches!(result, EndpointReachability::Offline(_)));
    }

    #[test]
    fn adding_an_entry_does_not_start_a_probe() {
        let mut probes = ServerProbeManager::default();

        probes.record_added(0);

        let status = probes.get(0).unwrap();
        assert_eq!(status.game, EndpointReachability::NotChecked);
        assert_eq!(status.https, EndpointReachability::NotChecked);
    }

    #[test]
    fn combines_endpoint_results_into_one_server_status() {
        let online = ServerReachability {
            game: EndpointReachability::Online { latency_ms: 4 },
            https: EndpointReachability::Online { latency_ms: 9 },
        }
        .summary();
        assert_eq!(online.kind, ServerReachabilityKind::Online);
        assert_eq!(online.label, "online (9 ms)");

        let offline = ServerReachability {
            game: EndpointReachability::Offline("timed out".to_string()),
            https: EndpointReachability::Offline("timed out".to_string()),
        }
        .summary();
        assert_eq!(offline.kind, ServerReachabilityKind::Offline);
        assert_eq!(offline.label, "offline (timed out)");
    }
}
