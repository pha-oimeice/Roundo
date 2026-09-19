//! Camera-driven voxel selection and render-object highlighting.

use bevy::{prelude::*, transform::TransformSystems};
use roundo_local_coordinate::{VoxelRaycastHit, VoxelRaycaster};
use roundo_marionette::ClientPlayerController;
use roundo_rendering::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjectSync, RenderObjects,
    RenderTransform, issue_render_object, update_render_object_transform,
    update_render_object_visibility,
};
use roundo_user_config::{
    DEFAULT_VOXEL_RAYCAST_DISTANCE, MAX_VOXEL_RAYCAST_DISTANCE, MIN_VOXEL_RAYCAST_DISTANCE,
};

// Slight oversizing prevents the highlight shell from z-fighting with voxel faces.
const HIGHLIGHT_EDGE_LENGTH: f32 = 1.025;
const HIGHLIGHT_ALPHA: f32 = 0.22;

/// Installs post-transform raycasting before render-object synchronization.
pub(crate) struct ClientVoxelTargetingPlugin;

impl Plugin for ClientVoxelTargetingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientVoxelRaycastSettings>()
            .init_resource::<ClientVoxelTarget>()
            .init_resource::<VoxelHighlightState>()
            .configure_sets(
                PostUpdate,
                ClientVoxelTargetingSet
                    .after(TransformSystems::Propagate)
                    .before(RenderObjectSync),
            )
            .add_systems(
                PostUpdate,
                update_voxel_target.in_set(ClientVoxelTargetingSet),
            );
    }
}

#[derive(SystemSet, Clone, Debug, Eq, Hash, PartialEq)]
/// Ordering boundary for consumers of the current voxel target.
pub(crate) struct ClientVoxelTargetingSet;

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
/// Validated maximum distance for client targeting rays.
pub(crate) struct ClientVoxelRaycastSettings {
    max_distance: f32,
}

impl ClientVoxelRaycastSettings {
    /// Creates settings with protocol bounds applied.
    pub(crate) fn new(max_distance: f32) -> Self {
        let mut settings = Self::default();
        settings.set_max_distance(max_distance);
        settings
    }

    pub(crate) fn max_distance(self) -> f32 {
        self.max_distance
    }

    /// Replaces invalid values with the default and clamps finite values.
    pub(crate) fn set_max_distance(&mut self, max_distance: f32) -> f32 {
        self.max_distance = if max_distance.is_finite() {
            max_distance.clamp(MIN_VOXEL_RAYCAST_DISTANCE, MAX_VOXEL_RAYCAST_DISTANCE)
        } else {
            DEFAULT_VOXEL_RAYCAST_DISTANCE
        };
        self.max_distance
    }
}

impl Default for ClientVoxelRaycastSettings {
    fn default() -> Self {
        Self {
            max_distance: DEFAULT_VOXEL_RAYCAST_DISTANCE,
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
/// Latest voxel intersection exposed to interaction and HUD systems.
pub(crate) struct ClientVoxelTarget {
    pub(crate) hit: Option<VoxelRaycastHit>,
}

#[derive(Resource, Default)]
/// Retains one reusable render object for the selection shell.
struct VoxelHighlightState {
    render_object_id: Option<RenderObjectId>,
}

// Casts from the active camera and synchronizes target, HUD, and highlight.
fn update_voxel_target(
    player_controller: Res<ClientPlayerController>,
    settings: Res<ClientVoxelRaycastSettings>,
    raycaster: VoxelRaycaster,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    local_coordinates: Query<&GlobalTransform>,
    mut target: ResMut<ClientVoxelTarget>,
    mut hud: ResMut<crate::commands::HudCache>,
    mut highlight: ResMut<VoxelHighlightState>,
    mut render_objects: ResMut<RenderObjects>,
) {
    let hit = player_controller.input_enabled().then(|| {
        cameras
            .iter()
            .find(|(camera, _)| camera.is_active)
            .and_then(|(_, transform)| {
                raycaster.cast_from_camera(transform, settings.max_distance())
            })
    });
    let hit = hit.flatten();
    if target.hit != hit {
        target.hit = hit;
    }
    hud.set_target(hit.map(|hit| {
        let voxel = hit.voxel_position();
        let chunk = hit.chunk.local_chunk_position();
        serde_json::json!({
            "voxel": format!("{:?}", hit.voxel),
            "voxel_position": [voxel.x, voxel.y, voxel.z],
            "relative_position": [hit.voxel_relative_position.x, hit.voxel_relative_position.y, hit.voxel_relative_position.z],
            "chunk": [chunk.x, chunk.y, chunk.z],
            "local_coordinate": format!("{:?}", hit.chunk.local_coordinate_entity()),
        })
    }));

    // Missing targets hide rather than destroy the reusable highlight.
    let Some(hit) = hit else {
        if let Some(id) = highlight.render_object_id {
            update_render_object_visibility(&mut render_objects, id, false);
        }
        return;
    };
    let Ok(local_coordinate_transform) = local_coordinates.get(hit.chunk.local_coordinate_entity())
    else {
        if let Some(id) = highlight.render_object_id {
            update_render_object_visibility(&mut render_objects, id, false);
        }
        return;
    };

    let local_transform = Transform::from_translation(hit.voxel_position().as_vec3() + 0.5);
    let world_transform = local_coordinate_transform.mul_transform(local_transform);
    // Allocate lazily after the first valid hit.
    let render_object_id = match highlight.render_object_id {
        Some(id) => id,
        None => {
            let id = issue_render_object(
                &mut render_objects,
                RenderObject::new(
                    RenderMesh::cuboid(Vec3::splat(HIGHLIGHT_EDGE_LENGTH)),
                    RenderMaterial::unlit([1.0, 1.0, 1.0, HIGHLIGHT_ALPHA]).with_alpha_blend(),
                    RenderTransform::from(world_transform.compute_transform()),
                )
                .with_name("Voxel Target Highlight"),
            );
            highlight.render_object_id = Some(id);
            id
        }
    };
    update_render_object_transform(
        &mut render_objects,
        render_object_id,
        RenderTransform::from(world_transform.compute_transform()),
    );
    update_render_object_visibility(&mut render_objects, render_object_id, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voxel_raycast_distance_is_always_finite_and_capped_at_sixteen() {
        let mut settings = ClientVoxelRaycastSettings::new(100.0);
        assert_eq!(settings.max_distance(), MAX_VOXEL_RAYCAST_DISTANCE);

        settings.set_max_distance(f32::NAN);
        assert_eq!(settings.max_distance(), DEFAULT_VOXEL_RAYCAST_DISTANCE);
    }
}
