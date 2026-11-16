//! Main-world native view ownership and actions. ChildOf owns logical navigation;
//! a linked relationship owns each independent UI render root and its widget tree.

use crate::{
    expression::{Context, boolean, set_model},
    registry::*,
    schema::Action,
};
use bevy::{input_focus::InputFocus, prelude::*};
use roundo_scene::{SceneKind, SceneRoot, scene_root};
use roundo_toolbox::request_response_pipe::{
    ContextualJsonRequestResponseIo, RequestCall, ResponsePoll,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    time::{Duration, Instant},
};

pub type UiCommandIo = ContextualJsonRequestResponseIo<UiCommandSource, Value>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct UiInstanceId(Entity);
impl UiInstanceId {
    /// Names an ECS instance; liveness and scene ownership are checked separately.
    pub fn from_entity(entity: Entity) -> Self {
        Self(entity)
    }
    pub fn entity(self) -> Entity {
        self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiCommandSource {
    Host,
    Native(UiInstanceId),
}
#[derive(Clone, Debug)]
pub struct UiOpenTarget(pub(crate) String);
impl UiOpenTarget {
    pub fn resource(&self) -> &str {
        &self.0
    }
}
#[derive(Debug)]
pub enum UiLifecycleError {
    Registry(String),
    UiInstanceLimit,
    UnknownImport(String),
    StaleUiInstance,
    CannotCloseUiRoot,
    Presentation(String),
}
impl fmt::Display for UiLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(e) => write!(f, "invalid native UI resource: {e}"),
            Self::UiInstanceLimit => write!(f, "UI instance limit reached"),
            Self::UnknownImport(e) => write!(f, "unknown UI import `{e}`"),
            Self::StaleUiInstance => write!(f, "UI instance is no longer live"),
            Self::CannotCloseUiRoot => write!(f, "cannot close the UI scene root"),
            Self::Presentation(e) => write!(f, "native UI presentation failed: {e}"),
        }
    }
}
impl std::error::Error for UiLifecycleError {}

#[derive(Component)]
pub(crate) struct UiInstance {
    pub definition: String,
    // Materialized instance data, never registered with ModManager.
    pub resource: std::sync::Arc<UiResource>,
    pub root_ui: bool,
    pub visible: bool,
    pub z: i32,
    pub model: Value,
    pub error: String,
    pub task: Option<Task>,
    pub session: Option<Value>,
    observed: BTreeMap<String, Value>,
}
#[derive(Component)]
#[relationship(relationship_target = UiViews)]
pub(crate) struct UiViewOf(pub Entity);
#[derive(Component, Default)]
#[relationship_target(relationship = UiViewOf, linked_spawn)]
pub(crate) struct UiViews(Vec<Entity>);

#[derive(Resource)]
pub(crate) struct UiRuntime {
    pub registry: UiRegistry,
    pub root: Entity,
    pub focus: Vec<UiInstanceId>,
    pub next_z: i32,
    pub io: UiCommandIo,
    pub font: Handle<Font>,
}

/// Direct ECS snapshot store. Counts are rebuilt from live views, so destroying
/// a view immediately ends its subscriptions without a transport or lease table.
#[derive(Resource, Default)]
pub struct UiData {
    snapshots: BTreeMap<String, Value>,
    subscribers: BTreeMap<String, usize>,
    generation: u64,
}
impl UiData {
    pub fn data_subscription_generation(&self) -> u64 {
        self.generation
    }
    pub fn has_data_subscribers(&self, name: &str) -> bool {
        self.subscribers.contains_key(name)
    }
    pub fn publish_data(&mut self, name: &str, value: &Value) -> usize {
        let count = self.subscribers.get(name).copied().unwrap_or(0);
        if count != 0 && self.snapshots.get(name) != Some(value) {
            self.snapshots.insert(name.into(), value.clone());
        }
        count
    }
    pub(crate) fn snapshot(&self, name: &str) -> Value {
        self.snapshots.get(name).cloned().unwrap_or(Value::Null)
    }
}

pub struct UiLifecycleManager<'w> {
    pub world: &'w mut World,
}
impl UiLifecycleManager<'_> {
    pub fn is_installed(world: &World) -> bool {
        world.contains_resource::<UiRuntime>()
    }
    pub fn with<R>(world: &mut World, f: impl FnOnce(&mut UiLifecycleManager<'_>) -> R) -> R {
        f(&mut UiLifecycleManager { world })
    }
    pub(crate) fn registry(&self) -> &UiRegistry {
        &self.world.resource::<UiRuntime>().registry
    }
    pub fn root_entity(&self) -> Entity {
        self.world.resource::<UiRuntime>().root
    }
    pub fn root_scene(&self) -> SceneKind {
        self.world
            .get::<SceneRoot>(self.root_entity())
            .expect("live scene root")
            .kind
    }
    fn belongs(&self, mut entity: Entity) -> bool {
        while let Some(parent) = self.world.get::<ChildOf>(entity) {
            entity = parent.parent();
            if entity == self.root_entity() {
                return true;
            }
        }
        false
    }
    pub fn command_source_is_live(&self, id: UiInstanceId) -> bool {
        self.world.get::<UiInstance>(id.0).is_some() && self.belongs(id.0)
    }
    pub fn is_root(&self, id: UiInstanceId) -> bool {
        self.world
            .get::<UiInstance>(id.0)
            .is_some_and(|v| v.root_ui)
            && self.belongs(id.0)
    }
    pub fn focused_instance(&self) -> Option<UiInstanceId> {
        self.world
            .resource::<UiRuntime>()
            .focus
            .iter()
            .rev()
            .find(|id| {
                self.world
                    .get::<UiInstance>(id.0)
                    .is_some_and(|v| v.visible)
                    && self.belongs(id.0)
            })
            .copied()
    }
    fn instances(&self) -> Vec<Entity> {
        self.world
            .try_query::<(Entity, &UiInstance)>()
            .map(|mut q| {
                q.iter(self.world)
                    .map(|(e, _)| e)
                    .filter(|e| self.belongs(*e))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn event_target(&self) -> Option<UiInstanceId> {
        self.focused_instance().or_else(|| {
            self.instances()
                .into_iter()
                .filter(|e| self.world.get::<UiInstance>(*e).unwrap().visible)
                .max_by_key(|e| self.world.get::<UiInstance>(*e).unwrap().z)
                .map(UiInstanceId)
        })
    }
    pub fn blocks_gameplay_input(&self) -> bool {
        self.instances().into_iter().any(|e| {
            self.registry()
                .resource(&self.world.get::<UiInstance>(e).unwrap().definition)
                .unwrap()
                .interaction_mode
                == ClientInteractionMode::Ui
        })
    }
    pub fn world_visible(&self) -> bool {
        self.instances()
            .into_iter()
            .filter(|e| self.world.get::<UiInstance>(*e).unwrap().visible)
            .max_by_key(|e| self.world.get::<UiInstance>(*e).unwrap().z)
            .is_some_and(|e| {
                self.registry()
                    .resource(&self.world.get::<UiInstance>(e).unwrap().definition)
                    .unwrap()
                    .world_visibility
                    == UiWorldVisibility::Visible
            })
    }
    pub fn resolve_import(
        &self,
        source: UiInstanceId,
        handle: &str,
    ) -> Result<UiOpenTarget, UiLifecycleError> {
        if !self.command_source_is_live(source) {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        let instance = self.world.get::<UiInstance>(source.0).unwrap();
        self.registry()
            .resource(&instance.definition)
            .and_then(|r| r.imports.get(handle))
            .cloned()
            .map(UiOpenTarget)
            .ok_or_else(|| UiLifecycleError::UnknownImport(handle.into()))
    }
    pub fn resolve_slot(&self, slot: &str) -> Result<UiOpenTarget, UiLifecycleError> {
        self.registry()
            .slot_name(slot)
            .map(|name| UiOpenTarget(name.to_owned()))
            .ok_or_else(|| UiLifecycleError::Registry(format!("unknown slot {slot}")))
    }
    pub fn navigate(
        &mut self,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let parent = match source {
            UiCommandSource::Host => self.root_entity(),
            UiCommandSource::Native(id) if self.command_source_is_live(id) => id.0,
            _ => return Err(UiLifecycleError::StaleUiInstance),
        };
        let definition = self
            .registry()
            .resource(target.resource())
            .ok_or_else(|| UiLifecycleError::Registry(target.0.clone()))?;
        if self
            .instances()
            .iter()
            .filter(|e| self.world.get::<UiInstance>(**e).unwrap().definition == target.0)
            .count()
            >= definition.max_instances as usize
        {
            return Err(UiLifecycleError::UiInstanceLimit);
        }
        let id = self.spawn(parent, &target.0, false)?;
        self.recompute();
        Ok(id)
    }
    fn spawn(
        &mut self,
        parent: Entity,
        name: &str,
        root_ui: bool,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let definition = std::sync::Arc::new(
            self.registry()
                .materialize(name)
                .map_err(UiLifecycleError::Registry)?,
        );
        let z = self
            .world
            .resource::<UiRuntime>()
            .next_z
            .checked_add(1)
            .ok_or_else(|| UiLifecycleError::Presentation("stacking identity exhausted".into()))?;
        let model = serde_json::to_value(&definition.page.model)
            .map_err(|e| UiLifecycleError::Registry(e.to_string()))?;
        let session = definition
            .page
            .session_source
            .as_ref()
            .and_then(|alias| definition.page.sources.get(alias))
            .map(|name| self.world.resource::<UiData>().snapshot(name)["epoch"].clone());
        let id = self
            .world
            .spawn((
                UiInstance {
                    definition: name.into(),
                    resource: std::sync::Arc::clone(&definition),
                    root_ui,
                    visible: true,
                    z,
                    model,
                    error: String::new(),
                    task: None,
                    session,
                    observed: BTreeMap::new(),
                },
                ChildOf(parent),
            ))
            .id();
        if let Err(error) = crate::view::mount(self.world, id, &definition) {
            self.world.despawn(id);
            return Err(UiLifecycleError::Presentation(error));
        }
        self.world.resource_mut::<UiRuntime>().next_z = z;
        if definition.interaction_mode == ClientInteractionMode::Ui {
            self.world
                .resource_mut::<UiRuntime>()
                .focus
                .push(UiInstanceId(id));
        }
        if !definition.page.on_open.is_empty() {
            self.world.get_mut::<UiInstance>(id).unwrap().task = Some(Task::new(
                definition.page.on_open.clone(),
                BTreeMap::new(),
                Value::Null,
            ));
        }
        Ok(UiInstanceId(id))
    }
    pub fn replace_root(&mut self, scene: SceneKind) -> Result<(), UiLifecycleError> {
        if self.root_scene() == scene
            && self
                .instances()
                .iter()
                .any(|e| self.world.get::<UiInstance>(*e).unwrap().root_ui)
        {
            return Ok(());
        }
        let slot = match scene {
            SceneKind::Menu => MENU_ROOT_SLOT,
            SceneKind::InGame => IN_GAME_ROOT_SLOT,
        };
        let target = self.resolve_slot(slot)?;
        let staging = self
            .world
            .spawn_scene(scene_root(scene))
            .map_err(|e| UiLifecycleError::Presentation(e.to_string()))?
            .id();
        if let Err(error) = self.spawn(staging, target.resource(), true) {
            self.world.despawn(staging);
            return Err(error);
        }
        let previous = self.root_entity();
        self.world.resource_mut::<UiRuntime>().root = staging;
        self.world.despawn(previous);
        self.recompute();
        Ok(())
    }
    pub fn back(&mut self, id: UiInstanceId) -> Result<(), UiLifecycleError> {
        if !self.command_source_is_live(id) {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        if self.is_root(id) {
            return Err(UiLifecycleError::CannotCloseUiRoot);
        }
        let parent = self.world.get::<ChildOf>(id.0).unwrap().parent();
        self.preserve_independent(id.0, parent);
        self.world.despawn(id.0);
        self.recompute();
        Ok(())
    }
    fn preserve_independent(&mut self, node: Entity, survivor: Entity) {
        let children = self
            .world
            .get::<Children>(node)
            .map(|c| c.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children {
            let independent = self
                .world
                .get::<UiInstance>(child)
                .and_then(|v| self.registry().resource(&v.definition))
                .is_some_and(|r| r.lifecycle_independent);
            if independent {
                self.world.entity_mut(child).insert(ChildOf(survivor));
            } else {
                self.preserve_independent(child, survivor);
            }
        }
    }
    pub fn focus(&mut self, id: UiInstanceId) {
        if !self.command_source_is_live(id) || !self.world.get::<UiInstance>(id.0).unwrap().visible
        {
            return;
        }
        let definition = &self.world.get::<UiInstance>(id.0).unwrap().definition;
        if self
            .registry()
            .resource(definition)
            .unwrap()
            .interaction_mode
            != ClientInteractionMode::Ui
        {
            return;
        }
        let mut runtime = self.world.resource_mut::<UiRuntime>();
        runtime.focus.retain(|v| *v != id);
        runtime.focus.push(id);
        drop(runtime);
        self.recompute();
    }
    pub fn dispatch_function(&mut self, target: UiInstanceId, function: &str) {
        if self.event_target() != Some(target) {
            return;
        }
        let Some(instance) = self.world.get::<UiInstance>(target.0) else {
            return;
        };
        let actions = instance.resource.page.shortcuts.get(function).cloned();
        if let Some(actions) = actions {
            start_task(self.world, target.0, actions, BTreeMap::new(), Value::Null);
        }
    }
    fn recompute(&mut self) {
        let instances = self.instances();
        let live = instances.iter().copied().collect::<BTreeSet<_>>();
        self.world
            .resource_mut::<UiRuntime>()
            .focus
            .retain(|id| live.contains(&id.0));
        for e in &instances {
            self.world.get_mut::<UiInstance>(*e).unwrap().visible = true;
        }
        let parents = std::iter::once(self.root_entity())
            .chain(instances.iter().copied())
            .collect::<Vec<_>>();
        let focus = self.world.resource::<UiRuntime>().focus.clone();
        for parent in parents {
            let children = self
                .world
                .get::<Children>(parent)
                .map(|c| c.iter().filter(|e| live.contains(e)).collect::<Vec<_>>())
                .unwrap_or_default();
            let winner = children
                .iter()
                .copied()
                .filter(|e| {
                    self.registry()
                        .resource(&self.world.get::<UiInstance>(*e).unwrap().definition)
                        .unwrap()
                        .presentation
                        == PresentationMode::Exclusive
                })
                .max_by_key(|e| {
                    (
                        focus.iter().position(|id| id.0 == *e),
                        self.world.get::<UiInstance>(*e).unwrap().z,
                    )
                });
            if let Some(winner) = winner {
                if let Some(mut instance) = self.world.get_mut::<UiInstance>(parent) {
                    instance.visible = false;
                }
                for sibling in children {
                    if sibling != winner {
                        self.hide(sibling);
                    }
                }
            }
        }
        // Independent render roots must follow the same ownership-aware order
        // as logical focus. Raising a descendant raises its whole branch.
        fn order(
            ui: &UiLifecycleManager,
            parent: Entity,
            focus: &[UiInstanceId],
            out: &mut Vec<Entity>,
        ) -> usize {
            let mut children = ui
                .world
                .get::<Children>(parent)
                .map(|c| {
                    c.iter()
                        .filter(|e| ui.world.get::<UiInstance>(*e).is_some())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut branches = Vec::new();
            for child in children.drain(..) {
                let mut branch = vec![child];
                let rank = order(ui, child, focus, &mut branch).max(
                    focus
                        .iter()
                        .position(|id| id.0 == child)
                        .map_or(0, |i| i + 1),
                );
                branches.push((rank, ui.world.get::<UiInstance>(child).unwrap().z, branch));
            }
            branches.sort_by_key(|(rank, z, _)| (*rank, *z));
            let highest = branches.iter().map(|(rank, _, _)| *rank).max().unwrap_or(0);
            for (_, _, branch) in branches {
                out.extend(branch);
            }
            highest
        }
        let mut stacking = Vec::new();
        order(self, self.root_entity(), &focus, &mut stacking);
        for (index, entity) in stacking.into_iter().enumerate() {
            self.world.get_mut::<UiInstance>(entity).unwrap().z =
                i32::try_from(index + 1).expect("validated instance count fits z ordering");
        }
        let mut counts = BTreeMap::new();
        for e in &instances {
            for source in self
                .world
                .get::<UiInstance>(*e)
                .unwrap()
                .resource
                .page
                .sources
                .values()
            {
                *counts.entry(source.clone()).or_insert(0) += 1;
            }
        }
        let mut data = self.world.resource_mut::<UiData>();
        if counts != data.subscribers {
            data.subscribers = counts;
            data.generation = data
                .generation
                .checked_add(1)
                .expect("UI subscription generation exhausted");
            let names = data.subscribers.keys().cloned().collect::<BTreeSet<_>>();
            data.snapshots.retain(|name, _| names.contains(name));
        }
        let focused = self.world.resource::<InputFocus>().get();
        if focused.is_some_and(|e| {
            crate::view::owner(self.world, e).is_none_or(|owner| {
                !self
                    .world
                    .get::<UiInstance>(owner)
                    .is_some_and(|v| v.visible)
            })
        }) {
            self.world.resource_mut::<InputFocus>().clear();
        }
        crate::view::project_roots(self.world);
    }
    fn hide(&mut self, e: Entity) {
        if let Some(mut v) = self.world.get_mut::<UiInstance>(e) {
            v.visible = false;
        }
        let children = self
            .world
            .get::<Children>(e)
            .map(|c| c.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children {
            self.hide(child);
        }
    }
}

pub(crate) struct Task {
    actions: VecDeque<Action>,
    locals: BTreeMap<String, Value>,
    event: Value,
    result: Value,
    waiting: Option<Waiting>,
    completed_steps: usize,
}
enum Waiting {
    Command {
        call: RequestCall<Value>,
        save: Option<String>,
        debug_epoch: Option<Value>,
    },
    Debug {
        request: String,
        epoch: Value,
        save: Option<String>,
        deadline: Instant,
    },
    Timer(Instant),
}
impl Task {
    fn new(actions: Vec<Action>, locals: BTreeMap<String, Value>, event: Value) -> Self {
        Self {
            actions: actions.into(),
            locals,
            event,
            result: Value::Null,
            waiting: None,
            completed_steps: 0,
        }
    }
}
pub(crate) fn start_task(
    world: &mut World,
    instance: Entity,
    actions: Vec<Action>,
    locals: BTreeMap<String, Value>,
    event: Value,
) {
    if actions.is_empty() {
        return;
    }
    let Some(mut view) = world.get_mut::<UiInstance>(instance) else {
        return;
    };
    if view.task.is_none() {
        view.error.clear();
        view.task = Some(Task::new(actions, locals, event));
    }
}
pub(crate) fn data_context(world: &World, definition: &UiResource) -> Value {
    Value::Object(
        definition
            .page
            .sources
            .iter()
            .map(|(alias, source)| (alias.clone(), world.resource::<UiData>().snapshot(source)))
            .collect(),
    )
}

pub(crate) fn advance(world: &mut World) {
    let instances = world
        .query::<(Entity, &UiInstance)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect::<Vec<_>>();
    for entity in instances {
        let name = world.get::<UiInstance>(entity).unwrap().definition.clone();
        let definition = std::sync::Arc::clone(&world.get::<UiInstance>(entity).unwrap().resource);
        if let Some(alias) = &definition.page.session_source {
            let epoch = world
                .resource::<UiData>()
                .snapshot(&definition.page.sources[alias])["epoch"]
                .clone();
            let changed = world.get::<UiInstance>(entity).unwrap().session.as_ref() != Some(&epoch);
            if changed {
                let mut instance = world.get_mut::<UiInstance>(entity).unwrap();
                instance.model =
                    serde_json::to_value(&definition.page.model).expect("serializable model");
                instance.task = None;
                instance.session = Some(epoch);
                instance.observed.clear();
                instance.error = "连接会话已改变；旧选择与草稿已清除。已接受操作的结果可能未知，请重新读取，勿自动重试。".into();
                drop(instance);
                crate::view::clear_widget_focus(world, entity);
            }
        }
        if world.get::<UiInstance>(entity).unwrap().task.is_none() {
            for (alias, actions) in &definition.page.on_data {
                let snapshot = world
                    .resource::<UiData>()
                    .snapshot(&definition.page.sources[alias]);
                if snapshot.is_null()
                    || world.get::<UiInstance>(entity).unwrap().observed.get(alias)
                        == Some(&snapshot)
                {
                    continue;
                }
                world
                    .get_mut::<UiInstance>(entity)
                    .unwrap()
                    .observed
                    .insert(alias.clone(), snapshot);
                start_task(world, entity, actions.clone(), BTreeMap::new(), Value::Null);
                break;
            }
        }
        let Some(mut task) = world.get_mut::<UiInstance>(entity).unwrap().task.take() else {
            continue;
        };
        match advance_task(world, entity, &definition, &mut task) {
            Ok(true) => {}
            Ok(false) => {
                if let Some(mut view) = world.get_mut::<UiInstance>(entity) {
                    view.task = Some(task);
                }
            }
            Err(error) => {
                if let Some(mut view) = world.get_mut::<UiInstance>(entity) {
                    log::error!("native UI {name}: {error}");
                    view.error = if task.completed_steps == 0 {
                        error
                    } else {
                        format!(
                            "动作序列已有 {} 个步骤收到成功响应；后续失败不撤回已完成操作，请读取状态，勿重复提交。\n{error}",
                            task.completed_steps
                        )
                    };
                    drop(view);
                    crate::view::invalidate_values(world, entity);
                }
            }
        }
    }
}
fn advance_task(
    world: &mut World,
    entity: Entity,
    definition: &UiResource,
    task: &mut Task,
) -> Result<bool, String> {
    if let Some(waiting) = task.waiting.take() {
        match waiting {
            Waiting::Timer(deadline) if Instant::now() < deadline => {
                task.waiting = Some(Waiting::Timer(deadline));
                return Ok(false);
            }
            Waiting::Timer(_) => {}
            Waiting::Command {
                call,
                save,
                debug_epoch,
            } => match call.poll() {
                ResponsePoll::Pending => {
                    task.waiting = Some(Waiting::Command {
                        call,
                        save,
                        debug_epoch,
                    });
                    return Ok(false);
                }
                ResponsePoll::Disconnected => {
                    return Err(
                        "命令响应通道已断开，结果未知；请读取当前状态，不要自动重试。".into(),
                    );
                }
                ResponsePoll::Ready(response) => {
                    if response["ok"] != true {
                        return Err(format!(
                            "{}: {}",
                            response["error"]["code"], response["error"]["message"]
                        ));
                    }
                    if let Some(epoch) = debug_epoch {
                        if world.resource::<UiData>().snapshot("client.debug")["epoch"] != epoch {
                            return Err("调试连接会话已改变，操作结果未知。".into());
                        }
                        let request = response["data"]["request_id"]
                            .as_str()
                            .ok_or("debug acceptance omitted request_id")?
                            .to_owned();
                        task.waiting = Some(Waiting::Debug {
                            request,
                            epoch,
                            save,
                            deadline: Instant::now()
                                + Duration::from_secs_f32(
                                    world
                                        .resource::<UiRuntime>()
                                        .registry
                                        .theme
                                        .debug_receipt_timeout_secs,
                                ),
                        });
                        return Ok(false);
                    }
                    task.completed_steps += 1;
                    task.result = response["data"].clone();
                    if let Some(path) = save {
                        set_model(
                            &mut world
                                .get_mut::<UiInstance>(entity)
                                .ok_or("view ended")?
                                .model,
                            &path,
                            task.result.clone(),
                        )?;
                    }
                }
            },
            Waiting::Debug {
                request,
                epoch,
                save,
                deadline,
            } => {
                let snapshot = world.resource::<UiData>().snapshot("client.debug");
                if snapshot["epoch"] != epoch {
                    return Err("调试连接会话已改变；操作结果未知，请重新读取。".into());
                }
                let record = snapshot["requests"]
                    .as_array()
                    .ok_or("debug snapshot omitted requests")?
                    .iter()
                    .find(|r| r["request_id"] == request);
                let result = record.map(|r| &r["result"]).filter(|r| !r.is_null());
                if let Some(result) = result {
                    if let Some(error) = result.get("Err") {
                        return Err(format!("{}: {}", error["code"], error["message"]));
                    }
                    task.completed_steps += 1;
                    task.result = result
                        .get("Ok")
                        .ok_or("invalid native debug receipt")?
                        .clone();
                    if let Some(path) = save {
                        set_model(
                            &mut world
                                .get_mut::<UiInstance>(entity)
                                .ok_or("view ended")?
                                .model,
                            &path,
                            task.result.clone(),
                        )?;
                    }
                } else {
                    if Instant::now() >= deadline {
                        return Err(
                            "诊断完成回执未送达；操作可能已执行。请刷新核实，勿自动重试。".into(),
                        );
                    }
                    task.waiting = Some(Waiting::Debug {
                        request,
                        epoch,
                        save,
                        deadline,
                    });
                    return Ok(false);
                }
            }
        }
    }
    while let Some(action) = task.actions.pop_front() {
        let instance = world.get::<UiInstance>(entity).ok_or("view ended")?;
        let data = data_context(world, definition);
        let context = Context {
            model: &instance.model,
            data: &data,
            locals: &task.locals,
            event: &task.event,
            result: &task.result,
            error: &instance.error,
            busy: true,
        };
        match action {
            Action::Set { path, value } => {
                let value = context.eval(&value)?;
                set_model(
                    &mut world.get_mut::<UiInstance>(entity).unwrap().model,
                    &path,
                    value,
                )?;
            }
            Action::Require { condition, message } => {
                if !boolean(&context.eval(&condition)?)? {
                    return Err(message);
                }
            }
            Action::Wait { seconds } => {
                task.waiting = Some(Waiting::Timer(
                    Instant::now() + Duration::from_secs_f32(seconds),
                ));
                return Ok(false);
            }
            Action::Command {
                name,
                arguments,
                save,
            } => {
                let arguments = context.eval(&arguments)?;
                let call = world
                    .resource::<UiRuntime>()
                    .io
                    .submit(
                        json!({"version":1,"command":name,"arguments":arguments}),
                        UiCommandSource::Native(UiInstanceId(entity)),
                    )
                    .map_err(|e| format!("command not queued: {e:?}"))?;
                task.waiting = Some(Waiting::Command {
                    call,
                    save,
                    debug_epoch: None,
                });
                return Ok(false);
            }
            Action::Debug { operation, save } => {
                if definition.page.session_source.is_none() {
                    return Err("debug action requires a session_source".into());
                }
                let epoch = world.resource::<UiData>().snapshot("client.debug")["epoch"].clone();
                if epoch.is_null() {
                    return Err("尚未连接到可执行调试操作的会话。".into());
                }
                let operation = context.eval(&operation)?;
                let call = world.resource::<UiRuntime>().io.submit(json!({"version":1,"command":"debug.request","arguments":{"operation":operation}}), UiCommandSource::Native(UiInstanceId(entity))).map_err(|e| format!("debug command not queued: {e:?}"))?;
                task.waiting = Some(Waiting::Command {
                    call,
                    save,
                    debug_epoch: Some(epoch),
                });
                return Ok(false);
            }
        }
    }
    Ok(true)
}
