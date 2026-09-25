use crate::{EnvironmentSample, GazeIntent, MovementIntent, TestExecutionBodyPrototype};
use bevy::prelude::{Bundle, Component, Entity, Resource, Vec3, World};
use std::collections::HashMap;

macro_rules! body_id {
    ($name:ident) => {
        #[derive(Component, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub u64);
    };
}

body_id!(IntentBodyId);
body_id!(ExecutionBodyId);
body_id!(RecordBodyId);

/// A Life has no separately generated identity: its identity is exactly this
/// ordered triple of independently-lived body instance identities.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LifeId {
    pub intent: IntentBodyId,
    pub execution: ExecutionBodyId,
    pub record: RecordBodyId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifeEntities {
    pub intent: Entity,
    pub execution: Entity,
    pub record: Entity,
}

/// The same explicit relationship is projected onto all three body entities.
/// It is relationship data, not a fourth Life entity.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifeLink {
    id: LifeId,
    entities: LifeEntities,
}

impl LifeLink {
    pub fn id(self) -> LifeId {
        self.id
    }

    pub fn entities(self) -> LifeEntities {
        self.entities
    }
}

#[derive(Bundle)]
pub struct IntentBodyBundle {
    pub id: IntentBodyId,
    pub movement: MovementIntent,
    pub gaze: GazeIntent,
}

impl IntentBodyBundle {
    pub fn materialize(id: IntentBodyId) -> Self {
        Self {
            id,
            movement: MovementIntent::default(),
            gaze: GazeIntent::default(),
        }
    }
}

#[derive(Bundle)]
pub struct RecordBodyBundle {
    pub id: RecordBodyId,
}

impl RecordBodyBundle {
    pub fn materialize(id: RecordBodyId) -> Self {
        Self { id }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TestIntentBodyPrototype;
impl TestIntentBodyPrototype {
    pub fn materialize(self, id: IntentBodyId) -> IntentBodyBundle {
        IntentBodyBundle::materialize(id)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TestRecordBodyPrototype;
impl TestRecordBodyPrototype {
    pub fn materialize(self, id: RecordBodyId) -> RecordBodyBundle {
        RecordBodyBundle::materialize(id)
    }
}

/// The current named test-Life recipe. It exists only at the creation boundary;
/// materialized bodies retain no source prototype identity or live inheritance.
#[derive(Clone, Copy, Debug, Default)]
pub struct TestLifePrototype {
    pub intent: TestIntentBodyPrototype,
    pub execution: TestExecutionBodyPrototype,
    pub record: TestRecordBodyPrototype,
}

impl TestLifePrototype {
    pub fn instantiate(
        self,
        world: &mut World,
        translation: Vec3,
        environment: EnvironmentSample,
    ) -> (LifeId, LifeEntities) {
        let (intent_id, execution_id, record_id) = {
            let mut ids = world.resource_mut::<BodyInstanceIds>();
            (ids.intent(), ids.execution(), ids.record())
        };
        let entities = LifeEntities {
            intent: world.spawn(self.intent.materialize(intent_id)).id(),
            execution: world
                .spawn(
                    self.execution
                        .materialize(execution_id, translation, environment),
                )
                .id(),
            record: world.spawn(self.record.materialize(record_id)).id(),
        };
        let id = compose_life(world, entities).expect("fresh test Life bodies compose");
        (id, entities)
    }
}

/// Allocates independent typed identities. Persistence can replace this runtime
/// allocator without changing Life identity semantics.
#[derive(Resource, Default)]
pub struct BodyInstanceIds {
    next_intent: u64,
    next_execution: u64,
    next_record: u64,
}

impl BodyInstanceIds {
    pub fn intent(&mut self) -> IntentBodyId {
        self.next_intent = self
            .next_intent
            .checked_add(1)
            .expect("intent body id exhausted");
        IntentBodyId(self.next_intent)
    }

    pub fn execution(&mut self) -> ExecutionBodyId {
        self.next_execution = self
            .next_execution
            .checked_add(1)
            .expect("execution body id exhausted");
        ExecutionBodyId(self.next_execution)
    }

    pub fn record(&mut self) -> RecordBodyId {
        self.next_record = self
            .next_record
            .checked_add(1)
            .expect("record body id exhausted");
        RecordBodyId(self.next_record)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifeCompositionError {
    MissingIntentBody,
    MissingExecutionBody,
    MissingRecordBody,
    BodyAlreadyBound,
    LifeAlreadyExists,
    InvalidComposition,
}

/// Indexes active relationships and enforces that a body participates in at
/// most one Life. Validation is explicit and demand-driven, never per-tick.
#[derive(Resource, Default)]
pub struct LifeRegistry {
    lives: HashMap<LifeId, LifeEntities>,
    intent: HashMap<IntentBodyId, LifeId>,
    execution: HashMap<ExecutionBodyId, LifeId>,
    record: HashMap<RecordBodyId, LifeId>,
}

impl LifeRegistry {
    pub fn entities(&self, id: LifeId) -> Option<LifeEntities> {
        self.lives.get(&id).copied()
    }

    pub fn remove(&mut self, id: LifeId) -> Option<LifeEntities> {
        let entities = self.lives.remove(&id)?;
        self.intent.remove(&id.intent);
        self.execution.remove(&id.execution);
        self.record.remove(&id.record);
        Some(entities)
    }
}

pub fn compose_life(
    world: &mut World,
    entities: LifeEntities,
) -> Result<LifeId, LifeCompositionError> {
    if entities.intent == entities.execution
        || entities.intent == entities.record
        || entities.execution == entities.record
        || !body_roles_are_exclusive(world, entities)
    {
        return Err(LifeCompositionError::InvalidComposition);
    }
    let intent = world
        .get::<IntentBodyId>(entities.intent)
        .copied()
        .ok_or(LifeCompositionError::MissingIntentBody)?;
    let execution = world
        .get::<ExecutionBodyId>(entities.execution)
        .copied()
        .ok_or(LifeCompositionError::MissingExecutionBody)?;
    let record = world
        .get::<RecordBodyId>(entities.record)
        .copied()
        .ok_or(LifeCompositionError::MissingRecordBody)?;
    if [entities.intent, entities.execution, entities.record]
        .into_iter()
        .any(|entity| world.get::<LifeLink>(entity).is_some())
    {
        return Err(LifeCompositionError::BodyAlreadyBound);
    }

    let id = LifeId {
        intent,
        execution,
        record,
    };
    let mut registry = world
        .get_resource_mut::<LifeRegistry>()
        .expect("LifeRegistry must be initialized");
    if registry.lives.contains_key(&id) {
        return Err(LifeCompositionError::LifeAlreadyExists);
    }
    if registry.intent.contains_key(&intent)
        || registry.execution.contains_key(&execution)
        || registry.record.contains_key(&record)
    {
        return Err(LifeCompositionError::BodyAlreadyBound);
    }
    registry.lives.insert(id, entities);
    registry.intent.insert(intent, id);
    registry.execution.insert(execution, id);
    registry.record.insert(record, id);
    drop(registry);

    let link = LifeLink { id, entities };
    world.entity_mut(entities.intent).insert(link);
    world.entity_mut(entities.execution).insert(link);
    world.entity_mut(entities.record).insert(link);
    Ok(id)
}

/// Checks the complete relationship only when a caller needs to rely on it.
/// An invalid relationship is removed from the index and surviving bodies are
/// detached without being destroyed.
pub fn validate_life(world: &mut World, id: LifeId) -> Result<LifeEntities, LifeCompositionError> {
    let Some(entities) = world.resource::<LifeRegistry>().entities(id) else {
        return Err(LifeCompositionError::InvalidComposition);
    };
    let valid = body_roles_are_exclusive(world, entities)
        && world.get::<IntentBodyId>(entities.intent) == Some(&id.intent)
        && world.get::<ExecutionBodyId>(entities.execution) == Some(&id.execution)
        && world.get::<RecordBodyId>(entities.record) == Some(&id.record)
        && [entities.intent, entities.execution, entities.record]
            .into_iter()
            .all(|entity| {
                world
                    .get::<LifeLink>(entity)
                    .is_some_and(|link| *link == LifeLink { id, entities })
            });
    if valid {
        return Ok(entities);
    }
    dissolve_life(world, id);
    Err(LifeCompositionError::InvalidComposition)
}

fn body_roles_are_exclusive(world: &World, entities: LifeEntities) -> bool {
    world.get::<IntentBodyId>(entities.intent).is_some()
        && world.get::<ExecutionBodyId>(entities.intent).is_none()
        && world.get::<RecordBodyId>(entities.intent).is_none()
        && world.get::<IntentBodyId>(entities.execution).is_none()
        && world.get::<ExecutionBodyId>(entities.execution).is_some()
        && world.get::<RecordBodyId>(entities.execution).is_none()
        && world.get::<IntentBodyId>(entities.record).is_none()
        && world.get::<ExecutionBodyId>(entities.record).is_none()
        && world.get::<RecordBodyId>(entities.record).is_some()
}

pub fn dissolve_life(world: &mut World, id: LifeId) -> Option<LifeEntities> {
    let entities = world.resource_mut::<LifeRegistry>().remove(id)?;
    for entity in [entities.intent, entities.execution, entities.record] {
        if let Ok(mut entity) = world.get_entity_mut(entity) {
            entity.remove::<LifeLink>();
        }
    }
    Some(entities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestExecutionBodyPrototype;

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<LifeRegistry>();
        world
    }

    #[test]
    fn composition_identity_is_the_ordered_body_id_triple() {
        let mut world = world();
        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(7)))
            .id();
        let execution = world
            .spawn(TestExecutionBodyPrototype::default().materialize(
                ExecutionBodyId(11),
                Vec3::ZERO,
                EnvironmentSample::default(),
            ))
            .id();
        let record = world
            .spawn(RecordBodyBundle::materialize(RecordBodyId(13)))
            .id();

        let entities = LifeEntities {
            intent,
            execution,
            record,
        };
        let id = compose_life(&mut world, entities).unwrap();
        assert_eq!(
            id,
            LifeId {
                intent: IntentBodyId(7),
                execution: ExecutionBodyId(11),
                record: RecordBodyId(13)
            }
        );
        assert_eq!(validate_life(&mut world, id), Ok(entities));
    }

    #[test]
    fn one_body_cannot_belong_to_two_lives() {
        let mut world = world();
        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let execution = world
            .spawn(TestExecutionBodyPrototype::default().materialize(
                ExecutionBodyId(1),
                Vec3::ZERO,
                EnvironmentSample::default(),
            ))
            .id();
        let record = world
            .spawn(RecordBodyBundle::materialize(RecordBodyId(1)))
            .id();
        compose_life(
            &mut world,
            LifeEntities {
                intent,
                execution,
                record,
            },
        )
        .unwrap();
        let second_execution = world
            .spawn(TestExecutionBodyPrototype::default().materialize(
                ExecutionBodyId(2),
                Vec3::ZERO,
                EnvironmentSample::default(),
            ))
            .id();
        let second_record = world
            .spawn(RecordBodyBundle::materialize(RecordBodyId(2)))
            .id();
        assert_eq!(
            compose_life(
                &mut world,
                LifeEntities {
                    intent,
                    execution: second_execution,
                    record: second_record
                }
            ),
            Err(LifeCompositionError::BodyAlreadyBound)
        );
    }

    #[test]
    fn validation_detaches_survivors_without_destroying_them() {
        let mut world = world();
        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let execution = world
            .spawn(TestExecutionBodyPrototype::default().materialize(
                ExecutionBodyId(1),
                Vec3::ZERO,
                EnvironmentSample::default(),
            ))
            .id();
        let record = world
            .spawn(RecordBodyBundle::materialize(RecordBodyId(1)))
            .id();
        let id = compose_life(
            &mut world,
            LifeEntities {
                intent,
                execution,
                record,
            },
        )
        .unwrap();
        world.despawn(execution);
        assert_eq!(
            validate_life(&mut world, id),
            Err(LifeCompositionError::InvalidComposition)
        );
        assert!(world.get_entity(intent).is_ok());
        assert!(world.get_entity(record).is_ok());
        assert!(world.get::<LifeLink>(intent).is_none());
        assert!(world.get::<LifeLink>(record).is_none());
    }
}
