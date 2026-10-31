//! First-class Life identity and typed Event/State collaboration runtime.

mod access;
mod admin;
pub mod attributes;
mod data;
mod event;
mod factory;
mod handle;
mod management;
mod resource;
mod scoped_management;
mod state;
mod structure;
mod tick;

pub use access::{BodyInstance, LifeInstances};
pub use attributes::{
    AttributeKind, AttributeValue, ChangeRate, ContinuityRate, CycleRate, EternityRate,
    ExecutionValue, Health, IntentValue, InvalidUnorm, RecordValue, Unorm,
};
pub use data::{
    __private, DataShape, EventData, EventShape, LifeData, LifeDataTypeKey, StateData, StateShape,
    TypeRegistrationError, close_life_data_catalogue,
};
pub use event::{
    EventError, EventInterfaceEvidence, EventPolicy, LifeEventManagement, LifeEventReader,
    LifeEventRegistrations, LifeEventWriter, approve_event_reader, approve_event_writer,
    declare_event_reader, declare_event_writer, deregister_event_reader, deregister_event_writer,
    install_event_interface, register_event_reader, register_event_type, register_event_writer,
    remove_event_interface,
};
pub use factory::{
    BodyCaller, ConceptBodyFactory, ConceptBodyPrototype, FactoryError, FactoryInvalidation,
    LifeAdministration, LifeFactory, LifePrototype, with_body_caller, with_life_administration,
};
pub use handle::{ConceptBodyHandle, ConceptBodyIdentity, LifeHandle};
pub use management::{
    FACTORY_REQUEST_CAPACITY, FactoryCompletion, FactoryManagement, FactoryRequestId,
    FactorySubmitError,
};
pub use scoped_management::{
    LifeCommitScope, LifeScopedManagement, ScopedLifeCompletion, ScopedLifeError,
    ScopedLifeRequestId, commit_scoped_life_request,
};
pub use state::{
    LifeStateManagement, LifeStateReader, LifeStateRegistrations, LifeStateWriter, StateError,
    StatePolicy, StateSlotEvidence, StateTickView, approve_state_origin, approve_state_reader,
    create_state_slot, declare_state_reader, declare_state_writer, deregister_state_reader,
    deregister_state_writer, install_state_policy, rebind_state_writer, register_state_reader,
    register_state_type, remove_state_slot,
};
pub use structure::{LifeStructureCompletion, LifeStructureManagement, LifeStructureRequestId};
pub(crate) use tick::FrozenStateHistoryConfig;
pub use tick::{AuthoritativeFixedTick, LifeFixedSet, LifeRuntimePlugin, StateHistoryConfig};
