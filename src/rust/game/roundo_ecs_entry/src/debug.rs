//! Bounded, World-local operations for native game diagnostics.
use crate::{
    authoritative_creature::CreatureAssemblyRegistry, player_control::LocalAiControlSet,
};
use bevy::prelude::*;
use roundo_contracts::AttributeId;
mod data;
pub use data::*;
use roundo_creature::{
    CreatureMotionExperienceState, ExecutionBodyId, GazeCommandEvent, GazeCommandState,
    IntentBodyId, JumpCommandEvent, MovementCommandState, RecordBodyId,
};
use roundo_life::{
    attributes::{AttributeRegistry, AttributeWrites},
    *,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod inventory;
mod items;

pub use items::DebugItemIssuer;

const MAX_TARGETS: usize = 4096;
const MAX_WORK: usize = 64;
const PAGE: usize = 64;

#[derive(Resource, Default)]
pub struct DebugIngress {
    queue: VecDeque<DebugRequest>,
}
impl DebugIngress {
    pub fn submit(&mut self, request: DebugRequest) -> Result<(), DebugFailure> {
        let short = |value: &str| !value.is_empty() && value.len() <= 128;
        let valid = match &request.operation {
            DebugOperation::Inspect { body } | DebugOperation::SetAttribute { body, .. } => {
                short(body)
            }
            DebugOperation::Relation {
                body,
                life,
                data_type,
                ..
            } => short(body) && short(life) && short(data_type),
            DebugOperation::CreateItem {
                inventory,
                placement_reach,
                destruction_reach,
            } => {
                short(inventory)
                    && [placement_reach, destruction_reach]
                        .into_iter()
                        .all(|reach| reach.is_none_or(|value| value.is_finite() && value > 0.0))
            }
            DebugOperation::Item(operation) => match operation {
                DebugItemOperation::List { inventory, .. } => short(inventory),
                DebugItemOperation::Remove {
                    inventory,
                    selector,
                } => short(inventory) && short(selector),
            },
            DebugOperation::Inventory(operation) => match operation {
                DebugInventoryOperation::List { body, .. } => body.as_deref().is_none_or(short),
                DebugInventoryOperation::Create { body } => short(body),
                DebugInventoryOperation::Inspect { inventory, .. }
                | DebugInventoryOperation::Remove { inventory } => short(inventory),
                DebugInventoryOperation::Spill { inventory, item } => {
                    short(inventory) && inventory::valid_item(item)
                }
                DebugInventoryOperation::Transfer {
                    source,
                    target,
                    item,
                } => short(source) && short(target) && inventory::valid_item(item),
            },
            _ => true,
        };
        if !valid {
            return Err(DebugFailure::new(
                "invalid_request",
                "诊断选择器或参数无效；物品功能距离必须为有限正数",
            ));
        }
        if self.queue.len() >= MAX_WORK {
            return Err(DebugFailure::new("busy", "诊断入口容量已满"));
        }
        self.queue.push_back(request);
        Ok(())
    }
}

pub(crate) struct DebugPlugin;
impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugIngress>()
            .init_resource::<DebugState>()
            .add_message::<DebugReply>()
            .add_systems(
                FixedUpdate,
                (
                    begin,
                    state_type::<MovementCommandState>,
                    state_type::<GazeCommandState>,
                    state_type::<CreatureMotionExperienceState>,
                    event_type::<GazeCommandEvent>,
                    event_type::<JumpCommandEvent>,
                    finish,
                )
                    .chain()
                    .after(LocalAiControlSet)
                    .before(roundo_creature::CreatureLifeSet::CommandProjection),
            );
        if app.is_plugin_added::<roundo_inventory::InventoryPlugin>() {
            app.init_resource::<inventory::InventorySelectors>()
                .init_resource::<items::ItemSelectors>()
                .add_systems(
                    FixedUpdate,
                    (describe_public, inventory::manage, items::manage)
                        .chain()
                        .after(begin)
                        .before(finish),
                );
        }
    }
}

#[derive(Resource, Default)]
struct DebugState {
    next: u64,
    bodies: BTreeMap<ConceptBodyHandle, String>,
    lives: BTreeMap<LifeHandle, String>,
    work: Vec<Work>,
    pending: Vec<Pending>,
}
struct Work {
    request: DebugRequest,
    body: Option<ConceptBodyHandle>,
    life: Option<LifeHandle>,
    result: Option<Result<DebugData, DebugFailure>>,
    deferred: bool,
}
struct Pending {
    request: String,
    operation: PendingOperation,
}
enum PendingOperation {
    Factory(FactoryRequestId),
    Structure(LifeStructureRequestId),
}

fn failure(error: impl std::fmt::Debug) -> DebugFailure {
    DebugFailure::new("native_rejection", format!("{error:?}"))
}
impl DebugState {
    fn allocate(next: &mut u64, kind: &str) -> String {
        *next = next.checked_add(1).expect("diagnostic selector exhausted");
        format!("{kind}-{next}")
    }
    fn sync(&mut self, instances: &LifeInstances) {
        self.bodies
            .retain(|handle, _| instances.resolve_body(*handle).is_ok());
        self.lives
            .retain(|handle, _| instances.resolve_life(*handle).is_ok());
        let mut bodies: Vec<_> = instances.bodies().collect();
        bodies.sort();
        bodies.truncate(MAX_TARGETS);
        let mut lives: Vec<_> = instances.lives().collect();
        lives.sort();
        lives.truncate(MAX_TARGETS);
        let selected_bodies: BTreeSet<_> = bodies.iter().copied().collect();
        let selected_lives: BTreeSet<_> = lives.iter().copied().collect();
        self.bodies
            .retain(|handle, _| selected_bodies.contains(handle));
        self.lives
            .retain(|handle, _| selected_lives.contains(handle));
        for handle in bodies {
            self.bodies
                .entry(handle)
                .or_insert_with(|| Self::allocate(&mut self.next, "body"));
        }
        for handle in lives {
            self.lives
                .entry(handle)
                .or_insert_with(|| Self::allocate(&mut self.next, "life"));
        }
    }
    fn body(&self, id: &str) -> Result<ConceptBodyHandle, DebugFailure> {
        self.bodies
            .iter()
            .find_map(|(handle, name)| (name == id).then_some(*handle))
            .ok_or_else(|| {
                DebugFailure::new("stale_body", "概念体不存在或诊断选择器已失效，请刷新列表")
            })
    }
    fn life(&self, id: &str) -> Result<LifeHandle, DebugFailure> {
        self.lives
            .iter()
            .find_map(|(handle, name)| (name == id).then_some(*handle))
            .ok_or_else(|| {
                DebugFailure::new("stale_life", "生命体不存在或诊断选择器已失效，请刷新列表")
            })
    }
}

fn data_types() -> Vec<DebugDataType> {
    let states = [
        MovementCommandState::KEY,
        GazeCommandState::KEY,
        CreatureMotionExperienceState::KEY,
    ];
    let events = [GazeCommandEvent::KEY, JumpCommandEvent::KEY];
    states
        .into_iter()
        .map(|key| (key, "state"))
        .chain(events.into_iter().map(|key| (key, "event")))
        .map(|(key, shape)| DebugDataType {
            key: key.as_str().into(),
            shape: shape.into(),
        })
        .collect()
}
fn life_recipe() -> LifePrototype {
    let policy = StatePolicy::allow_uninitialized();
    let events = EventPolicy::new(64, 65536, 1, 200).expect("finite diagnostic Event policy");
    LifePrototype::empty()
        .with_state_policy::<MovementCommandState>(policy)
        .with_state_policy::<GazeCommandState>(policy)
        .with_state_policy::<CreatureMotionExperienceState>(policy)
        .with_event_interface::<GazeCommandEvent>(events)
        .with_event_interface::<JumpCommandEvent>(events)
}
fn body_values(values: DebugBodyValues) -> Result<impl Bundle, DebugFailure> {
    Ok((
        Health::new(values.health),
        IntentValue::new(values.intent),
        ExecutionValue::new(values.execution),
        RecordValue::new(values.record),
        ContinuityRate::new(Unorm::new(values.continuity).map_err(failure)?),
        ChangeRate::new(Unorm::new(values.change).map_err(failure)?),
        CycleRate::new(Unorm::new(values.cycle).map_err(failure)?),
        EternityRate::new(Unorm::new(values.eternity).map_err(failure)?),
    ))
}

type BodyRoles<'w, 's> =
    Query<'w, 's, (Has<IntentBodyId>, Has<ExecutionBodyId>, Has<RecordBodyId>)>;

fn body_summary(
    body: &str,
    handle: ConceptBodyHandle,
    state: &DebugState,
    instances: &LifeInstances,
    roles: &BodyRoles,
    assemblies: Option<&CreatureAssemblyRegistry>,
) -> Result<DebugBodySummary, DebugFailure> {
    let entity = instances.resolve_body(handle).map_err(failure)?;
    let (intent, execution, record) = roles.get(entity).map_err(failure)?;
    let mut roles = Vec::new();
    if intent {
        roles.push(DebugBodyRole::Intent);
    }
    if execution {
        roles.push(DebugBodyRole::Execution);
    }
    if record {
        roles.push(DebugBodyRole::Record);
    }
    let recipe_life = assemblies
        .and_then(|assemblies| assemblies.life_for_body(handle))
        .filter(|life| instances.resolve_life(*life).is_ok())
        .and_then(|life| state.lives.get(&life).cloned());
    Ok(DebugBodySummary {
        body: body.into(),
        roles,
        is_public: false,
        recipe_life,
    })
}

// Compare checked native owner identities for Public attribution; no new
// component, ownership field or capability is minted.
fn describe_public(mut state: ResMut<DebugState>, inventories: roundo_inventory::Inventories) {
    if !state.work.iter().any(|work| {
        matches!(
            work.result,
            Some(Ok(DebugData::Objects { .. } | DebugData::Body { .. }))
        )
    }) {
        return;
    }
    let public = inventories.public_owner().map_err(failure);
    let DebugState { bodies, work, .. } = &mut *state;
    let annotate = |summary: &mut DebugBodySummary| -> Result<(), DebugFailure> {
        let public = public.as_ref().map_err(Clone::clone)?;
        let handle = bodies
            .iter()
            .find_map(|(handle, name)| (name == &summary.body).then_some(*handle))
            .ok_or_else(|| DebugFailure::new("stale_body", "概念体描述已失效，请刷新列表"))?;
        summary.is_public = inventories.bind_owner(handle).map_err(failure)? == *public;
        Ok(())
    };
    for work in work {
        let result = match work.result.as_mut() {
            Some(Ok(DebugData::Objects { body_summaries, .. })) => {
                body_summaries.iter_mut().try_for_each(&annotate)
            }
            Some(Ok(DebugData::Body {
                summary: Some(summary),
                ..
            })) => annotate(summary),
            _ => continue,
        };
        if let Err(error) = result {
            work.result = Some(Err(error));
        }
    }
}

fn begin(
    mut ingress: ResMut<DebugIngress>,
    mut state: ResMut<DebugState>,
    instances: LifeInstances,
    roles: BodyRoles,
    assemblies: Option<Res<CreatureAssemblyRegistry>>,
    mut attributes: AttributeWrites,
    registry: Res<AttributeRegistry>,
    mut management: ParamSet<(FactoryManagement, LifeStructureManagement)>,
    mut replies: MessageWriter<DebugReply>,
) {
    if ingress.queue.is_empty() && state.pending.is_empty() {
        return;
    }
    state.sync(&instances);
    let pending = std::mem::take(&mut state.pending);
    for item in pending {
        let result = match item.operation {
            PendingOperation::Factory(id) => {
                management
                    .p0()
                    .take_completion(id)
                    .map(|completion| match completion {
                        FactoryCompletion::BodyCreated(handle) => {
                            // Factory completion, not enqueue, establishes successful creation.
                            let name = if let Some(name) = state.bodies.get(&handle) {
                                name.clone()
                            } else {
                                let name = DebugState::allocate(&mut state.next, "body");
                                state.bodies.insert(handle, name.clone());
                                name
                            };
                            Ok(DebugData::Created {
                                body: Some(name),
                                life: None,
                            })
                        }
                        FactoryCompletion::LifeCreated(handle) => {
                            let name = if let Some(name) = state.lives.get(&handle) {
                                name.clone()
                            } else {
                                let name = DebugState::allocate(&mut state.next, "life");
                                state.lives.insert(handle, name.clone());
                                name
                            };
                            Ok(DebugData::Created {
                                body: None,
                                life: Some(name),
                            })
                        }
                        other => Err(failure(other)),
                    })
            }
            PendingOperation::Structure(id) => {
                management
                    .p1()
                    .take_completion(id)
                    .map(|completion| match completion {
                        LifeStructureCompletion::State(Ok(()))
                        | LifeStructureCompletion::Event(Ok(())) => Ok(DebugData::Done),
                        other => Err(failure(other)),
                    })
            }
        };
        if let Some(result) = result {
            replies.write(DebugReply {
                request_id: item.request,
                result,
            });
        } else {
            state.pending.push(item);
        }
    }
    for _ in 0..16 {
        let Some(request) = ingress.queue.pop_front() else {
            break;
        };
        if state.pending.len() + state.work.len() >= MAX_WORK {
            replies.write(DebugReply {
                request_id: request.request_id,
                result: Err(DebugFailure::new("busy", "诊断请求容量已满")),
            });
            continue;
        }
        let mut work = Work {
            request,
            body: None,
            life: None,
            result: None,
            deferred: false,
        };
        let result = (|| -> Result<Option<DebugData>, DebugFailure> {
            match &work.request.operation {
                DebugOperation::List { offset } => {
                    let offset = *offset as usize;
                    let count = state.bodies.len().max(state.lives.len());
                    let body_summaries = state
                        .bodies
                        .iter()
                        .skip(offset)
                        .take(PAGE)
                        .map(|(handle, name)| {
                            body_summary(
                                name,
                                *handle,
                                &state,
                                &instances,
                                &roles,
                                assemblies.as_deref(),
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Some(DebugData::Objects {
                        body_summaries,
                        bodies: state
                            .bodies
                            .values()
                            .skip(offset)
                            .take(PAGE)
                            .cloned()
                            .collect(),
                        lives: state
                            .lives
                            .values()
                            .skip(offset)
                            .take(PAGE)
                            .cloned()
                            .collect(),
                        total_bodies: instances.bodies().count().try_into().unwrap_or(u32::MAX),
                        total_lives: instances.lives().count().try_into().unwrap_or(u32::MAX),
                        next_offset: (offset.saturating_add(PAGE) < count)
                            .then_some((offset + PAGE) as u32),
                        truncated: instances.bodies().count() > MAX_TARGETS
                            || instances.lives().count() > MAX_TARGETS,
                        data_types: data_types(),
                    }))
                }
                DebugOperation::Inspect { body } => {
                    let handle = state.body(body)?;
                    work.body = Some(handle);
                    let entity = instances.resolve_body(handle).map_err(failure)?;
                    let mut values = Vec::new();
                    for descriptor in registry.iter() {
                        if let Some(value) =
                            attributes.get(entity, descriptor.id()).map_err(failure)?
                        {
                            let value = value.as_i32().map(DebugValue::Integer).or_else(|| {
                                value.as_unorm().map(|v| DebugValue::Unorm(v.get()))
                            });
                            values.push(DebugAttribute {
                                id: descriptor.id() as u8,
                                name: descriptor.name().into(),
                                writable: value.is_some(),
                                value,
                            });
                        }
                    }
                    Ok(Some(DebugData::Body {
                        body: body.clone(),
                        summary: Some(body_summary(
                            body,
                            handle,
                            &state,
                            &instances,
                            &roles,
                            assemblies.as_deref(),
                        )?),
                        attributes: values,
                        relations: Vec::new(),
                        relations_truncated: false,
                    }))
                }
                DebugOperation::SetAttribute {
                    body,
                    attribute,
                    value,
                } => {
                    let entity = instances.resolve_body(state.body(body)?).map_err(failure)?;
                    let id = AttributeId::try_from(*attribute).map_err(failure)?;
                    match value {
                        DebugValue::Integer(value) => {
                            attributes.set(entity, id, value).map_err(failure)?
                        }
                        DebugValue::Unorm(value) => attributes
                            .set(entity, id, &Unorm::new(*value).map_err(failure)?)
                            .map_err(failure)?,
                    }
                    Ok(Some(DebugData::Done))
                }
                DebugOperation::CreateBody { values } => {
                    if instances.bodies().count() + state.pending.len() >= MAX_TARGETS {
                        return Err(DebugFailure::new("capacity", "概念体诊断创建上限已到达"));
                    }
                    let id = management
                        .p0()
                        .create_body(ConceptBodyPrototype::new(body_values(*values)?))
                        .map_err(failure)?;
                    state.pending.push(Pending {
                        request: work.request.request_id.clone(),
                        operation: PendingOperation::Factory(id),
                    });
                    work.deferred = true;
                    Ok(None)
                }
                DebugOperation::CreateLife => {
                    if instances.lives().count() + state.pending.len() >= MAX_TARGETS {
                        return Err(DebugFailure::new("capacity", "生命体诊断创建上限已到达"));
                    }
                    let id = management
                        .p0()
                        .create_life(life_recipe())
                        .map_err(failure)?;
                    state.pending.push(Pending {
                        request: work.request.request_id.clone(),
                        operation: PendingOperation::Factory(id),
                    });
                    work.deferred = true;
                    Ok(None)
                }
                DebugOperation::Inventory(_)
                | DebugOperation::Item(_)
                | DebugOperation::CreateItem { .. } => Ok(None),
                DebugOperation::Relation {
                    body,
                    life,
                    data_type,
                    ..
                } => {
                    if !data_types().iter().any(|t| &t.key == data_type) {
                        return Err(DebugFailure::new(
                            "unsupported_type",
                            "此诊断适配器不支持该原生数据类型",
                        ));
                    }
                    work.body = Some(state.body(body)?);
                    work.life = Some(state.life(life)?);
                    Ok(None)
                }
            }
        })();
        work.result = match result {
            Ok(data) => data.map(Ok),
            Err(error) => Some(Err(error)),
        };
        state.work.push(work);
    }
}

// Typed systems keep ECS borrow declarations visible. The diagnostic selector
// table is routing only; every native interface rechecks live Factory provenance.
macro_rules! typed_system {
    ($name:ident, $bound:ident, $registrations:ident, $management:ident, $shape:literal,
     $present:expr, $origin:expr, $approve_writer:ident, $register_writer:ident,
     $declare_reader:ident, $declare_writer:ident, $create:expr) => {
        fn $name<T: $bound>(
            mut state: ResMut<DebugState>,
            mut params: ParamSet<($registrations<T>, $management<T>, LifeStructureManagement)>,
        ) {
            if state.work.is_empty() {
                return;
            }
            let lives = state.lives.clone();
            let mut pending = Vec::new();
            for work in &mut state.work {
                let Some(body) = work.body else { continue };
                if let Some(Ok(DebugData::Body {
                    relations,
                    relations_truncated,
                    ..
                })) = &mut work.result
                {
                    let evidence = params.p0();
                    for (&life, name) in &lives {
                        let present: bool = ($present)(&evidence, life);
                        if present && relations.len() >= 1024 {
                            *relations_truncated = true;
                        }
                        if present && relations.len() < 1024 {
                            relations.push(DebugRelation {
                                life: name.clone(),
                                data_type: T::KEY.as_str().into(),
                                shape: $shape.into(),
                                reader_declared: evidence
                                    .validate_reader_declaration(body)
                                    .is_ok(),
                                writer_declared: evidence
                                    .validate_writer_declaration(body)
                                    .is_ok(),
                                reader_registered: evidence.validate_reader(life, body).is_ok(),
                                writer_registered: evidence.validate_writer(life, body).is_ok(),
                                immutable_origin: ($origin)(&evidence, life, body),
                            });
                        }
                    }
                }
                if work.result.is_some() || work.deferred {
                    continue;
                }
                let DebugOperation::Relation {
                    data_type, action, ..
                } = &work.request.operation
                else {
                    continue;
                };
                if data_type != T::KEY.as_str() {
                    continue;
                }
                let life = work.life.expect("validated Life selector");
                use DebugRelationAction::*;
                let structure = match action {
                    DeclareReader => Some(params.p2().$declare_reader::<T>(body)),
                    DeclareWriter => Some(params.p2().$declare_writer::<T>(body)),
                    CreateSlot => ($create)(&mut params.p2(), life, body),
                    _ => None,
                };
                if let Some(result) = structure {
                    match result {
                        Ok(id) => {
                            work.deferred = true;
                            pending.push(Pending {
                                request: work.request.request_id.clone(),
                                operation: PendingOperation::Structure(id),
                            });
                        }
                        Err(error) => work.result = Some(Err(failure(error))),
                    }
                    continue;
                }
                let mut management = params.p1();
                let result = match action {
                    ApproveReader => management.approve_reader(life, body),
                    ApproveWriter => management.$approve_writer(life, body),
                    RegisterReader => management.register_reader(life, body),
                    RegisterWriter => management.$register_writer(life, body),
                    UnregisterReader => management.deregister_reader(life, body),
                    UnregisterWriter => management.deregister_writer(life, body),
                    _ => {
                        work.result = Some(Err(DebugFailure::new(
                            "unsupported_action",
                            "此操作不适用于该数据形状",
                        )));
                        continue;
                    }
                };
                work.result = Some(result.map(|()| DebugData::Done).map_err(failure));
            }
            state.pending.extend(pending);
        }
    };
}
typed_system!(
    state_type,
    StateData,
    LifeStateRegistrations,
    LifeStateManagement,
    "state",
    |e: &LifeStateRegistrations<T>, life| e.state_policy(life).is_ok(),
    |e: &LifeStateRegistrations<T>, life, body| e
        .slot_evidence(life)
        .ok()
        .flatten()
        .is_some_and(|slot| slot.origin() == body),
    approve_origin,
    rebind_writer,
    declare_state_reader,
    declare_state_writer,
    |s: &mut LifeStructureManagement, life, body| Some(
        s.create_state_slot::<T>(life, body, None)
    )
);
typed_system!(
    event_type,
    EventData,
    LifeEventRegistrations,
    LifeEventManagement,
    "event",
    |e: &LifeEventRegistrations<T>, life| e.interface_evidence(life).is_ok(),
    |_e: &LifeEventRegistrations<T>, _life, _body| false,
    approve_writer,
    register_writer,
    declare_event_reader,
    declare_event_writer,
    |_s: &mut LifeStructureManagement, _life, _body| None::<
        Result<LifeStructureRequestId, FactorySubmitError>,
    >
);

fn finish(mut state: ResMut<DebugState>, mut replies: MessageWriter<DebugReply>) {
    for work in state.work.drain(..) {
        if work.deferred {
            continue;
        }
        let result = work.result.unwrap_or_else(|| {
            Err(DebugFailure::new(
                "unsupported_operation",
                "没有原生诊断适配器处理该操作",
            ))
        });
        replies.write(DebugReply {
            request_id: work.request.request_id,
            result,
        });
    }
}
