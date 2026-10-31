use bevy::prelude::Component;

macro_rules! opaque_handle {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name {
            slot: u32,
            generation: u64,
        }

        impl $name {
            pub(crate) const fn new(slot: u32, generation: u64) -> Self {
                Self { slot, generation }
            }

            pub(crate) const fn slot(self) -> u32 {
                self.slot
            }

            pub(crate) const fn generation(self) -> u64 {
                self.generation
            }
        }
    };
}

opaque_handle!(LifeHandle);
opaque_handle!(ConceptBodyHandle);

#[derive(Component)]
pub(crate) struct LifeIdentity(pub(crate) LifeHandle);

/// ECS projection of a factory Body identity, used only to correlate lifecycle
/// notifications; the Handle remains the routing truth.
#[derive(Component)]
pub struct ConceptBodyIdentity(pub(crate) ConceptBodyHandle);

impl ConceptBodyIdentity {
    pub const fn handle(&self) -> ConceptBodyHandle {
        self.0
    }
}
