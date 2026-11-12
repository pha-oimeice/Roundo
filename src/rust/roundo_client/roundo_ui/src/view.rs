//! Keyed native widget projection. Snapshot changes update existing controls;
//! editing/focus entities are not rebuilt on each HUD or server-list publication.

use crate::{
    expression::{Context, boolean, set_model, text},
    registry::*,
    runtime::*,
    schema::*,
};
use bevy::{
    feathers::{controls::*, display::caption, font_styles::InheritableFont},
    input_focus::{InputFocus, tab_navigation::TabGroup},
    picking::{Pickable, events::PointerPress},
    prelude::*,
    text::{EditableText, FontSize, FontSource, TextEdit, TextEditChange},
    ui::{Checked, InteractionDisabled, Selected},
    ui_widgets::{
        Activate, ControlOrientation, ScrollArea, SetSelected, SliderRange, SliderStep,
        SliderValue, ValueChange,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Component, Default)]
struct Tree(BTreeMap<String, Entity>);
#[derive(Component, Clone)]
struct Element {
    instance: Entity,
    definition: Arc<NodeDef>,
    locals: Arc<BTreeMap<String, Value>>,
    caption: Option<Entity>,
    editor: Option<Entity>,
    external_value: Option<Value>,
    native_node: Node,
    external_layout: Option<Value>,
    options: Value,
    enabled: bool,
}
#[derive(Component, Clone, Default)]
struct Caption;
#[derive(Component, Clone)]
struct OptionValue(Value);
#[derive(Component)]
struct ProgrammaticText(String);
#[derive(Default, Resource)]
pub(crate) struct UiEvents(Vec<Edit>);
struct Edit {
    entity: Entity,
    kind: EditKind,
}
enum EditKind {
    Activate,
    Change(Value, bool),
    Text(String),
    Focus,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<UiEvents>()
        .add_observer(on_activate)
        .add_observer(on_bool)
        .add_observer(on_f32)
        .add_observer(on_f64)
        .add_observer(on_i64)
        .add_observer(on_select)
        .add_observer(on_text)
        .add_observer(on_pointer);
}
fn on_activate(event: On<Activate>, mut queue: ResMut<UiEvents>) {
    queue.0.push(Edit {
        entity: event.entity,
        kind: EditKind::Activate,
    });
}
fn on_bool(event: On<ValueChange<bool>>, mut queue: ResMut<UiEvents>) {
    queue.0.push(Edit {
        entity: event.source,
        kind: EditKind::Change(json!(event.value), event.is_final),
    });
}
fn on_f32(event: On<ValueChange<f32>>, mut queue: ResMut<UiEvents>) {
    queue.0.push(Edit {
        entity: event.source,
        kind: EditKind::Change(json!(event.value), event.is_final),
    });
}
fn on_f64(event: On<ValueChange<f64>>, mut queue: ResMut<UiEvents>) {
    queue.0.push(Edit {
        entity: event.source,
        kind: EditKind::Change(json!(event.value), event.is_final),
    });
}
fn on_i64(event: On<ValueChange<i64>>, mut queue: ResMut<UiEvents>) {
    queue.0.push(Edit {
        entity: event.source,
        kind: EditKind::Change(json!(event.value), event.is_final),
    });
}
fn on_select(
    event: On<ValueChange<Entity>>,
    options: Query<&OptionValue>,
    mut queue: ResMut<UiEvents>,
) {
    if let Ok(value) = options.get(event.value) {
        queue.0.push(Edit {
            entity: event.source,
            kind: EditKind::Change(value.0.clone(), event.is_final),
        });
    }
}
fn on_text(
    event: On<TextEditChange>,
    editors: Query<(&EditableText, Option<&ProgrammaticText>)>,
    focus: Res<InputFocus>,
    mut queue: ResMut<UiEvents>,
    mut commands: Commands,
) {
    let entity = event.event_target();
    let Ok((editor, programmatic)) = editors.get(entity) else {
        return;
    };
    if let Some(expected) = programmatic {
        commands.entity(entity).remove::<ProgrammaticText>();
        if editor.value() == expected.0.as_str() {
            return;
        }
    }
    if focus.get() == Some(entity) {
        queue.0.push(Edit {
            entity,
            kind: EditKind::Text(editor.value().to_string()),
        });
    }
}
fn on_pointer(event: On<PointerPress>, mut queue: ResMut<UiEvents>) {
    // Observe each original target once, not once for every propagated ancestor.
    if event.entity == event.original_event_target() {
        queue.0.push(Edit {
            entity: event.entity,
            kind: EditKind::Focus,
        });
    }
}

pub(crate) fn owner(world: &World, mut entity: Entity) -> Option<Entity> {
    loop {
        if let Some(element) = world.get::<Element>(entity) {
            return Some(element.instance);
        }
        if let Some(view) = world.get::<UiViewOf>(entity) {
            return Some(view.0);
        }
        entity = world.get::<ChildOf>(entity)?.parent();
    }
}
fn element_at(world: &World, mut entity: Entity) -> Option<(Entity, Element)> {
    loop {
        if let Some(element) = world.get::<Element>(entity) {
            return Some((entity, element.clone()));
        }
        entity = world.get::<ChildOf>(entity)?.parent();
    }
}
pub fn text_edit_has_focus(world: &World) -> bool {
    let Some(mut entity) = world.resource::<InputFocus>().get() else {
        return false;
    };
    loop {
        if world.get::<EditableText>(entity).is_some()
            || world.get::<FeathersNumberInput>(entity).is_some()
        {
            return true;
        }
        let Some(parent) = world.get::<ChildOf>(entity) else {
            return false;
        };
        entity = parent.parent();
    }
}
pub(crate) fn invalidate_values(world: &mut World, instance: Entity) {
    for mut element in world.query::<&mut Element>().iter_mut(world) {
        if element.instance == instance {
            element.external_value = None;
        }
    }
}
pub(crate) fn clear_widget_focus(world: &mut World, instance: Entity) {
    if world
        .resource::<InputFocus>()
        .get()
        .is_some_and(|e| owner(world, e) == Some(instance))
    {
        world.resource_mut::<InputFocus>().clear();
    }
}

pub(crate) fn ingress(world: &mut World) {
    for edit in std::mem::take(&mut world.resource_mut::<UiEvents>().0) {
        let Some((entity, element)) = element_at(world, edit.entity) else {
            continue;
        };
        let live = UiLifecycleManager::with(world, |ui| {
            ui.command_source_is_live(UiInstanceId::from_entity(element.instance))
        });
        if !live
            || !world
                .get::<UiInstance>(element.instance)
                .is_some_and(|v| v.visible)
            || !element.enabled
        {
            continue;
        }
        if matches!(edit.kind, EditKind::Focus) {
            UiLifecycleManager::with(world, |ui| {
                ui.focus(UiInstanceId::from_entity(element.instance))
            });
            continue;
        }
        if world
            .get::<UiInstance>(element.instance)
            .unwrap()
            .task
            .is_some()
        {
            continue;
        }
        let (value, actions, final_edit) = match edit.kind {
            EditKind::Activate => (Value::Null, element.definition.on_activate.clone(), true),
            EditKind::Change(value, final_edit) => {
                (value, element.definition.on_change.clone(), final_edit)
            }
            EditKind::Text(value) if element.definition.kind == Widget::TextInput => {
                (json!(value), element.definition.on_change.clone(), true)
            }
            _ => continue,
        };
        if !value.is_null() {
            if let Some(path) = &element.definition.bind {
                let result = set_model(
                    &mut world.get_mut::<UiInstance>(element.instance).unwrap().model,
                    path,
                    value.clone(),
                );
                if let Err(error) = result {
                    world.get_mut::<UiInstance>(element.instance).unwrap().error = error;
                    continue;
                }
            }
            if let Err(error) = apply_value(world, entity, &element, &value) {
                world.get_mut::<UiInstance>(element.instance).unwrap().error = error;
                continue;
            }
        }
        // Numeric drag previews update the widget/model, but invoke mutations only
        // on final edits. This prevents a drag from flooding the command queue.
        if final_edit {
            start_task(
                world,
                element.instance,
                actions,
                (*element.locals).clone(),
                value,
            );
        }
    }
}

pub(crate) fn mount(
    world: &mut World,
    instance: Entity,
    definition: &UiResource,
) -> Result<(), String> {
    let mut node = Node {
        position_type: PositionType::Absolute,
        ..default()
    };
    match definition.layout {
        UiLayout::Fullscreen => {
            node.width = percent(100);
            node.height = percent(100);
        }
        UiLayout::Windowed { width, height } => {
            node.width = px(width as f32);
            node.height = px(height as f32);
            node.left = percent(50);
            node.top = percent(50);
        }
    }
    let view = world
        .spawn((
            node,
            UiViewOf(instance),
            Tree::default(),
            TabGroup::default(),
            BackgroundColor(color(definition.background_color)),
            GlobalZIndex(world.get::<UiInstance>(instance).unwrap().z),
        ))
        .id();
    if definition.interaction_mode == ClientInteractionMode::InGame {
        world.entity_mut(view).insert(Pickable::IGNORE);
    }
    if matches!(definition.layout, UiLayout::Windowed { .. }) {
        world
            .entity_mut(view)
            .insert(UiTransform::from_translation(Val2::percent(-50., -50.)));
    }
    reconcile(world, instance, view, definition)
}

pub(crate) fn project(world: &mut World) {
    project_roots(world);
    let views = world
        .query::<(Entity, &UiViewOf)>()
        .iter(world)
        .map(|(e, v)| (e, v.0))
        .collect::<Vec<_>>();
    for (view, instance) in views {
        let Some(state) = world.get::<UiInstance>(instance) else {
            continue;
        };
        if !state.visible {
            continue;
        }
        let definition = world
            .resource::<UiRuntime>()
            .registry
            .resource(&state.definition)
            .unwrap()
            .clone();
        if let Err(error) = reconcile(world, instance, view, &definition) {
            // A broken schema/data contract must not leave an apparently working
            // interactive surface accepting arbitrary operations.
            log::error!(
                "native UI projection failed for {}: {error}",
                definition.name
            );
            world.write_message(bevy::app::AppExit::error());
        }
    }
}
pub(crate) fn project_roots(world: &mut World) {
    let views = world
        .query::<(Entity, &UiViewOf)>()
        .iter(world)
        .map(|(e, v)| (e, v.0))
        .collect::<Vec<_>>();
    for (view, instance) in views {
        let Some(instance) = world.get::<UiInstance>(instance) else {
            continue;
        };
        let visible = instance.visible;
        let z = instance.z;
        world.get_mut::<Node>(view).unwrap().display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        world.entity_mut(view).insert(GlobalZIndex(z));
    }
}

fn reconcile(
    world: &mut World,
    instance: Entity,
    view: Entity,
    definition: &UiResource,
) -> Result<(), String> {
    let mut tree = std::mem::take(&mut world.get_mut::<Tree>(view).unwrap().0);
    let mut desired = BTreeSet::new();
    let mut ordering = BTreeMap::new();
    let data = data_context(world, definition);
    let result = sync_nodes(
        world,
        instance,
        view,
        "",
        &definition.page.nodes,
        &BTreeMap::new(),
        definition,
        &data,
        true,
        &mut tree,
        &mut desired,
        &mut ordering,
    );
    let removed = tree
        .keys()
        .filter(|key| !desired.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for key in removed {
        let entity = tree.remove(&key).unwrap();
        if world.get_entity(entity).is_ok() {
            world.despawn(entity);
        }
    }
    if result.is_ok() {
        for (parent, children) in ordering {
            let current = world
                .get::<Children>(parent)
                .map(|c| c.iter().collect::<Vec<_>>())
                .unwrap_or_default();
            let mut ordered = current
                .iter()
                .copied()
                .filter(|e| world.get::<Element>(*e).is_none())
                .collect::<Vec<_>>();
            ordered.extend(children);
            if current != ordered {
                world.entity_mut(parent).replace_children(&ordered);
            }
        }
    }
    world.get_mut::<Tree>(view).unwrap().0 = tree;
    let focus = world.resource::<InputFocus>().get();
    if focus.is_some_and(|entity| world.get_entity(entity).is_err()) {
        world.resource_mut::<InputFocus>().clear();
    }
    result
}

fn sync_nodes(
    world: &mut World,
    instance: Entity,
    parent: Entity,
    prefix: &str,
    nodes: &[NodeDef],
    locals: &BTreeMap<String, Value>,
    resource: &UiResource,
    data: &Value,
    parent_enabled: bool,
    tree: &mut BTreeMap<String, Entity>,
    desired: &mut BTreeSet<String>,
    ordering: &mut BTreeMap<Entity, Vec<Entity>>,
) -> Result<(), String> {
    for def in nodes {
        let path = format!("{prefix}/{}", def.id);
        let state = world.get::<UiInstance>(instance).unwrap();
        let context = Context {
            model: &state.model,
            data,
            locals,
            event: &Value::Null,
            result: &Value::Null,
            error: &state.error,
            busy: state.task.is_some(),
        };
        let visible = def
            .visible
            .as_ref()
            .map(|v| context.eval(v).and_then(|v| boolean(&v)))
            .transpose()?
            .unwrap_or(true);
        if !visible {
            continue;
        }
        let enabled = parent_enabled
            && resource.interaction_mode == ClientInteractionMode::Ui
            && state.task.is_none()
            && def
                .enabled
                .as_ref()
                .map(|v| context.eval(v).and_then(|v| boolean(&v)))
                .transpose()?
                .unwrap_or(true);
        let label = def
            .text
            .as_ref()
            .map(|v| context.eval(v).map(|v| text(&v)))
            .transpose()?;
        let value = def.value.as_ref().map(|v| context.eval(v)).transpose()?;
        // No placeholder zero/empty editor is invented for an absent snapshot.
        if value.as_ref().is_some_and(Value::is_null) {
            continue;
        }
        let options = def
            .options
            .as_ref()
            .map(|v| context.eval(v))
            .transpose()?
            .unwrap_or(Value::Null);
        let items = def.items.as_ref().map(|v| context.eval(v)).transpose()?;
        let min = def.min.as_ref().map(|v| context.eval(v)).transpose()?;
        let max = def.max.as_ref().map(|v| context.eval(v)).transpose()?;
        let step = def.step.as_ref().map(|v| context.eval(v)).transpose()?;
        let layout = def
            .style
            .as_ref()
            .map(|style| context.eval(&json!(style.node)))
            .transpose()?;
        let mut entity = tree
            .get(&path)
            .copied()
            .filter(|e| world.get::<Element>(*e).is_some());
        if let Some(old) = entity
            && def.kind == Widget::Select
            && world.get::<Element>(old).unwrap().options != options
        {
            world.despawn(old);
            entity = None;
        }
        let entity = match entity {
            Some(entity) => entity,
            None => {
                let entity = spawn_widget(
                    world,
                    parent,
                    def,
                    label.as_deref(),
                    &options,
                    value.as_ref(),
                    min.as_ref(),
                    max.as_ref(),
                )?;
                let caption = descendants(world, entity)
                    .into_iter()
                    .find(|e| world.get::<Caption>(*e).is_some());
                let editor = descendants(world, entity)
                    .into_iter()
                    .find(|e| world.get::<EditableText>(*e).is_some());
                let native_node = world.get::<Node>(entity).unwrap().clone();
                world.entity_mut(entity).insert(Element {
                    instance,
                    definition: Arc::new(def.clone()),
                    locals: Arc::new(locals.clone()),
                    caption,
                    editor,
                    external_value: None,
                    native_node,
                    external_layout: None,
                    options: options.clone(),
                    enabled,
                });
                set_font(world, entity, resource, def.style.as_ref());
                tree.insert(path.clone(), entity);
                entity
            }
        };
        desired.insert(path.clone());
        ordering.entry(parent).or_default().push(entity);
        let mut element = world.get::<Element>(entity).unwrap().clone();
        if element.locals.as_ref() != locals {
            element.locals = Arc::new(locals.clone());
        }
        element.enabled = enabled;
        if let Some(layout) = layout
            && element.external_layout.as_ref() != Some(&layout)
        {
            let node = crate::layout::resolve(&element.native_node, &layout)
                .map_err(|error| format!("{path}: {error}"))?;
            if world.get::<Node>(entity) != Some(&node) {
                world.entity_mut(entity).insert(node);
            }
            element.external_layout = Some(layout);
        }
        if let (Some(caption), Some(label)) = (element.caption, label)
            && let Some(mut t) = world.get_mut::<Text>(caption)
            && t.0 != label
        {
            t.0 = label;
        }
        set_disabled(world, entity, &element, !enabled);
        if let Some(value) = value
            && element.external_value.as_ref() != Some(&value)
        {
            if !value.is_null() {
                apply_value(world, entity, &element, &value)?;
            }
            element.external_value = Some(value);
        }
        if def.kind == Widget::NumberInput {
            let min = min.as_ref().filter(|v| !v.is_null());
            let max = max.as_ref().filter(|v| !v.is_null());
            if def.integer == Some(true) {
                let lower = min.map(integer).transpose()?.unwrap_or(i64::MIN);
                let upper = max.map(integer).transpose()?.unwrap_or(i64::MAX);
                if lower > upper {
                    return Err(format!("{path}: inverted integer limits"));
                }
                world
                    .entity_mut(entity)
                    .insert(HardLimit::i64(lower..=upper));
            } else {
                let lower = min.map(number).transpose()?.unwrap_or(f64::NEG_INFINITY);
                let upper = max.map(number).transpose()?.unwrap_or(f64::INFINITY);
                if lower > upper {
                    return Err(format!("{path}: inverted numeric limits"));
                }
                world
                    .entity_mut(entity)
                    .insert(HardLimit::f64(lower..=upper));
            }
            if let Some(step) = step.as_ref().filter(|v| !v.is_null()) {
                let step = number(step)?;
                if step <= 0.0 {
                    return Err(format!("{path}: number step must be positive"));
                }
                world.entity_mut(entity).insert(NumberInputStep(step));
            }
        }
        if def.kind == Widget::Slider
            && let (Some(min), Some(max)) = (min.as_ref(), max.as_ref())
        {
            if !min.is_null() && !max.is_null() {
                world
                    .entity_mut(entity)
                    .insert(SliderRange::new(number(min)? as f32, number(max)? as f32));
            }
            if let Some(step) = step.as_ref().filter(|v| !v.is_null()) {
                world
                    .entity_mut(entity)
                    .insert(SliderStep(number(step)? as f32));
            }
        }
        world.entity_mut(entity).insert(element);
        if def.kind == Widget::Repeat {
            let items = items.unwrap();
            if items.is_null() {
                continue;
            }
            let items = items
                .as_array()
                .ok_or_else(|| format!("{path}: repeat items is not array"))?;
            let mut keys = BTreeSet::new();
            for (index, item) in items.iter().enumerate() {
                let mut locals = locals.clone();
                locals.insert(def.alias.clone().unwrap(), item.clone());
                locals.insert("index".into(), json!(index));
                let state = world.get::<UiInstance>(instance).unwrap();
                let context = Context {
                    model: &state.model,
                    data,
                    locals: &locals,
                    event: &Value::Null,
                    result: &Value::Null,
                    error: &state.error,
                    busy: state.task.is_some(),
                };
                let key = context.eval(def.key.as_ref().unwrap())?;
                if !matches!(key, Value::String(_) | Value::Number(_)) {
                    return Err(format!("{path}: repeat key must be text or number"));
                }
                let key = key.to_string();
                if !keys.insert(key.clone()) {
                    return Err(format!("{path}: duplicate repeat key {key}"));
                }
                // Length-prefixed keys cannot collide with schema paths or nested keys.
                let row = format!("{path}/{}:{key}", key.len());
                sync_nodes(
                    world,
                    instance,
                    entity,
                    &row,
                    &def.children,
                    &locals,
                    resource,
                    data,
                    enabled,
                    tree,
                    desired,
                    ordering,
                )?;
            }
        } else {
            sync_nodes(
                world,
                instance,
                entity,
                &path,
                &def.children,
                locals,
                resource,
                data,
                enabled,
                tree,
                desired,
                ordering,
            )?;
        }
    }
    Ok(())
}

fn spawn_widget(
    world: &mut World,
    parent: Entity,
    def: &NodeDef,
    label: Option<&str>,
    options: &Value,
    value: Option<&Value>,
    min: Option<&Value>,
    max: Option<&Value>,
) -> Result<Entity, String> {
    let label = label.unwrap_or("").to_owned();
    let entity = match def.kind {
        Widget::Button => world
            .spawn_scene(bsn! { @FeathersButton { @caption: bsn! { @caption(label) Caption } } })
            .map_err(|e| e.to_string())?
            .id(),
        Widget::Checkbox => world
            .spawn_scene(bsn! { @FeathersCheckbox { @caption: bsn! { @caption(label) Caption } } })
            .map_err(|e| e.to_string())?
            .id(),
        Widget::NumberInput => world
            .spawn_scene(bsn! { @FeathersNumberInput })
            .map_err(|e| e.to_string())?
            .id(),
        Widget::Slider => {
            let min = number(min.ok_or("slider min missing")?)? as f32;
            let max = number(max.ok_or("slider max missing")?)? as f32;
            if min >= max {
                return Err("slider requires min < max".into());
            }
            world
                .spawn_scene(bsn! { @FeathersSlider { @min: min, @max: max } })
                .map_err(|e| e.to_string())?
                .id()
        }
        Widget::TextInput => world
            .spawn_scene(bsn! { @FeathersTextInputContainer Children [ @FeathersTextInput ] })
            .map_err(|e| e.to_string())?
            .id(),
        Widget::Select => {
            let values = options
                .as_array()
                .ok_or("select options must be an array")?;
            let mut seen = BTreeSet::new();
            let rows = values.iter().map(|option| -> Result<Box<dyn bevy::scene::SceneList>, String> {
                let value = option.get("value").ok_or("select option has no value")?.clone();
                if !seen.insert(value.to_string()) { return Err("duplicate select option value".into()); }
                let label = option.get("label").and_then(Value::as_str).ok_or("select option has no text label")?.to_owned();
                Ok(Box::new(bsn! { @FeathersListRow ~{OptionValue(value)} Children [ @caption(label) ] }))
            }).collect::<Result<Vec<_>, _>>()?;
            let max_visible = def.max_visible.unwrap();
            let entity = world.spawn_scene(bsn! { @FeathersSelect { @options: {Box::new(rows) as Box<dyn bevy::scene::SceneList>}, @max_visible: max_visible } }).map_err(|e| e.to_string())?.id();
            if let Some(value) = value {
                select_value(world, entity, value)?;
            }
            entity
        }
        Widget::Text => world
            .spawn((Node::default(), Text::new(label), Caption))
            .id(),
        Widget::Container | Widget::Column | Widget::Row | Widget::Repeat | Widget::Scroll => {
            let mut node = Node::default();
            if def.kind != Widget::Container {
                node.flex_direction = if def.kind == Widget::Row {
                    FlexDirection::Row
                } else {
                    FlexDirection::Column
                };
            }
            let entity = world.spawn(node).id();
            if def.kind == Widget::Scroll {
                world.get_mut::<Node>(entity).unwrap().overflow = Overflow::scroll_y();
                world.entity_mut(entity).insert(ScrollArea);
                // Feather supplies thumb sizing, drag behavior and auto-hide.
                world.spawn_scene(bsn! { @FeathersScrollbar { @target: entity, @orientation: ControlOrientation::Vertical }
                    Node { position_type: PositionType::Absolute, right: px(0), top: px(0), height: percent(100), width: {px(def.scrollbar_width.unwrap())} }
                    ChildOf(entity)
                }).map_err(|e| e.to_string())?;
            }
            entity
        }
    };
    world.entity_mut(entity).insert(ChildOf(parent));
    if let Some(background) = def.style.as_ref().and_then(|style| style.background) {
        world
            .entity_mut(entity)
            .insert(BackgroundColor(color(background)));
    }
    Ok(entity)
}

fn apply_value(
    world: &mut World,
    entity: Entity,
    element: &Element,
    value: &Value,
) -> Result<(), String> {
    match element.definition.kind {
        Widget::TextInput => {
            let string = value.as_str().ok_or("text-input value must be text")?;
            let editor = element.editor.ok_or("text-input lost its native editor")?;
            let mut edit = world
                .get_mut::<EditableText>(editor)
                .ok_or("native editor missing")?;
            if edit.value() != string {
                edit.queue_edit(TextEdit::SelectAll);
                edit.queue_edit(TextEdit::Insert(string.into()));
                drop(edit);
                world
                    .entity_mut(editor)
                    .insert(ProgrammaticText(string.into()));
            }
        }
        Widget::NumberInput => {
            let number = if element.definition.integer == Some(true) {
                NumberInputValue::I64(integer(value)?)
            } else {
                NumberInputValue::F64(number(value)?)
            };
            if world.get::<NumberInputValue>(entity) != Some(&number) {
                world.entity_mut(entity).insert(number);
            }
        }
        Widget::Checkbox => {
            if boolean(value)? {
                world.entity_mut(entity).insert(Checked);
            } else {
                world.entity_mut(entity).remove::<Checked>();
            }
        }
        Widget::Slider => {
            world
                .entity_mut(entity)
                .insert(SliderValue(number(value)? as f32));
        }
        Widget::Select => select_value(world, entity, value)?,
        _ => {}
    }
    Ok(())
}
fn select_value(world: &mut World, entity: Entity, value: &Value) -> Result<(), String> {
    let rows = descendants(world, entity)
        .into_iter()
        .filter(|e| world.get::<OptionValue>(*e).is_some())
        .collect::<Vec<_>>();
    let selected = rows
        .iter()
        .copied()
        .find(|e| &world.get::<OptionValue>(*e).unwrap().0 == value);
    for row in rows {
        if Some(row) == selected {
            world.entity_mut(row).insert(Selected);
        } else {
            world.entity_mut(row).remove::<Selected>();
        }
    }
    if let Some(row) = selected {
        world.trigger(SetSelected { entity, row });
    } else {
        // The select's native caption is outside every option row. Clear it
        // explicitly when a model reset removes selection but keeps the options.
        let caption = descendants(world, entity).into_iter().find(|e| {
            if world.get::<Text>(*e).is_none() {
                return false;
            }
            let mut parent = *e;
            while let Some(child_of) = world.get::<ChildOf>(parent) {
                parent = child_of.parent();
                if world.get::<OptionValue>(parent).is_some() {
                    return false;
                }
                if parent == entity {
                    return true;
                }
            }
            false
        });
        if let Some(caption) = caption {
            world.get_mut::<Text>(caption).unwrap().0.clear();
        }
    }
    Ok(())
}
fn set_disabled(world: &mut World, entity: Entity, element: &Element, disabled: bool) {
    for entity in std::iter::once(entity).chain(element.editor) {
        let current = world.get::<InteractionDisabled>(entity).is_some();
        if current != disabled {
            if disabled {
                world.entity_mut(entity).insert(InteractionDisabled);
            } else {
                world.entity_mut(entity).remove::<InteractionDisabled>();
            }
        }
    }
}
fn descendants(world: &World, entity: Entity) -> Vec<Entity> {
    let mut out = vec![entity];
    let mut i = 0;
    while i < out.len() {
        if let Some(children) = world.get::<Children>(out[i]) {
            out.extend(children.iter());
        }
        i += 1;
    }
    out
}
fn set_font(world: &mut World, entity: Entity, resource: &UiResource, style: Option<&Style>) {
    let runtime = world.resource::<UiRuntime>();
    let font = runtime.font.clone();
    let size = style
        .and_then(|s| s.font_size)
        .unwrap_or(runtime.registry.theme.font_size);
    let color_override = style.and_then(|s| s.color);
    for entity in descendants(world, entity) {
        if resource.interaction_mode == ClientInteractionMode::InGame {
            world.entity_mut(entity).insert(Pickable::IGNORE);
        }
        if let Some(mut text) = world.get_mut::<TextFont>(entity) {
            text.font = FontSource::Handle(font.clone());
            text.font_size = FontSize::Px(size);
        }
        if let Some(mut inherit) = world.get::<InheritableFont>(entity).cloned() {
            inherit.font = font.clone();
            inherit.font_size = FontSize::Px(size);
            // Feathers refreshes Propagate<TextFont> on Insert, not field mutation.
            world.entity_mut(entity).insert(inherit);
        }
        if world.get::<Text>(entity).is_some()
            && let Some(value) = color_override
        {
            world.entity_mut(entity).insert(TextColor(color(value)));
        }
    }
}
fn color(v: [u8; 4]) -> Color {
    Color::srgba_u8(v[0], v[1], v[2], v[3])
}
fn number(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("expected finite numeric UI value, got {v}"))
}
fn integer(v: &Value) -> Result<i64, String> {
    v.as_i64()
        .ok_or_else(|| format!("expected integer UI value, got {v}"))
}
