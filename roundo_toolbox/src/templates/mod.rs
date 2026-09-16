//! Reusable data structures and transport primitives.

mod bit_mask;
mod crossbeam_thread_pipe;
mod crud_request;
#[allow(unused)]
mod graph;
mod linear_interpolation;
mod registry;

/// Stable public surface for toolbox templates.
pub use self::{
    bit_mask::BitMask,
    crossbeam_thread_pipe::{
        CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
    },
    crud_request::CRUDRequest,
    linear_interpolation::LinearInterpolation,
    registry::{Registry, RegistryDefinitionTrait, RegistryInsertError},
};
