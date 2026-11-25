use super::super::local_game::{LocalControl, LocalScreen};
use super::{
    LauncherFont, LauncherRoot, SIDEBAR,
    directory::{DirectoryDisclosure, directory_scene, toggle_directory_details},
    form::{TextField, set_button_enabled},
    launcher_label_with_font,
    search::{SearchQuery, search_field_scene},
};
use bevy::{
    ecs::template::FromTemplate,
    feathers::{
        controls::{ButtonVariant, FeathersButton, FeathersTextInput, FeathersTextInputContainer},
        display::caption,
        font_styles::InheritableFont,
    },
    input_focus::{AutoFocus, FocusCause, InputFocus, tab_navigation::TabGroup},
    prelude::*,
    scene::Scene,
    text::{EditableText, TextEdit, TextEditChange},
    ui::InteractionDisabled,
    ui_widgets::Activate,
};
use chrono::{DateTime, Utc};
use std::time::{Duration, Instant, SystemTime};

const SAVE_NAME: TextField = TextField {
    label: "Name",
    required: true,
};

pub(super) struct LocalSavesPlugin;

impl Plugin for LocalSavesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalSession>()
            .add_systems(
                Update,
                sync_save_rows.run_if(in_state(LocalScreen::Launcher)),
            )
            .add_systems(OnEnter(LocalScreen::Game), show_loading_view)
            .add_systems(
                Update,
                sync_loading_view.run_if(in_state(LocalScreen::Game)),
            )
            .add_systems(OnExit(LocalScreen::Game), restore_launcher);
    }
}

// The launcher owns the in-memory rows, including while it is hidden in Game.
// A row entity identifies an entry independently of its display name.
#[derive(Component)]
struct LocalSave {
    name: String,
    created_at: SystemTime,
    runtime: Duration,
}

struct ActiveLocalSave {
    row: Entity,
    started_at: Instant,
}

#[derive(Resource, Default)]
struct LocalSession(Option<ActiveLocalSave>);

#[derive(Clone, Component, Default)]
struct SaveFilterInput;

#[derive(Component, FromTemplate)]
struct SaveRowLabels {
    title: Entity,
    runtime: Entity,
}

#[derive(Component, FromTemplate)]
struct RenameInput {
    row: Entity,
    button: Entity,
}

#[derive(Component, FromTemplate)]
struct RenameButton(Entity);

#[derive(Clone, Component, Default)]
struct SaveList;

#[derive(Clone, Component, Default)]
struct AddSaveButton;

#[derive(Clone, Component, Default)]
struct SaveFormSlot;

#[derive(Clone, Component, Default)]
struct SaveForm;

#[derive(Clone, Component, Default)]
struct SaveNameInput;

#[derive(Clone, Component, Default)]
struct CreateSaveButton;

#[derive(Component, FromTemplate)]
struct SaveRowAction(Entity);

#[derive(Clone, Component, Default)]
struct LocalLoadingView;

pub(super) fn local_save_list_scene(font: Handle<Font>, emoji_font: Handle<Font>) -> impl Scene {
    directory_scene(
        font.clone(),
        bsn! { SaveList },
        bsn! {
            AddSaveButton
            @FeathersButton { @caption: bsn! { @caption("+") } }
            AccessibleLabel("Create a save")
            on(toggle_save_form)
        },
        bsn! { SaveFormSlot },
        search_field_scene(
            font,
            emoji_font,
            "Filter saves by name",
            bsn! { SaveFilterInput },
        ),
    )
}

fn sync_save_rows(
    filter: Single<(Entity, Ref<SearchQuery>), With<SaveFilterInput>>,
    mut rows: Query<(
        Entity,
        Ref<LocalSave>,
        &SaveRowLabels,
        &mut Node,
        &mut Visibility,
    )>,
    mut labels: Query<&mut Text>,
    parents: Query<&ChildOf>,
    mut focus: ResMut<InputFocus>,
) {
    let (filter_input, filter) = filter.into_inner();
    for (row, save, fields, mut node, mut visibility) in &mut rows {
        if save.is_changed() {
            labels.get_mut(fields.title).expect("save title").0 = save.name.clone();
            labels.get_mut(fields.runtime).expect("save runtime").0 = runtime_label(save.runtime);
        } else if !filter.is_changed() {
            continue;
        }

        let matches = filter.matches(&save.name);
        let (display, row_visibility) = if matches {
            (Display::Flex, Visibility::Inherited)
        } else {
            (Display::None, Visibility::Hidden)
        };
        if node.display != display {
            node.display = display;
            *visibility = row_visibility;
            if !matches
                && let Some(focused) = focus.get()
                && parents
                    .iter_ancestors(focused)
                    .any(|ancestor| ancestor == row)
            {
                focus.set(filter_input, FocusCause::Auto);
            }
        }
    }
}

fn toggle_save_form(
    _: On<Activate>,
    mut commands: Commands,
    slot: Single<Entity, With<SaveFormSlot>>,
    form: Query<Entity, With<SaveForm>>,
    font: Res<LauncherFont>,
    add_button: Single<Entity, With<AddSaveButton>>,
    mut focus: ResMut<InputFocus>,
) {
    if let Ok(form) = form.single() {
        close_save_form(&mut commands, form, *add_button, &mut focus);
    } else {
        commands
            .spawn_scene(save_form_scene(font.0.clone()))
            .insert(ChildOf(*slot));
    }
}

fn save_form_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        SaveForm
        Node { flex_direction: FlexDirection::Column, row_gap: px(12), padding: {UiRect::all(px(12))} }
        BackgroundColor(SIDEBAR)
        Children [
            @launcher_label_with_font(font.clone(), "Create a save", 18.0)
            --
            Node { align_items: AlignItems::Center, column_gap: px(12) }
            Children [
                @launcher_label_with_font(font.clone(), SAVE_NAME.display_label(), 16.0)
                --
                @FeathersTextInputContainer
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                Node { min_width: px(0) }
                Children [
                    @FeathersTextInput
                    AccessibleLabel("Save name")
                    SaveNameInput
                    Node { min_width: px(0) }
                    AutoFocus
                    on(update_create_button)
                ]
            ]
            --
            Node { justify_content: JustifyContent::FlexEnd, column_gap: px(8) }
            Children [
                @FeathersButton { @caption: bsn! { @caption("Cancel") } }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                on(cancel_save_form)
                --
                @FeathersButton { @caption: bsn! { @caption("Create") }, @variant: ButtonVariant::Primary }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                CreateSaveButton
                @{SAVE_NAME.required.then(|| bsn! { InteractionDisabled })}
                on(create_save)
            ]
        ]
    }
}

fn update_create_button(
    event: On<TextEditChange>,
    input: Query<&EditableText, With<SaveNameInput>>,
    button: Single<(Entity, Has<InteractionDisabled>), With<CreateSaveButton>>,
    mut commands: Commands,
) {
    let text = input.get(event.event_target()).expect("save name input");
    let (button, disabled) = *button;
    set_button_enabled(
        &mut commands,
        button,
        disabled,
        SAVE_NAME.read(text).is_some(),
    );
}

fn cancel_save_form(
    _: On<Activate>,
    mut commands: Commands,
    form: Single<Entity, With<SaveForm>>,
    add_button: Single<Entity, With<AddSaveButton>>,
    mut focus: ResMut<InputFocus>,
) {
    close_save_form(&mut commands, *form, *add_button, &mut focus);
}

fn close_save_form(
    commands: &mut Commands,
    form: Entity,
    add_button: Entity,
    focus: &mut InputFocus,
) {
    focus.set(add_button, FocusCause::Auto);
    commands.entity(form).try_despawn();
}

fn create_save(
    _: On<Activate>,
    input: Single<&EditableText, With<SaveNameInput>>,
    form: Single<Entity, With<SaveForm>>,
    list: Single<Entity, With<SaveList>>,
    add_button: Single<Entity, With<AddSaveButton>>,
    font: Res<LauncherFont>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    let Some(name) = SAVE_NAME.read(&input) else {
        return;
    };
    let save = LocalSave {
        name,
        created_at: SystemTime::now(),
        runtime: Duration::ZERO,
    };
    commands
        .spawn_scene(save_row_scene(font.0.clone(), &save))
        .insert((ChildOf(*list), save));
    close_save_form(&mut commands, *form, *add_button, &mut focus);
}

fn runtime_label(runtime: Duration) -> String {
    let seconds = runtime.as_secs();
    format!(
        "Play time: {:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn save_row_scene(font: Handle<Font>, save: &LocalSave) -> impl Scene + use<> {
    let name = save.name.clone();
    let initial_name = TextEdit::Insert(save.name.clone().into());
    let created_at = DateTime::<Utc>::from(save.created_at);
    let created = format!("Created: {}", created_at.format("%Y-%m-%d %H:%M:%S UTC"));
    let runtime = runtime_label(save.runtime);
    bsn! {
        #SaveRow
        SaveRowLabels { title: #SaveTitle, runtime: #SaveRuntime }
        Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(8) }
            Children [
                @FeathersButton
                Node { flex_grow: 1.0, min_width: px(0), min_height: px(36), justify_content: JustifyContent::SpaceBetween, column_gap: px(12), overflow: {Overflow::clip()} }
                DirectoryDisclosure { row: #SaveRow, body: #SaveDetails, indicator: #ExpansionIndicator }
                on(toggle_directory_details)
                Children [
                    #SaveTitle
                    @launcher_label_with_font(font.clone(), name, 18.0)
                    Pickable::IGNORE
                    --
                    #ExpansionIndicator
                    @launcher_label_with_font(font.clone(), "+", 18.0)
                    Pickable::IGNORE
                ]
                --
                @FeathersButton { @caption: bsn! { @caption("Load") }, @variant: ButtonVariant::Primary }
                Node { min_width: px(76), min_height: px(36) }
                SaveRowAction(#SaveRow)
                on(load_save)
            ]
            --
            #SaveDetails
            Visibility::Hidden
            Node { display: Display::None, flex_direction: FlexDirection::Column, row_gap: px(8), padding: {UiRect::all(px(12))} }
            BackgroundColor(SIDEBAR)
            Children [
                Node { align_items: AlignItems::Center, column_gap: px(8) }
                Children [
                    @launcher_label_with_font(font.clone(), SAVE_NAME.display_label(), 16.0)
                    --
                    @FeathersTextInputContainer
                    InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                    Node { min_width: px(0) }
                    Children [
                        #RenameField
                        @FeathersTextInput
                        AccessibleLabel("Save name")
                        Node { min_width: px(0) }
                        EditableText { pending_edits: {vec![initial_name]} }
                        RenameInput { row: #SaveRow, button: #RenameSubmit }
                        on(update_rename_button)
                    ]
                    --
                    #RenameSubmit
                    @FeathersButton { @caption: bsn! { @caption("Save") } }
                    InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                    InteractionDisabled
                    SaveRowAction(#SaveRow)
                    RenameButton(#RenameField)
                    on(rename_save)
                ]
                --
                @launcher_label_with_font(font.clone(), created, 16.0)
                --
                #SaveRuntime
                @launcher_label_with_font(font.clone(), runtime, 16.0)
                --
                @launcher_label_with_font(font.clone(), "Size: Not available (no save file)", 16.0)
                --
                @FeathersButton { @caption: bsn! { @caption("Delete") } }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                Node { align_self: AlignSelf::FlexEnd }
                SaveRowAction(#SaveRow)
                on(delete_save)
            ]
        ]
    }
}

fn update_rename_button(
    event: On<TextEditChange>,
    inputs: Query<(&EditableText, &RenameInput)>,
    saves: Query<&LocalSave>,
    buttons: Query<Has<InteractionDisabled>, With<RenameButton>>,
    mut commands: Commands,
) {
    let (text, editor) = inputs.get(event.event_target()).expect("save rename input");
    let save = saves.get(editor.row).expect("save row for rename input");
    let disabled = buttons.get(editor.button).expect("save rename button");
    let change = SAVE_NAME.read(text).is_some_and(|name| name != save.name);
    set_button_enabled(&mut commands, editor.button, disabled, change);
}

fn rename_save(
    event: On<Activate>,
    buttons: Query<(&RenameButton, &SaveRowAction, Has<InteractionDisabled>)>,
    mut inputs: Query<&mut EditableText>,
    mut saves: Query<&mut LocalSave>,
    mut commands: Commands,
) {
    let (button, action, disabled) = buttons.get(event.entity).expect("save rename button");
    let mut input = inputs.get_mut(button.0).expect("save rename input");
    let mut save = saves.get_mut(action.0).expect("save row for rename button");
    let Some(name) = SAVE_NAME.read(&input) else {
        return;
    };
    if name == save.name {
        return;
    }
    if input.value().to_string() != name {
        input.clear();
        input.queue_edit(TextEdit::Insert(name.clone().into()));
    }
    save.name = name;
    set_button_enabled(&mut commands, event.entity, disabled, false);
}

fn delete_save(
    event: On<Activate>,
    actions: Query<&SaveRowAction>,
    saves: Query<(), With<LocalSave>>,
    session: Res<LocalSession>,
    add_button: Single<Entity, With<AddSaveButton>>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    let row = actions.get(event.entity).expect("delete save button").0;
    if session.0.is_none() && saves.contains(row) {
        focus.set(*add_button, FocusCause::Auto);
        commands.entity(row).try_despawn();
    }
}

fn load_save(
    event: On<Activate>,
    actions: Query<&SaveRowAction>,
    saves: Query<(), With<LocalSave>>,
    mut session: ResMut<LocalSession>,
    mut next: ResMut<NextState<LocalScreen>>,
) {
    let row = actions.get(event.entity).expect("load save button").0;
    if session.0.is_none() && saves.contains(row) {
        session.0 = Some(ActiveLocalSave {
            row,
            started_at: Instant::now(),
        });
        next.set(LocalScreen::Game);
    }
}

fn show_loading_view(
    session: Res<LocalSession>,
    saves: Query<&LocalSave>,
    mut launcher: Single<(&mut Node, &mut Visibility), With<LauncherRoot>>,
    font: Res<LauncherFont>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    let active_save = session
        .0
        .as_ref()
        .expect("game view requires a selected local save");
    let save = saves
        .get(active_save.row)
        .expect("active local save must still exist");
    launcher.0.display = Display::None;
    *launcher.1 = Visibility::Hidden;
    focus.clear();

    let title = format!("Current save: {}", save.name);
    let font = font.0.clone();
    commands.spawn_scene(bsn! {
        LocalLoadingView
        TabGroup
        Node { position_type: PositionType::Absolute, top: px(24), right: px(24), flex_direction: FlexDirection::Column, row_gap: px(8), padding: {UiRect::all(px(12))} }
        BackgroundColor(SIDEBAR)
        Children [
            @launcher_label_with_font(font.clone(), title, 18.0)
            --
            @launcher_label_with_font(font.clone(), "Generating terrain…", 16.0)
            --
            @FeathersButton { @caption: bsn! { @caption("Back to Launcher") } }
            InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
            AutoFocus
            on(return_to_launcher)
        ]
    });
}

pub(super) fn return_to_launcher(_: On<Activate>, mut next: ResMut<NextState<LocalScreen>>) {
    next.set(LocalScreen::Launcher);
}

fn sync_loading_view(
    control: Res<LocalControl>,
    mut loading: Single<(&mut Node, &mut Visibility), With<LocalLoadingView>>,
) {
    if control.is_changed() && *control != LocalControl::Loading {
        loading.0.display = Display::None;
        *loading.1 = Visibility::Hidden;
    }
}

fn restore_launcher(
    mut session: ResMut<LocalSession>,
    mut saves: Query<&mut LocalSave>,
    mut launcher: Single<(&mut Node, &mut Visibility), With<LauncherRoot>>,
    loading: Single<Entity, With<LocalLoadingView>>,
    add_button: Single<Entity, With<AddSaveButton>>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    commands.entity(*loading).despawn();
    let active = session
        .0
        .take()
        .expect("game view requires an active local save");
    saves
        .get_mut(active.row)
        .expect("active local save must still exist")
        .runtime += active.started_at.elapsed();
    launcher.0.display = Display::Flex;
    *launcher.1 = Visibility::Inherited;
    focus.set(*add_button, FocusCause::Auto);
}
