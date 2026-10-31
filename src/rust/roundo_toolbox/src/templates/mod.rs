//! Reusable request and transport primitives.

mod crossbeam_thread_pipe;
mod crud_request;

/// Stable public surface for toolbox templates.
pub use self::{
    crossbeam_thread_pipe::{
        CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
    },
    crud_request::CRUDRequest,
};
