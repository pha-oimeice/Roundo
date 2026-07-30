use super::{
    UiAction,
    server_probe::{ServerProbeManager, ServerReachabilityKind},
    state::{ClientUiState, ServerSelectionView},
    widgets::{spawn_button, spawn_label, spawn_menu_button, spawn_status, spawn_text_input},
};
use bevy::{
    prelude::*,
    ui::{FocusPolicy, RelativeCursorPosition},
    ui_widgets::{ControlOrientation, Scrollbar, ScrollbarThumb},
};

#[derive(Component)]
pub(super) struct ServerNameInput;

#[derive(Component)]
pub(super) struct GameAddressInput;

#[derive(Component)]
pub(super) struct HttpsAddressInput;

#[derive(Component)]
pub(super) struct ServerListScrollArea;

#[derive(Component)]
pub(super) struct ServerListEntry(pub(super) usize);

pub(super) fn spawn(
    commands: &mut Commands,
    panel: Entity,
    state: &ClientUiState,
    probes: &ServerProbeManager,
    scroll_y: f32,
) {
    match state.server_selection_view {
        ServerSelectionView::List => {
            spawn_list(commands, panel, state, probes, scroll_y);
        }
        ServerSelectionView::AddEntry | ServerSelectionView::EditEntry => {
            spawn_server_form(commands, panel, state);
        }
    }
}

fn spawn_list(
    commands: &mut Commands,
    panel: Entity,
    state: &ClientUiState,
    probes: &ServerProbeManager,
    scroll_y: f32,
) {
    spawn_header(commands, panel);

    let list_frame = commands
        .spawn(Node {
            display: Display::Grid,
            width: percent(100),
            min_height: px(0),
            flex_grow: 1.0,
            grid_template_columns: vec![
                RepeatedGridTrack::flex(1, 1.0),
                RepeatedGridTrack::px(1, 10.0),
            ],
            column_gap: px(6),
            ..default()
        })
        .id();
    commands.entity(panel).add_child(list_frame);

    let scroll_area = commands
        .spawn((
            ServerListScrollArea,
            Node {
                width: percent(100),
                height: percent(100),
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(8),
                padding: UiRect::all(px(8)),
                overflow: Overflow::scroll_y(),
                grid_column: GridPlacement::start(1),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.04, 0.055)),
            ScrollPosition(Vec2::new(0.0, scroll_y)),
            RelativeCursorPosition::default(),
        ))
        .id();
    commands.entity(list_frame).add_child(scroll_area);

    if state.servers.is_empty() {
        spawn_label(commands, scroll_area, "No servers recorded");
    }
    for (index, server) in state.servers.iter().enumerate() {
        spawn_server_entry(commands, scroll_area, state, probes, index, server);
    }
    spawn_scrollbar(commands, list_frame, scroll_area);

    if !state.status_message.is_empty() {
        spawn_status(commands, panel, &state.status_message);
    }

    let action_menu = commands
        .spawn(Node {
            width: percent(100),
            column_gap: px(8),
            ..default()
        })
        .id();
    commands.entity(panel).add_child(action_menu);
    let has_selection = state.selected_server.is_some();
    spawn_menu_button(
        commands,
        action_menu,
        "Add Server",
        UiAction::OpenAddServer,
        true,
    );
    spawn_menu_button(
        commands,
        action_menu,
        "Join Server",
        UiAction::JoinSelectedServer,
        has_selection,
    );
    spawn_menu_button(
        commands,
        action_menu,
        "Edit Server",
        UiAction::EditSelectedServer,
        has_selection,
    );
    spawn_menu_button(
        commands,
        action_menu,
        "Delete Server",
        UiAction::DeleteSelectedServer,
        has_selection,
    );
}

fn spawn_header(commands: &mut Commands, panel: Entity) {
    let header = commands
        .spawn(Node {
            width: percent(100),
            align_items: AlignItems::Center,
            column_gap: px(8),
            ..default()
        })
        .id();
    commands.entity(panel).add_child(header);

    let heading = commands
        .spawn((
            Text::new("Servers"),
            TextFont {
                font_size: FontSize::Px(34.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                flex_basis: px(0),
                flex_grow: 2.0,
                ..default()
            },
        ))
        .id();
    commands.entity(header).add_child(heading);
    spawn_menu_button(commands, header, "Refresh", UiAction::RefreshServers, true);
    spawn_menu_button(commands, header, "Back", UiAction::BackToMainMenu, true);
}

fn spawn_server_entry(
    commands: &mut Commands,
    parent: Entity,
    state: &ClientUiState,
    probes: &ServerProbeManager,
    index: usize,
    server: &crate::config::ServerEntry,
) {
    let selected = state.selected_server == Some(index);
    let entry = commands
        .spawn((
            ServerListEntry(index),
            Interaction::default(),
            FocusPolicy::Block,
            Node {
                width: percent(100),
                min_height: px(68),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                column_gap: px(12),
                padding: px(12).all(),
                border: px(if selected { 2 } else { 1 }).all(),
                border_radius: BorderRadius::all(px(7)),
                ..default()
            },
            BackgroundColor(if selected {
                Color::srgb(0.075, 0.12, 0.17)
            } else {
                Color::srgb(0.055, 0.065, 0.09)
            }),
            BorderColor::all(if selected {
                Color::srgb(0.22, 0.68, 0.95)
            } else {
                Color::srgb(0.25, 0.3, 0.4)
            }),
        ))
        .id();
    commands.entity(parent).add_child(entry);
    let name = commands
        .spawn((
            Text::new(&server.name),
            TextFont {
                font_size: FontSize::Px(19.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                flex_basis: px(0),
                flex_grow: 1.0,
                ..default()
            },
        ))
        .id();
    commands.entity(entry).add_child(name);

    let summary = probes.get(index).map(|status| status.summary());
    let (kind, label) = summary
        .map(|summary| (summary.kind, summary.label))
        .unwrap_or((
            ServerReachabilityKind::NotChecked,
            "not checked".to_string(),
        ));
    let status = commands
        .spawn((
            Text::new(label),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
            TextColor(match kind {
                ServerReachabilityKind::NotChecked => Color::srgb(0.55, 0.58, 0.65),
                ServerReachabilityKind::Checking => Color::srgb(0.95, 0.78, 0.25),
                ServerReachabilityKind::Online => Color::srgb(0.3, 0.85, 0.45),
                ServerReachabilityKind::Offline => Color::srgb(0.95, 0.3, 0.3),
            }),
        ))
        .id();
    commands.entity(entry).add_child(status);
}

fn spawn_scrollbar(commands: &mut Commands, parent: Entity, target: Entity) {
    let scrollbar = commands
        .spawn((
            Node {
                min_width: px(10),
                grid_column: GridPlacement::start(2),
                ..default()
            },
            BackgroundColor(Color::srgb(0.055, 0.065, 0.09)),
            Scrollbar::new(target, ControlOrientation::Vertical, 18.0),
        ))
        .id();
    commands.entity(parent).add_child(scrollbar);
    let thumb = commands
        .spawn((
            BackgroundColor(Color::srgb(0.32, 0.38, 0.48)),
            BorderColor::all(Color::srgb(0.45, 0.52, 0.64)),
            ScrollbarThumb {
                border_radius: BorderRadius::all(px(5)),
                border: px(1).all(),
            },
        ))
        .id();
    commands.entity(scrollbar).add_child(thumb);
}

fn spawn_server_form(commands: &mut Commands, panel: Entity, state: &ClientUiState) {
    let heading = match state.server_selection_view {
        ServerSelectionView::AddEntry => "Add Server",
        ServerSelectionView::EditEntry => "Edit Server",
        ServerSelectionView::List => return,
    };
    let heading_entity = commands
        .spawn((
            Text::new(heading),
            TextFont {
                font_size: FontSize::Px(34.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    commands.entity(panel).add_child(heading_entity);
    spawn_text_input(
        commands,
        panel,
        "Name",
        &state.new_server.name,
        ServerNameInput,
    );
    spawn_text_input(
        commands,
        panel,
        "Game address",
        &state.new_server.game_addr,
        GameAddressInput,
    );
    spawn_text_input(
        commands,
        panel,
        "HTTPS address",
        &state.new_server.https_addr,
        HttpsAddressInput,
    );
    if !state.status_message.is_empty() {
        spawn_status(commands, panel, &state.status_message);
    }
    spawn_button(commands, panel, "Save", UiAction::SaveServer);
    spawn_button(commands, panel, "Cancel", UiAction::CancelServerForm);
}
