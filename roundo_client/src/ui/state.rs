use crate::{
    config::{self, ServerEntry},
    network::ClientConnectionStatus,
};
use bevy::prelude::Resource;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ClientUiPhase {
    #[default]
    S0MainMenu,
    S1ServerSelection,
    S2Connecting,
    S3InGame,
    S4Settings,
    S5PauseMenu,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ServerSelectionView {
    #[default]
    List,
    AddEntry,
    EditEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ServerMutation {
    Added(usize),
    Edited(usize),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsReturn {
    #[default]
    MainMenu,
    PauseMenu,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClientUiEvent {
    StartGame,
    BackToMainMenu,
    BeginConnection(ServerEntry),
    ConnectionSucceeded,
    ConnectionFailed(String),
    CancelConnection,
    PauseGame,
    ContinueGame,
    OpenSettings,
    CloseSettings,
    LeaveGameToServers,
    GameEnded,
}

#[derive(Resource)]
pub(super) struct ClientUiState {
    pub(super) phase: ClientUiPhase,
    pub(super) server_selection_view: ServerSelectionView,
    pub(super) selected_server: Option<usize>,
    pub(super) servers: Vec<ServerEntry>,
    pub(super) new_server: ServerEntry,
    pub(super) connecting_server: Option<ServerEntry>,
    pub(super) status_message: String,
    pub(super) observed_connection_status: Option<ClientConnectionStatus>,
    pub(super) refresh_revision: u64,
    settings_return: SettingsReturn,
}

impl ClientUiState {
    pub(super) fn with_servers(servers: Vec<ServerEntry>) -> Self {
        Self {
            phase: ClientUiPhase::S0MainMenu,
            server_selection_view: ServerSelectionView::List,
            selected_server: None,
            servers,
            new_server: ServerEntry::default(),
            connecting_server: None,
            status_message: String::new(),
            observed_connection_status: None,
            refresh_revision: 0,
            settings_return: SettingsReturn::MainMenu,
        }
    }

    pub(super) fn transition(&mut self, event: ClientUiEvent) {
        match event {
            ClientUiEvent::StartGame if self.phase == ClientUiPhase::S0MainMenu => {
                self.open_server_selection();
            }
            ClientUiEvent::BackToMainMenu if self.phase == ClientUiPhase::S1ServerSelection => {
                self.reset_session(ClientUiPhase::S0MainMenu);
            }
            ClientUiEvent::BeginConnection(server)
                if self.phase == ClientUiPhase::S1ServerSelection =>
            {
                self.phase = ClientUiPhase::S2Connecting;
                self.server_selection_view = ServerSelectionView::List;
                self.connecting_server = Some(server);
                self.status_message = "Connecting...".to_string();
                self.observed_connection_status = Some(ClientConnectionStatus::Connecting);
            }
            ClientUiEvent::ConnectionSucceeded if self.phase == ClientUiPhase::S2Connecting => {
                self.phase = ClientUiPhase::S3InGame;
                self.status_message.clear();
            }
            ClientUiEvent::ConnectionFailed(error) => {
                if matches!(
                    self.phase,
                    ClientUiPhase::S2Connecting
                        | ClientUiPhase::S3InGame
                        | ClientUiPhase::S4Settings
                        | ClientUiPhase::S5PauseMenu
                ) {
                    self.phase = ClientUiPhase::S2Connecting;
                    self.status_message = error;
                }
            }
            ClientUiEvent::CancelConnection if self.phase == ClientUiPhase::S2Connecting => {
                self.open_server_selection();
            }
            ClientUiEvent::PauseGame if self.phase == ClientUiPhase::S3InGame => {
                self.phase = ClientUiPhase::S5PauseMenu;
            }
            ClientUiEvent::ContinueGame if self.phase == ClientUiPhase::S5PauseMenu => {
                self.phase = ClientUiPhase::S3InGame;
            }
            ClientUiEvent::OpenSettings => match self.phase {
                ClientUiPhase::S0MainMenu => {
                    self.settings_return = SettingsReturn::MainMenu;
                    self.phase = ClientUiPhase::S4Settings;
                }
                ClientUiPhase::S5PauseMenu => {
                    self.settings_return = SettingsReturn::PauseMenu;
                    self.phase = ClientUiPhase::S4Settings;
                }
                _ => {}
            },
            ClientUiEvent::CloseSettings if self.phase == ClientUiPhase::S4Settings => {
                self.phase = match self.settings_return {
                    SettingsReturn::MainMenu => ClientUiPhase::S0MainMenu,
                    SettingsReturn::PauseMenu => ClientUiPhase::S5PauseMenu,
                };
            }
            ClientUiEvent::LeaveGameToServers if self.phase == ClientUiPhase::S5PauseMenu => {
                self.open_server_selection();
            }
            ClientUiEvent::GameEnded
                if matches!(
                    self.phase,
                    ClientUiPhase::S3InGame
                        | ClientUiPhase::S4Settings
                        | ClientUiPhase::S5PauseMenu
                ) =>
            {
                self.reset_session(ClientUiPhase::S0MainMenu);
            }
            _ => {}
        }
        self.touch();
    }

    pub(super) fn set_connecting_message(&mut self, message: impl Into<String>) {
        self.phase = ClientUiPhase::S2Connecting;
        self.status_message = message.into();
        self.touch();
    }

    pub(super) fn world_visible(&self) -> bool {
        matches!(
            self.phase,
            ClientUiPhase::S3InGame | ClientUiPhase::S5PauseMenu
        ) || (self.phase == ClientUiPhase::S4Settings
            && self.settings_return == SettingsReturn::PauseMenu)
    }

    pub(super) fn touch(&mut self) {
        self.refresh_revision = self.refresh_revision.wrapping_add(1);
    }

    pub(super) fn select_server(&mut self, index: usize) {
        if self.phase == ClientUiPhase::S1ServerSelection
            && self.server_selection_view == ServerSelectionView::List
            && index < self.servers.len()
        {
            self.selected_server = Some(index);
            self.touch();
        }
    }

    pub(super) fn begin_add_server(&mut self) {
        if self.phase != ClientUiPhase::S1ServerSelection {
            return;
        }
        self.new_server = ServerEntry::default();
        self.status_message.clear();
        self.server_selection_view = ServerSelectionView::AddEntry;
        self.touch();
    }

    pub(super) fn begin_edit_selected_server(&mut self) -> bool {
        if self.phase != ClientUiPhase::S1ServerSelection {
            return false;
        }
        let Some(index) = self.selected_server else {
            return false;
        };
        let Some(server) = self.servers.get(index).cloned() else {
            self.selected_server = None;
            self.touch();
            return false;
        };
        self.new_server = server;
        self.status_message.clear();
        self.server_selection_view = ServerSelectionView::EditEntry;
        self.touch();
        true
    }

    pub(super) fn cancel_server_form(&mut self) {
        if matches!(
            self.server_selection_view,
            ServerSelectionView::AddEntry | ServerSelectionView::EditEntry
        ) {
            self.server_selection_view = ServerSelectionView::List;
            self.status_message.clear();
            self.touch();
        }
    }

    pub(super) fn save_server_form(&mut self, server: ServerEntry) -> Option<ServerMutation> {
        let mutation = match self.server_selection_view {
            ServerSelectionView::AddEntry => {
                let index = self.servers.len();
                self.servers.push(server);
                self.selected_server = Some(index);
                ServerMutation::Added(index)
            }
            ServerSelectionView::EditEntry => {
                let index = self.selected_server?;
                let stored = self.servers.get_mut(index)?;
                *stored = server;
                ServerMutation::Edited(index)
            }
            ServerSelectionView::List => return None,
        };
        self.new_server = ServerEntry::default();
        self.server_selection_view = ServerSelectionView::List;
        self.touch();
        Some(mutation)
    }

    pub(super) fn delete_selected_server(&mut self) -> Option<(usize, ServerEntry)> {
        if self.phase != ClientUiPhase::S1ServerSelection
            || self.server_selection_view != ServerSelectionView::List
        {
            return None;
        }
        let index = self.selected_server?;
        if index >= self.servers.len() {
            self.selected_server = None;
            self.touch();
            return None;
        }
        let removed = self.servers.remove(index);
        self.selected_server = if self.servers.is_empty() {
            None
        } else {
            Some(index.min(self.servers.len() - 1))
        };
        self.touch();
        Some((index, removed))
    }

    fn open_server_selection(&mut self) {
        self.phase = ClientUiPhase::S1ServerSelection;
        self.server_selection_view = ServerSelectionView::List;
        self.selected_server = None;
        self.connecting_server = None;
        self.status_message.clear();
        self.observed_connection_status = None;
        self.settings_return = SettingsReturn::MainMenu;
    }

    fn reset_session(&mut self, phase: ClientUiPhase) {
        self.phase = phase;
        self.server_selection_view = ServerSelectionView::List;
        self.selected_server = None;
        self.connecting_server = None;
        self.status_message.clear();
        self.observed_connection_status = None;
        self.settings_return = SettingsReturn::MainMenu;
    }
}

impl Default for ClientUiState {
    fn default() -> Self {
        Self::with_servers(config::CLIENT_CONFIG.servers.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientUiEvent, ClientUiPhase, ClientUiState, ServerMutation, ServerSelectionView};
    use crate::config::ServerEntry;

    fn server() -> ServerEntry {
        ServerEntry {
            name: "Test".to_string(),
            game_addr: "127.0.0.1:4000".to_string(),
            https_addr: "127.0.0.1:5000".to_string(),
        }
    }

    #[test]
    fn follows_connection_and_pause_state_machine() {
        let mut state = ClientUiState::with_servers(vec![server()]);
        assert_eq!(state.phase, ClientUiPhase::S0MainMenu);
        assert!(!state.world_visible());

        state.transition(ClientUiEvent::StartGame);
        assert_eq!(state.phase, ClientUiPhase::S1ServerSelection);

        state.transition(ClientUiEvent::BeginConnection(server()));
        assert_eq!(state.phase, ClientUiPhase::S2Connecting);

        state.transition(ClientUiEvent::ConnectionFailed("No route".to_string()));
        assert_eq!(state.phase, ClientUiPhase::S2Connecting);
        assert_eq!(state.status_message, "No route");

        state.transition(ClientUiEvent::ConnectionSucceeded);
        assert_eq!(state.phase, ClientUiPhase::S3InGame);
        assert!(state.world_visible());

        state.transition(ClientUiEvent::PauseGame);
        assert_eq!(state.phase, ClientUiPhase::S5PauseMenu);
        assert!(state.world_visible());

        state.transition(ClientUiEvent::ContinueGame);
        assert_eq!(state.phase, ClientUiPhase::S3InGame);
    }

    #[test]
    fn settings_returns_to_its_opening_screen() {
        let mut state = ClientUiState::with_servers(vec![]);
        state.transition(ClientUiEvent::OpenSettings);
        assert_eq!(state.phase, ClientUiPhase::S4Settings);
        assert!(!state.world_visible());
        state.transition(ClientUiEvent::CloseSettings);
        assert_eq!(state.phase, ClientUiPhase::S0MainMenu);

        state.transition(ClientUiEvent::StartGame);
        state.transition(ClientUiEvent::BeginConnection(server()));
        state.transition(ClientUiEvent::ConnectionSucceeded);
        state.transition(ClientUiEvent::PauseGame);
        state.transition(ClientUiEvent::OpenSettings);
        assert_eq!(state.phase, ClientUiPhase::S4Settings);
        assert!(state.world_visible());
        state.transition(ClientUiEvent::CloseSettings);
        assert_eq!(state.phase, ClientUiPhase::S5PauseMenu);
    }

    #[test]
    fn pause_exit_returns_to_server_selection() {
        let mut state = ClientUiState::with_servers(vec![]);
        state.transition(ClientUiEvent::StartGame);
        state.transition(ClientUiEvent::BeginConnection(server()));
        state.transition(ClientUiEvent::ConnectionSucceeded);
        state.transition(ClientUiEvent::PauseGame);

        state.transition(ClientUiEvent::LeaveGameToServers);

        assert_eq!(state.phase, ClientUiPhase::S1ServerSelection);
        assert!(!state.world_visible());
    }

    #[test]
    fn cancelling_connection_returns_to_server_selection() {
        let mut state = ClientUiState::with_servers(vec![]);
        state.transition(ClientUiEvent::StartGame);
        state.transition(ClientUiEvent::BeginConnection(server()));

        state.transition(ClientUiEvent::CancelConnection);

        assert_eq!(state.phase, ClientUiPhase::S1ServerSelection);
    }

    #[test]
    fn completed_game_returns_to_main_menu() {
        let mut state = ClientUiState::with_servers(vec![]);
        state.transition(ClientUiEvent::StartGame);
        state.transition(ClientUiEvent::BeginConnection(server()));
        state.transition(ClientUiEvent::ConnectionSucceeded);

        state.transition(ClientUiEvent::GameEnded);

        assert_eq!(state.phase, ClientUiPhase::S0MainMenu);
    }

    #[test]
    fn server_selection_supports_add_edit_and_delete() {
        let mut state = ClientUiState::with_servers(vec![server()]);
        state.transition(ClientUiEvent::StartGame);
        assert_eq!(state.selected_server, None);

        state.select_server(0);
        assert_eq!(state.selected_server, Some(0));
        assert!(state.begin_edit_selected_server());
        assert_eq!(state.server_selection_view, ServerSelectionView::EditEntry);

        let mut edited = server();
        edited.name = "Edited".to_string();
        assert_eq!(
            state.save_server_form(edited),
            Some(ServerMutation::Edited(0))
        );
        assert_eq!(state.servers[0].name, "Edited");

        state.begin_add_server();
        let mut added = server();
        added.name = "Added".to_string();
        assert_eq!(
            state.save_server_form(added),
            Some(ServerMutation::Added(1))
        );
        assert_eq!(state.selected_server, Some(1));

        let (_, deleted) = state.delete_selected_server().unwrap();
        assert_eq!(deleted.name, "Added");
        assert_eq!(state.selected_server, Some(0));
    }
}
