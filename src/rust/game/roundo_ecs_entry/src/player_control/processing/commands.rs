//! Native data adapters. Authorization and FIFO aggregation stay with the owner;
//! these systems recheck live routing and typed registration at the actual write.

pub(super) use super::reads::ControllerCommandRoutes;
use super::*;
use bevy::{
    ecs::system::SystemParam,
    prelude::{In, Query, Res, ResMut},
};
use roundo_life::{LifeEventReader, LifeEventWriter, LifeStateWriter};

pub(super) fn publish_movement(
    In((id, local_axis)): In<(ControllerId, [f32; 3])>,
    routes: ControllerCommandRoutes,
    mut movement: LifeStateWriter<MovementCommandState>,
) -> bool {
    let Ok(value) = MovementCommandState::new(local_axis) else {
        return false;
    };
    let Some(route) = routes.resolve(id) else {
        return false;
    };
    if !route
        .domain
        .contains(roundo_controller::IntentKind::Movement)
    {
        return false;
    }
    let Ok(intent) = routes.instances.resolve_body(route.intent) else {
        return false;
    };
    routes
        .instances
        .with_body(intent, |body| movement.set_state(&body, route.life, value))
        .is_ok_and(|result| result.is_ok())
}

pub(super) fn publish_gaze(
    In((id, yaw_delta, pitch_delta)): In<(ControllerId, f32, f32)>,
    routes: ControllerCommandRoutes,
    mut gaze: LifeEventWriter<GazeCommandEvent>,
) -> bool {
    let Ok(value) = GazeCommandEvent::new(yaw_delta, pitch_delta) else {
        return false;
    };
    let Some(route) = routes.resolve(id) else {
        return false;
    };
    if !route.domain.contains(roundo_controller::IntentKind::Gaze) {
        return false;
    }
    let Ok(intent) = routes.instances.resolve_body(route.intent) else {
        return false;
    };
    routes
        .instances
        .with_body(intent, |body| gaze.write_event(&body, route.life, value, 1))
        .is_ok_and(|result| result.is_ok())
}

pub(super) fn publish_set_gaze(
    In((id, orientation)): In<(ControllerId, [f32; 4])>,
    routes: ControllerCommandRoutes,
    mut gaze: LifeStateWriter<roundo_creature::GazeCommandState>,
) -> bool {
    let Ok(value) = roundo_creature::GazeCommandState::new(orientation) else {
        return false;
    };
    let Some(route) = routes.resolve(id) else {
        return false;
    };
    if !route.domain.contains(roundo_controller::IntentKind::Gaze) {
        return false;
    }
    let Ok(intent) = routes.instances.resolve_body(route.intent) else {
        return false;
    };
    routes
        .instances
        .with_body(intent, |body| gaze.set_state(&body, route.life, value))
        .is_ok_and(|result| result.is_ok())
}

pub(super) fn publish_interaction(
    In((id, command)): In<(ControllerId, roundo_block_interaction::ItemUseCommandEvent)>,
    routes: ControllerCommandRoutes,
    mut events: LifeEventWriter<roundo_block_interaction::ItemUseCommandEvent>,
) -> bool {
    let Some(route) = routes.resolve(id) else {
        return false;
    };
    if !route
        .domain
        .contains(roundo_controller::IntentKind::BlockInteraction)
    {
        return false;
    }
    let Ok(intent) = routes.instances.resolve_body(route.intent) else {
        return false;
    };
    routes
        .instances
        .with_body(intent, |body| {
            events.write_event(&body, route.life, command, 1)
        })
        .is_ok_and(|result| result.is_ok())
}

/// Preserves the old ordered, domain-scoped semantics: typed neutralization must
/// succeed before clearing that domain's Action. This is not a cross-Slot atomic
/// transaction; accepted State writes/cursor advances are not rolled back when
/// an Action is missing or a later domain fails.
#[derive(SystemParam)]
pub(super) struct CommandNeutralization<'w, 's> {
    routes: ControllerCommandRoutes<'w, 's>,
    movement: LifeStateWriter<'w, 's, MovementCommandState>,
    gaze: LifeEventReader<'w, 's, GazeCommandEvent>,
    movement_actions: Query<'w, 's, &'static mut MovementAction>,
    gaze_actions: Query<'w, 's, &'static mut GazeAction>,
    interactions: LifeEventReader<'w, 's, roundo_block_interaction::ItemUseCommandEvent>,
    behaviors: Query<
        'w,
        's,
        &'static mut crate::authoritative_creature::player_interaction::PlayerBlockBehaviors,
    >,
}

impl CommandNeutralization<'_, '_> {
    fn apply(&mut self, id: ControllerId) -> bool {
        let Some(route) = self.routes.resolve(id) else {
            return false;
        };
        let Ok(execution) = self.routes.instances.resolve_body(route.execution) else {
            return false;
        };
        if route
            .domain
            .contains(roundo_controller::IntentKind::Movement)
        {
            let Ok(intent) = self.routes.instances.resolve_body(route.intent) else {
                return false;
            };
            if !self
                .routes
                .instances
                .with_body(intent, |body| {
                    self.movement
                        .set_state(&body, route.life, MovementCommandState::default())
                })
                .is_ok_and(|result| result.is_ok())
            {
                return false;
            }
            let Ok(mut action) = self.movement_actions.get_mut(execution) else {
                return false;
            };
            *action = MovementAction::default();
        }
        if route.domain.contains(roundo_controller::IntentKind::Gaze) {
            if !self
                .routes
                .instances
                .with_body(execution, |body| {
                    self.gaze.read_next_with(&body, route.life, |_| ())
                })
                .is_ok_and(|result| result.is_ok())
            {
                return false;
            }
            let Ok(mut action) = self.gaze_actions.get_mut(execution) else {
                return false;
            };
            *action = GazeAction::default();
        }
        if route
            .domain
            .contains(roundo_controller::IntentKind::BlockInteraction)
        {
            let drained = self
                .routes
                .instances
                .with_body(execution, |body| {
                    loop {
                        match self.interactions.read_next_with(&body, route.life, |_| ()) {
                            Ok(Some(())) => {}
                            Ok(None) => return true,
                            Err(_) => return false,
                        }
                    }
                })
                .is_ok_and(|result| result);
            if !drained {
                return false;
            }
            let Ok(mut work) = self.behaviors.get_mut(execution) else {
                return false;
            };
            work.clear();
        }
        true
    }
}

pub(super) fn neutralize(In(id): In<ControllerId>, mut commands: CommandNeutralization) -> bool {
    commands.apply(id)
}

/// Select the Player's gate-owned Controllers before admission revocation.
/// Selection is not new authorization: each data operation still rechecks its
/// live route and registration. Clear all sequence maps even on typed failure,
/// continue with other selected Controllers, and return selected IDs (not a
/// success certificate) so the FIFO owner discards exactly their pending input.
/// This does not mutate gates, Access, bindings, or the shared Life clock.
pub(super) fn neutralize_player(
    In(player): In<PlayerId>,
    mut commands: CommandNeutralization,
    admission: Res<Admission>,
    mut sequences: ResMut<ControllerInputSequences>,
) -> Vec<ControllerId> {
    let selected = commands.routes.gate_owned_by_player(&admission, player);
    for &id in &selected {
        commands.apply(id);
        sequences.forget_controller(id);
    }
    selected
}
