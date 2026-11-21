//! Native data adapters. Authorization and FIFO aggregation stay with the owner;
//! these systems recheck live routing and typed registration at the actual write.

pub(super) use super::reads::ControllerCommandRoutes;
use super::*;
use bevy::{
    ecs::system::SystemParam,
    prelude::{In, Query},
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
        true
    }
}

pub(super) fn neutralize(
    In(id): In<ControllerId>,
    mut commands: CommandNeutralization,
) -> bool {
    commands.apply(id)
}
