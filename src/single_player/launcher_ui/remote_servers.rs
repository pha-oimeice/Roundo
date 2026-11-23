use super::{
    LauncherFont, SIDEBAR,
    directory::{DirectoryDisclosure, directory_scene, toggle_directory_details},
    launcher_label_with_font,
};
use bevy::{
    ecs::template::FromTemplate,
    feathers::{
        controls::{ButtonVariant, FeathersButton, FeathersTextInput, FeathersTextInputContainer},
        display::caption,
        font_styles::InheritableFont,
    },
    input_focus::{AutoFocus, FocusCause, InputFocus},
    prelude::*,
    scene::Scene,
    text::{EditableText, TextEdit, TextEditChange},
    ui::InteractionDisabled,
    ui_widgets::Activate,
};

pub(super) struct RemoteDirectoryPlugin;

impl Plugin for RemoteDirectoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_remote_rows);
    }
}

#[derive(Clone, Component, Default)]
struct RemoteList;

#[derive(Clone, Component, Default)]
struct RemoteFilter;

#[derive(Clone, Component, Default)]
struct AddServer;

#[derive(Clone, Component, Default)]
struct FormSlot;

#[derive(Clone, Component, Default)]
struct AddForm;

#[derive(Clone, Component, Default)]
struct NewName;

#[derive(Clone, Component, Default)]
struct NewAddress;

#[derive(Clone, Component, Default)]
struct Submit;

// Remote entries exist only as launcher UI. Their addresses are display text,
// not resolved endpoints or connection settings.
#[derive(Component, FromTemplate)]
struct RemoteRow {
    title: Entity,
    name: Entity,
    address: Entity,
}

#[derive(Component, FromTemplate)]
struct RemoveServer(Entity);

pub(super) fn remote_server_list_scene(font: Handle<Font>) -> impl Scene {
    directory_scene(
        font,
        bsn! { RemoteList },
        bsn! {
            AddServer
            @FeathersButton { @caption: bsn! { @caption("+") } }
            AccessibleLabel("新增远程服务器")
            on(toggle_add_form)
        },
        bsn! { FormSlot },
        bsn! {
            @FeathersTextInput
            AccessibleLabel("Filter remote servers by name or address")
            RemoteFilter
        },
    )
}

fn toggle_add_form(
    _: On<Activate>,
    form: Query<Entity, With<AddForm>>,
    slot: Single<Entity, With<FormSlot>>,
    add: Single<Entity, With<AddServer>>,
    font: Res<LauncherFont>,
    mut commands: Commands,
    mut focus: ResMut<InputFocus>,
) {
    if let Ok(form) = form.single() {
        close_form(&mut commands, &mut focus, form, *add);
    } else {
        commands
            .spawn_scene(add_form_scene(font.0.clone()))
            .insert(ChildOf(*slot));
    }
}

fn server_field(font: Handle<Font>, label: &'static str) -> impl Scene {
    bsn! {
        Node { align_items: AlignItems::Center, column_gap: px(8) }
        Children [ @launcher_label_with_font(font, label, 16.0) ]
    }
}

fn server_input_container(font: Handle<Font>) -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        InheritableFont { font: font, font_size: FontSize::Px(16.0) }
        Node { min_width: px(0) }
    }
}

fn add_form_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        AddForm
        Node { flex_direction: FlexDirection::Column, row_gap: px(8), padding: {UiRect::all(px(12))} }
        BackgroundColor(SIDEBAR)
        Children [
            @launcher_label_with_font(font.clone(), "新增远程服务器", 18.0)
            --
            @server_field(font.clone(), "Name:")
            Children [
                @server_input_container(font.clone())
                Children [
                    @FeathersTextInput
                    Node { min_width: px(0) }
                    AccessibleLabel("服务器名称")
                    NewName
                    AutoFocus
                    on(sync_submit)
                ]
            ]
            --
            @server_field(font.clone(), "Address:")
            Children [
                @server_input_container(font.clone())
                Children [
                    @FeathersTextInput
                    Node { min_width: px(0) }
                    AccessibleLabel("服务器地址")
                    NewAddress
                    on(sync_submit)
                ]
            ]
            --
            Node { justify_content: JustifyContent::FlexEnd, column_gap: px(8) }
            Children [
                @FeathersButton { @caption: bsn! { @caption("取消") } }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                on(cancel_add)
                --
                @FeathersButton { @caption: bsn! { @caption("添加") }, @variant: ButtonVariant::Primary }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                Submit
                InteractionDisabled
                on(add_row)
            ]
        ]
    }
}

fn form_values(name: &EditableText, address: &EditableText) -> Option<(String, String)> {
    let name = name.value().to_string();
    let address = address.value().to_string();
    let (name, address) = (name.trim(), address.trim());
    (!name.is_empty() && !address.is_empty()).then(|| (name.to_owned(), address.to_owned()))
}

fn sync_submit(
    _: On<TextEditChange>,
    name: Single<&EditableText, With<NewName>>,
    address: Single<&EditableText, With<NewAddress>>,
    button: Single<(Entity, Has<InteractionDisabled>), With<Submit>>,
    mut commands: Commands,
) {
    let (button, disabled) = *button;
    match (form_values(&name, &address).is_some(), disabled) {
        (true, true) => {
            commands.entity(button).remove::<InteractionDisabled>();
        }
        (false, false) => {
            commands.entity(button).insert(InteractionDisabled);
        }
        _ => {}
    }
}

fn cancel_add(
    _: On<Activate>,
    form: Single<Entity, With<AddForm>>,
    add: Single<Entity, With<AddServer>>,
    mut commands: Commands,
    mut focus: ResMut<InputFocus>,
) {
    close_form(&mut commands, &mut focus, *form, *add);
}

fn close_form(commands: &mut Commands, focus: &mut InputFocus, form: Entity, add: Entity) {
    focus.set(add, FocusCause::Auto);
    commands.entity(form).try_despawn();
}

fn add_row(
    _: On<Activate>,
    name: Single<&EditableText, With<NewName>>,
    address: Single<&EditableText, With<NewAddress>>,
    form: Single<Entity, With<AddForm>>,
    list: Single<Entity, With<RemoteList>>,
    add: Single<Entity, With<AddServer>>,
    font: Res<LauncherFont>,
    mut commands: Commands,
    mut focus: ResMut<InputFocus>,
) {
    let Some((name, address)) = form_values(&name, &address) else {
        return;
    };
    commands
        .spawn_scene(row_scene(font.0.clone(), name, address))
        .insert(ChildOf(*list));
    close_form(&mut commands, &mut focus, *form, *add);
}

fn row_scene(font: Handle<Font>, name: String, address: String) -> impl Scene {
    let name_edit = TextEdit::Insert(name.clone().into());
    let address_edit = TextEdit::Insert(address.into());
    bsn! {
        #RemoteRow
        RemoteRow { title: #ServerTitle, name: #ServerName, address: #ServerAddress }
        Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0 }
        Children [
            @FeathersButton
            Node { width: percent(100), min_height: px(36), justify_content: JustifyContent::SpaceBetween, column_gap: px(12), overflow: {Overflow::clip()} }
            DirectoryDisclosure { row: #RemoteRow, body: #RemoteDetails, indicator: #ExpansionIndicator }
            on(toggle_directory_details)
            Children [
                #ServerTitle
                @launcher_label_with_font(font.clone(), name, 18.0)
                Pickable::IGNORE
                --
                #ExpansionIndicator
                @launcher_label_with_font(font.clone(), "+", 18.0)
                Pickable::IGNORE
            ]
            --
            #RemoteDetails
            Visibility::Hidden
            Node { display: Display::None, flex_direction: FlexDirection::Column, row_gap: px(8), padding: {UiRect::all(px(12))} }
            BackgroundColor(SIDEBAR)
            Children [
                @server_field(font.clone(), "Name:")
                Children [
                    @server_input_container(font.clone())
                    Children [
                        #ServerName
                        @FeathersTextInput
                        Node { min_width: px(0) }
                        AccessibleLabel("服务器名称")
                        EditableText { pending_edits: {vec![name_edit]} }
                    ]
                ]
                --
                @server_field(font.clone(), "Address:")
                Children [
                    @server_input_container(font.clone())
                    Children [
                        #ServerAddress
                        @FeathersTextInput
                        Node { min_width: px(0) }
                        AccessibleLabel("服务器地址")
                        EditableText { pending_edits: {vec![address_edit]} }
                    ]
                ]
                --
                @FeathersButton { @caption: bsn! { @caption("删除") } }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
                Node { align_self: AlignSelf::FlexEnd }
                RemoveServer(#RemoteRow)
                on(remove_row)
            ]
        ]
    }
}

fn sync_remote_rows(
    filter: Single<(Entity, Ref<EditableText>), With<RemoteFilter>>,
    inputs: Query<Ref<EditableText>>,
    mut rows: Query<(Entity, Ref<RemoteRow>, &mut Node, &mut Visibility)>,
    mut labels: Query<&mut Text>,
    parents: Query<&ChildOf>,
    mut focus: ResMut<InputFocus>,
) {
    let (filter_entity, filter) = filter.into_inner();
    let needle = filter.value().to_string().to_lowercase();
    for (entity, row, mut node, mut visibility) in &mut rows {
        let name = inputs.get(row.name).expect("remote name field");
        let address = inputs.get(row.address).expect("remote address field");
        if !filter.is_changed() && !row.is_added() && !name.is_changed() && !address.is_changed() {
            continue;
        }
        let name = name.value().to_string();
        let address = address.value().to_string();
        let title = if name.trim().is_empty() {
            address.trim()
        } else {
            name.trim()
        };
        let mut label = labels.get_mut(row.title).expect("remote row title");
        if label.0 != title {
            label.0 = title.to_owned();
        }
        let matches = needle.is_empty()
            || name.to_lowercase().contains(&needle)
            || address.to_lowercase().contains(&needle);
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
                    .any(|ancestor| ancestor == entity)
            {
                focus.set(filter_entity, FocusCause::Auto);
            }
        }
    }
}

fn remove_row(
    event: On<Activate>,
    buttons: Query<&RemoveServer>,
    add: Single<Entity, With<AddServer>>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    let row = buttons
        .get(event.entity)
        .expect("remove remote row button")
        .0;
    focus.set(*add, FocusCause::Auto);
    commands.entity(row).try_despawn();
}
