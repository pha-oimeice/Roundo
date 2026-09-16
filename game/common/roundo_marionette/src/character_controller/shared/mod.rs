//! Sequence-aware validation shared by controller domains.
mod controller;

pub use self::controller::{ControllerAction, ControllerError, EventController};
