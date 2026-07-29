//! Chunked local-coordinate voxel storage and its derived mesh and collider.

use bevy::app::PluginGroupBuilder;
use bevy::prelude::PluginGroup;

mod base;
mod chunk;
pub mod data;
mod geometry;
mod pcg_algo;
mod physics;
mod render;
mod test;
#[cfg(test)]
mod tests;

pub mod msg {
    #[allow(unused_imports)]
    pub use super::{
        data::{LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum},
        test::LocalCoordinateTestMessage,
    };
}

pub mod plugins {
    #[allow(unused_imports)]
    pub use super::{
        LocalCoordinateClientPlugin, LocalCoordinateServerPlugin, base::LocalCoordinateBasePlugin,
        physics::LocalCoordinatePhysicsPlugin, render::LocalCoordinateRenderPlugin,
        test::LocalCoordinateTestPlugin,
    };
}

/// Server composition: shared voxel state plus authoritative collision.
pub struct LocalCoordinateServerPlugin;

impl PluginGroup for LocalCoordinateServerPlugin {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(base::LocalCoordinateBasePlugin)
            .add(physics::LocalCoordinatePhysicsPlugin)
    }
}

/// Client composition: shared voxel state plus visual mesh generation.
pub struct LocalCoordinateClientPlugin;

impl PluginGroup for LocalCoordinateClientPlugin {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(base::LocalCoordinateBasePlugin)
            .add(render::LocalCoordinateRenderPlugin)
    }
}
