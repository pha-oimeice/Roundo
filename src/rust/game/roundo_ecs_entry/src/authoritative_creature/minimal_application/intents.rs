//! Trusted local semantic submission, only inside the explicit current-tick window.
use super::*;
use bevy::ecs::system::SystemParam;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinimalLifeSubmitError {
    Inactive,
    InvalidRoute,
    OutsideSubmissionPhase,
    InvalidValue,
    Factory(FactoryError),
    State(StateError),
    Event(EventError),
}
/// Hosts place their driver in MinimalLifeApplicationSet::Intents. Raw Handles
/// do not bind provenance or confer data permission; actual publication uses
/// approved native writers and freshly bound live Intent instances.
#[derive(SystemParam)]
pub struct MinimalLifeIntents<'w, 's> {
    apps: ResMut<'w, Applications>,
    window: Res<'w, SubmissionWindow>,
    tick: Res<'w, AuthoritativeFixedTick>,
    instances: LifeInstances<'w, 's>,
    owners: Query<'w, 's, (&'static ApplicationOwner, &'static IntentBodyId)>,
    data: ParamSet<
        'w,
        's,
        (
            LifeStateWriter<'w, 's, MovementCommandState>,
            LifeEventWriter<'w, 's, GazeCommandEvent>,
            LifeEventWriter<'w, 's, JumpCommandEvent>,
        ),
    >,
}
impl MinimalLifeIntents<'_, '_> {
    fn subject(
        &mut self,
        id: InteractionActivationId,
    ) -> Result<MinimalLifeMembers, MinimalLifeSubmitError> {
        let entry = self
            .apps
            .entries
            .get(&id)
            .ok_or(MinimalLifeSubmitError::Inactive)?;
        if entry.status != MinimalLifeStatus::Active {
            return Err(MinimalLifeSubmitError::Inactive);
        }
        if self.window.0 != Some(*self.tick) {
            return Err(MinimalLifeSubmitError::OutsideSubmissionPhase);
        }
        let intent = self
            .instances
            .resolve_body(entry.members.intent)
            .map_err(MinimalLifeSubmitError::Factory)?;
        if self.owners.get(intent).map(|(owner, _)| owner.0).ok() != Some(id) {
            return Err(MinimalLifeSubmitError::InvalidRoute);
        }
        // Native publication validates the Intent writer's own current Life
        // registration. Execution routing, coordinate readiness and Recorder
        // registrations are deliberately not Command acceptance conditions.
        Ok(entry.members)
    }
    pub fn movement(
        &mut self,
        id: InteractionActivationId,
        right: f32,
        forward: f32,
    ) -> Result<(), MinimalLifeSubmitError> {
        let value = horizontal_movement_command(right, forward)
            .map_err(|_| MinimalLifeSubmitError::InvalidValue)?;
        let members = self.subject(id)?;
        let entity = self
            .instances
            .resolve_body(members.intent)
            .map_err(MinimalLifeSubmitError::Factory)?;
        let mut writer = self.data.p0();
        self.instances
            .with_body(entity, |body| writer.set_state(&body, members.life, value))
            .map_err(MinimalLifeSubmitError::Factory)?
            .map_err(MinimalLifeSubmitError::State)
    }
    pub fn gaze(
        &mut self,
        id: InteractionActivationId,
        yaw: f32,
        pitch: f32,
    ) -> Result<(), MinimalLifeSubmitError> {
        let value =
            GazeCommandEvent::new(yaw, pitch).map_err(|_| MinimalLifeSubmitError::InvalidValue)?;
        let members = self.subject(id)?;
        let previous = self.apps.entries[&id].gaze_total;
        let total = [previous[0] + yaw, previous[1] + pitch];
        if !total.iter().all(|value| value.is_finite()) {
            return Err(MinimalLifeSubmitError::InvalidValue);
        }
        let entity = self
            .instances
            .resolve_body(members.intent)
            .map_err(MinimalLifeSubmitError::Factory)?;
        let mut writer = self.data.p1();
        self.instances
            .with_body(entity, |body| {
                writer.write_event(&body, members.life, value, 1)
            })
            .map_err(MinimalLifeSubmitError::Factory)?
            .map_err(MinimalLifeSubmitError::Event)?;
        self.apps.entries.get_mut(&id).unwrap().gaze_total = total;
        Ok(())
    }
    pub fn jump(&mut self, id: InteractionActivationId) -> Result<(), MinimalLifeSubmitError> {
        let members = self.subject(id)?;
        let entity = self
            .instances
            .resolve_body(members.intent)
            .map_err(MinimalLifeSubmitError::Factory)?;
        let mut writer = self.data.p2();
        self.instances
            .with_body(entity, |body| {
                writer.write_event(&body, members.life, JumpCommandEvent, 1)
            })
            .map_err(MinimalLifeSubmitError::Factory)?
            .map_err(MinimalLifeSubmitError::Event)
    }
}
