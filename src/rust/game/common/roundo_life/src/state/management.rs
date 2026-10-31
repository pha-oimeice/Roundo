//! Non-structural State control plane. Slot/policy materialization stays with Factory.
use super::*;
use crate::admin::RegistrationReverseIndex;
use bevy::prelude::{Entity, Has, ResMut};

/// Trusted typed State approval/registration management for ordinary systems.
/// This is not Body data access: it exposes no World, callback, declaration or
/// structural Slot installation. Every approval/registration checks live Factory
/// identities, and accepted changes update the cleanup index synchronously.
#[derive(SystemParam)]
pub struct LifeStateManagement<'w, 's, S: StateData> {
    instances: LifeInstances<'w, 's>,
    declarations: Query<
        'w,
        's,
        (
            Has<StateWriterDeclaration<S>>,
            Has<StateReaderDeclaration<S>>,
        ),
    >,
    admissions: Query<'w, 's, &'static mut StateAdmission<S>>,
    slots: Query<'w, 's, &'static mut StateSlot<S>>,
    reverse: ResMut<'w, RegistrationReverseIndex>,
}

impl<S: StateData> LifeStateManagement<'_, '_, S> {
    fn entities(
        &self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(Entity, Entity), StateError> {
        let life = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        let body = self
            .instances
            .resolve_body(body)
            .map_err(|_| StateError::BodyNotFound)?;
        Ok((life, body))
    }

    pub fn approve_origin(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.0) {
            return Err(StateError::MissingWriterDeclaration);
        }
        let mut admission = self
            .admissions
            .get_mut(life_entity)
            .map_err(|_| StateError::PolicyNotFound)?;
        if !admission.approved_origins.contains_key(&body) {
            let generation = self.reverse.record(
                body,
                life,
                S::KEY,
                RegistrationDirection::ApprovalWriter,
                cleanup_state_origin_approval::<S>,
            );
            admission.approved_origins.insert(body, generation);
        }
        Ok(())
    }

    pub fn approve_reader(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.1) {
            return Err(StateError::MissingReaderDeclaration);
        }
        let mut admission = self
            .admissions
            .get_mut(life_entity)
            .map_err(|_| StateError::PolicyNotFound)?;
        if !admission.approved_readers.contains_key(&body) {
            let generation = self.reverse.record(
                body,
                life,
                S::KEY,
                RegistrationDirection::ApprovalReader,
                cleanup_state_reader_approval::<S>,
            );
            admission.approved_readers.insert(body, generation);
        }
        Ok(())
    }

    pub fn register_reader(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.1) {
            return Err(StateError::MissingReaderDeclaration);
        }
        if !self
            .admissions
            .get(life_entity)
            .map_err(|_| StateError::PolicyNotFound)?
            .approved_readers
            .contains_key(&body)
        {
            return Err(StateError::ReaderNotApproved);
        }
        let mut slot = self
            .slots
            .get_mut(life_entity)
            .map_err(|_| StateError::SlotNotFound)?;
        if slot.readers.contains_key(&body) {
            return Err(StateError::ReaderAlreadyRegistered);
        }
        let access = slot
            .values
            .grant_read()
            .map_err(StateError::ResourceAccess)?;
        let generation = self.reverse.record(
            body,
            life,
            S::KEY,
            RegistrationDirection::Reader,
            cleanup_state_reader::<S>,
        );
        slot.readers
            .insert(body, Registration { generation, access });
        Ok(())
    }

    /// Deregistration accepts an already removed Body's exact old Handle. Life
    /// must still be live; an old generation never targets a replacement Body.
    pub fn deregister_reader(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        let mut slot = self
            .slots
            .get_mut(entity)
            .map_err(|_| StateError::SlotNotFound)?;
        let reader = slot
            .readers
            .remove(&body)
            .ok_or(StateError::ReaderNotRegistered)?;
        slot.values.revoke_read(&reader.access);
        self.reverse
            .forget(body, life, S::KEY, RegistrationDirection::Reader);
        Ok(())
    }

    /// Freezes values/history without removing readers or changing immutable origin.
    pub fn deregister_writer(
        &mut self,
        life: LifeHandle,
        origin: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        let mut slot = self
            .slots
            .get_mut(entity)
            .map_err(|_| StateError::SlotNotFound)?;
        if slot.origin != origin {
            return Err(StateError::OriginMismatch);
        }
        let writer = slot.writer.take().ok_or(StateError::WriterNotRegistered)?;
        slot.values.revoke_modify(&writer.access);
        self.reverse
            .forget(origin, life, S::KEY, RegistrationDirection::Writer);
        Ok(())
    }

    /// Only the same still-live immutable origin may resume a frozen Slot.
    pub fn rebind_writer(
        &mut self,
        life: LifeHandle,
        applicant: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        self.instances
            .resolve_body(applicant)
            .map_err(|_| StateError::BodyNotFound)?;
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        let mut slot = self
            .slots
            .get_mut(entity)
            .map_err(|_| StateError::SlotNotFound)?;
        if slot.origin != applicant {
            return Err(StateError::OriginMismatch);
        }
        if slot.writer.is_some() {
            return Err(StateError::WriterNotRegistered);
        }
        let access = slot
            .values
            .grant_modify()
            .map_err(StateError::ResourceAccess)?;
        let generation = self.reverse.record(
            applicant,
            life,
            S::KEY,
            RegistrationDirection::Writer,
            cleanup_state_writer::<S>,
        );
        slot.writer = Some(Registration { generation, access });
        Ok(())
    }
}
