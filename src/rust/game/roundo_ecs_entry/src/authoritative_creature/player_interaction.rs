//! Player-controlled block behavior. Triggered work owns a copy of the server
//! gaze at trigger time; later look updates and Command expiry cannot change it.
use super::*;
use bevy::{ecs::system::SystemParam, prelude::*};
use roundo_block_interaction::{BlockInteractionOperation, ItemUseCommandEvent};
use roundo_contracts::ItemUseMethod;
use roundo_creature::{AabbDimensions, FactoryMotionExecutions, FactoryMotionRoutes};
use roundo_life::{LifeEventReader, LifeInstances, LifeStateReader};
use roundo_local_coordinate::{
    AtomicVoxelRegistry, EMPTY_VOXEL_ID, LocalPhysicsDemands, LocalVoxelEdits, VoxelRaycaster,
};
use std::collections::VecDeque;

const BEHAVIOR_CAPACITY: usize = 16;

#[derive(Component, Clone, Copy)]
pub(super) struct TestInteractionController(pub ControllerId);
#[derive(Clone, Copy)]
struct AimedBehavior {
    use_item: ItemUseCommandEvent,
    gaze: [f32; 4],
}
#[derive(Component, Default)]
pub(crate) struct PlayerBlockBehaviors(VecDeque<AimedBehavior>);
impl PlayerBlockBehaviors {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

pub(super) fn attach_interaction_controller(
    In((assembly, id)): In<(UnpublishedCreatureLife, ControllerId)>,
    instances: LifeInstances,
    registry: Res<ControllerRegistry>,
    controllers: Query<&Controller>,
    mut commands: Commands,
) -> Result<(), CreatureAssemblyError> {
    let [_, execution, _] = assembly.resolve(&instances)?;
    let controller = registry
        .controller_entity(id)
        .and_then(|entity| controllers.get(entity).ok())
        .ok_or(CreatureAssemblyError::InvalidBootstrap)?;
    if controller.id() != id
        || controller.intent_body_handle() != Some(assembly.members.intent)
        || controller.domain() != IntentDomain::block_interaction()
        || controller.current_owner().is_some()
    {
        return Err(CreatureAssemblyError::InvalidBootstrap);
    }
    commands.entity(execution).insert((
        TestInteractionController(id),
        PlayerBlockBehaviors::default(),
    ));
    Ok(())
}

/// Reads native data as the Execution participant, separately from publication.
/// Full work storage does not reject a Command already accepted by Life.
pub(crate) fn trigger(
    In(route): In<FactoryMotionRoute>,
    routes: FactoryMotionRoutes<CreatureAssemblyRegistry>,
    instances: LifeInstances,
    gaze: LifeStateReader<GazeCommandState>,
    mut events: LifeEventReader<ItemUseCommandEvent>,
    mut executions: Query<(&GazeOrientation, &mut PlayerBlockBehaviors)>,
) {
    if !routes.is_live(route) {
        return;
    }
    let Ok(entity) = instances.resolve_body(route.execution()) else {
        return;
    };
    let Ok((baseline, mut work)) = executions.get_mut(entity) else {
        return;
    };
    if work.0.len() >= BEHAVIOR_CAPACITY {
        return;
    }
    let selected = instances
        .with_body(entity, |body| {
            gaze.with_state_view(&body, route.life(), |view| view.current().copied())
        })
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(selected) = selected else { return };
    let direction = selected.orientation().unwrap_or(baseline.0.to_array());
    let event = instances
        .with_body(entity, |body| {
            events.read_next_with(&body, route.life(), |event| *event)
        })
        .ok()
        .and_then(Result::ok)
        .flatten();
    if let Some(use_item) = event {
        work.0.push_back(AimedBehavior {
            use_item,
            gaze: direction,
        });
    }
}

#[derive(SystemParam)]
pub(super) struct EditEligibility<'w, 's> {
    admission: Option<Res<'w, crate::authority_admission::Admission>>,
    registry: Res<'w, ControllerRegistry>,
    controllers: Query<'w, 's, &'static Controller>,
    players: Query<'w, 's, &'static Player>,
    player_registry: Res<'w, PlayerRegistry>,
}
struct CurrentEditFacts<'a, 'w, 's> {
    eligibility: &'a EditEligibility<'w, 's>,
    controller: ControllerId,
}
impl roundo_controller::authority_gate::AuthorityFacts for CurrentEditFacts<'_, '_, '_> {
    fn player_access(&self, player: roundo_contracts::PlayerId) -> ControllerAccessLevel {
        self.eligibility
            .player_registry
            .player_entity(player)
            .and_then(|entity| self.eligibility.players.get(entity).ok())
            .map(|player| player.controller_access(self.controller))
            .unwrap_or_default()
    }
    fn player_access_revision(&self, player: roundo_contracts::PlayerId) -> Option<u64> {
        self.eligibility
            .player_registry
            .player_entity(player)
            .and_then(|entity| self.eligibility.players.get(entity).ok())
            .and_then(|player| player.controller_access_revision(self.controller))
    }
    fn sole_admitted_for(
        &self,
        player: roundo_contracts::PlayerId,
        connection: roundo_controller::authority_gate::AdmittedConnection,
    ) -> bool {
        self.eligibility
            .player_registry
            .player_for_connection(connection.id)
            == Some(player)
            && self
                .eligibility
                .player_registry
                .connections_for_player(player)
                .count()
                == 1
            && self
                .eligibility
                .admission
                .as_deref()
                .is_some_and(|admission| admission.connection(connection.id) == Some(connection))
    }
    fn live_ai(&self, _: roundo_controller::AiControllerId) -> bool {
        false
    }
}
impl EditEligibility<'_, '_> {
    fn permits(&self, id: ControllerId, route: FactoryMotionRoute) -> bool {
        let Some(admission) = self.admission.as_deref() else {
            return false;
        };
        let Some(entity) = self.registry.controller_entity(id) else {
            return false;
        };
        if !self.controllers.get(entity).is_ok_and(|controller| {
            controller.id() == id
                && controller.domain() == IntentDomain::block_interaction()
                && controller.intent_body_handle() == Some(route.intent())
        }) {
            return false;
        }
        admission
            .active_player(id)
            .is_some_and(|(connection, player)| {
                admission.connection(connection).is_some_and(|connection| {
                    admission
                        .submit_player_with_facts(
                            id,
                            entity,
                            player,
                            connection,
                            &CurrentEditFacts {
                                eligibility: self,
                                controller: id,
                            },
                        )
                        .is_ok()
                })
            })
    }
}

pub(super) fn execute(
    instances: LifeInstances,
    routes: FactoryMotionRoutes<CreatureAssemblyRegistry>,
    motions: FactoryMotionExecutions<CreatureAssemblyRegistry>,
    assemblies: Res<CreatureAssemblyRegistry>,
    eligibility: EditEligibility,
    item_data: crate::block_item::BlockItemData,
    registry: Res<AtomicVoxelRegistry>,
    coordinates: Query<&GlobalTransform>,
    mut executions: Query<(
        &TestInteractionController,
        &AabbDimensions,
        &mut PlayerBlockBehaviors,
    )>,
    mut world: ParamSet<(VoxelRaycaster, LocalVoxelEdits, LocalPhysicsDemands)>,
) {
    let mut lives = assemblies.by_life.keys().copied().collect::<Vec<_>>();
    lives.sort_by_key(|life| {
        assemblies
            .get(*life)
            .and_then(|members| instances.resolve_body(members.execution).ok())
    });
    for life in lives {
        let Some(members) = assemblies.get(life) else {
            continue;
        };
        if !assemblies.accepts_controller_input(life) {
            continue;
        }
        let route =
            FactoryMotionRoute::new(life, members.intent, members.execution, members.record);
        if !routes.is_live(route) {
            continue;
        }
        let Ok(entity) = instances.resolve_body(members.execution) else {
            continue;
        };
        let Ok((controller, aabb, mut work)) = executions.get_mut(entity) else {
            continue;
        };
        if assemblies.life_for_controller(controller.0) != Some(life)
            || !eligibility.permits(controller.0, route)
        {
            work.clear();
            continue;
        }
        let Some(motion) = motions.executed_motion(route) else {
            continue;
        };
        let pose = motion.experience();
        let origin = aabb.center_at(Vec3::from_array(*pose.translation()));
        for _ in 0..BEHAVIOR_CAPACITY {
            if !world.p1().can_submit_and_commit() {
                break;
            }
            let Some(behavior) = work.0.pop_front() else {
                break;
            };
            // The queued intent retains identity, not a permission snapshot.
            // Current source removal or loss of custody rejects this use.
            let Some(reach) = item_data.reach(behavior.use_item.item, behavior.use_item.method)
            else {
                bevy::log::debug!(
                    "item use rejected: instance={}, method={:?}, reason=missing_current_permission_or_custody",
                    behavior.use_item.item.as_u128(),
                    behavior.use_item.method
                );
                continue;
            };
            let operation = match behavior.use_item.method {
                ItemUseMethod::DestroyBlock => BlockInteractionOperation::destroy(),
                ItemUseMethod::PlaceBlock => {
                    let Ok(operation) =
                        BlockInteractionOperation::place(registry.default_placed_voxel())
                    else {
                        continue;
                    };
                    operation
                }
            };
            let Ok(direction) = Dir3::new(Quat::from_array(behavior.gaze) * Vec3::NEG_Z) else {
                continue;
            };
            let ray = Ray3d::new(origin, direction);
            let Some(hit) = world.p0().cast(ray, reach) else {
                continue;
            };
            let coordinate_entity = hit.chunk.local_coordinate_entity();
            let Ok(coordinate) = world.p2().bind_coordinate(coordinate_entity) else {
                continue;
            };
            let position = if operation.is_destroy() {
                hit.voxel_position()
            } else {
                hit.previous_voxel_position
            };
            if !operation.is_destroy() {
                if !registry.permits_placement(operation.replacement()) {
                    continue;
                }
                let Ok(transform) = coordinates.get(coordinate_entity) else {
                    continue;
                };
                if super::minimal_application::world_execution::intersects_aabb(
                    position, transform, pose, *aabb,
                ) {
                    continue;
                }
            }
            let mut local = world.p1();
            let Ok(target) = local.observe(coordinate, position) else {
                continue;
            };
            if !operation.is_destroy() && target.expected().voxel != EMPTY_VOXEL_ID {
                continue;
            }
            let Ok(edit) = local.submit(target, operation.replacement()) else {
                continue;
            };
            if routes.is_live(route)
                && assemblies.life_for_controller(controller.0) == Some(life)
                && eligibility.permits(controller.0, route)
            {
                local.commit_front(edit).expect("reserved exact LC head");
            } else {
                local
                    .reject_front(
                        edit,
                        roundo_local_coordinate::VoxelEditRejection::NotEligible,
                    )
                    .expect("reserved exact LC head");
            }
            let completion = local
                .take_completion(edit)
                .expect("claim exact LC completion");
            bevy::log::debug!(
                "item use completed: instance={}, method={:?}, completion={:?}",
                behavior.use_item.item.as_u128(),
                behavior.use_item.method,
                completion
            );
        }
    }
}
