//! Non-structural Event control plane; typed Interface installation stays with Factory.
use super::*;
use crate::admin::RegistrationReverseIndex;
use bevy::prelude::Has;

/// Trusted typed Event approval/registration management. No World, callback,
/// arbitrary Commands or Interface installation is exposed. Registration truth
/// stays in the Life-held Interface; the reverse index is cleanup metadata only.
#[derive(SystemParam)]
pub struct LifeEventManagement<'w, 's, E: EventData> {
    instances: LifeInstances<'w, 's>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
    declarations: Query<
        'w,
        's,
        (
            Has<EventWriterDeclaration<E>>,
            Has<EventReaderDeclaration<E>>,
        ),
    >,
    interfaces: Query<'w, 's, &'static mut EventInterface<E>>,
    reverse: ResMut<'w, RegistrationReverseIndex>,
}

impl<E: EventData> LifeEventManagement<'_, '_, E> {
    fn entities(
        &self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(Entity, Entity), EventError> {
        let life = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        let body = self
            .instances
            .resolve_body(body)
            .map_err(|_| EventError::BodyNotFound)?;
        Ok((life, body))
    }

    pub fn approve_writer(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.0) {
            return Err(EventError::MissingWriterDeclaration);
        }
        let mut interface = self
            .interfaces
            .get_mut(life_entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        if !interface.approved_writers.contains_key(&body) {
            let generation = self.reverse.record(
                body,
                life,
                E::KEY,
                RegistrationDirection::ApprovalWriter,
                cleanup_event_writer_approval::<E>,
            );
            interface.approved_writers.insert(body, generation);
        }
        Ok(())
    }

    pub fn approve_reader(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.1) {
            return Err(EventError::MissingReaderDeclaration);
        }
        let mut interface = self
            .interfaces
            .get_mut(life_entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        if !interface.approved_readers.contains_key(&body) {
            let generation = self.reverse.record(
                body,
                life,
                E::KEY,
                RegistrationDirection::ApprovalReader,
                cleanup_event_reader_approval::<E>,
            );
            interface.approved_readers.insert(body, generation);
        }
        Ok(())
    }

    pub fn register_writer(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.0) {
            return Err(EventError::MissingWriterDeclaration);
        }
        let mut interface = self
            .interfaces
            .get_mut(life_entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        if !interface.approved_writers.contains_key(&body) {
            return Err(EventError::WriterNotApproved);
        }
        if interface.writer.is_some() {
            return Err(EventError::WriterAlreadyRegistered);
        }
        let access = interface
            .data
            .grant_modify()
            .map_err(EventError::ResourceAccess)?;
        let generation = self.reverse.record(
            body,
            life,
            E::KEY,
            RegistrationDirection::Writer,
            cleanup_event_writer::<E>,
        );
        interface.writer = Some((body, Registration { generation, access }));
        Ok(())
    }

    /// A newly registered reader starts at the first currently unexpired entry,
    /// not at another reader's cursor. Registration never consumes payloads.
    pub fn register_reader(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let tick = self
            .tick
            .as_ref()
            .ok_or(EventError::TypeNotRegistered)?
            .get();
        let (life_entity, body_entity) = self.entities(life, body)?;
        if !self.declarations.get(body_entity).is_ok_and(|decl| decl.1) {
            return Err(EventError::MissingReaderDeclaration);
        }
        let mut interface = self
            .interfaces
            .get_mut(life_entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        if !interface.approved_readers.contains_key(&body) {
            return Err(EventError::ReaderNotApproved);
        }
        if interface.readers.contains_key(&body) {
            return Err(EventError::ReaderAlreadyRegistered);
        }
        let cursor = interface.data.inspect(|data| {
            data.entries
                .iter()
                .find(|(_, entry)| tick < entry.expires_at)
                .map_or(data.next_sequence, |(&sequence, _)| sequence)
        });
        let access = interface
            .data
            .grant_read()
            .map_err(EventError::ResourceAccess)?;
        let generation = self.reverse.record(
            body,
            life,
            E::KEY,
            RegistrationDirection::Reader,
            cleanup_event_reader::<E>,
        );
        interface
            .readers
            .insert(body, (cursor, Registration { generation, access }));
        Ok(())
    }

    pub fn deregister_writer(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        let mut interface = self
            .interfaces
            .get_mut(entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        if interface.writer.as_ref().map(|(registered, _)| *registered) != Some(body) {
            return Err(EventError::WriterNotRegistered);
        }
        let (_, writer) = interface.writer.take().unwrap();
        interface.data.revoke_modify(&writer.access);
        self.reverse
            .forget(body, life, E::KEY, RegistrationDirection::Writer);
        Ok(())
    }

    pub fn deregister_reader(
        &mut self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        let mut interface = self
            .interfaces
            .get_mut(entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        let (_, reader) = interface
            .readers
            .remove(&body)
            .ok_or(EventError::ReaderNotRegistered)?;
        interface.data.revoke_read(&reader.access);
        self.reverse
            .forget(body, life, E::KEY, RegistrationDirection::Reader);
        Ok(())
    }
}
