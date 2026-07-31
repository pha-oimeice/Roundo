use bevy::{prelude::*, transform::TransformSystems};
use roundo_local_coordinate::{VoxelRaycastHit, VoxelRaycaster};
use roundo_marionette::ClientPlayerController;
use roundo_user_config::{
    DEFAULT_VOXEL_RAYCAST_DISTANCE, MAX_VOXEL_RAYCAST_DISTANCE, MIN_VOXEL_RAYCAST_DISTANCE,
};

const HIGHLIGHT_EDGE_LENGTH: f32 = 1.025;
const HIGHLIGHT_ALPHA: f32 = 0.22;

pub(crate) struct ClientVoxelTargetingPlugin;

impl Plugin for ClientVoxelTargetingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientVoxelRaycastSettings>()
            .init_resource::<ClientVoxelTarget>()
            .init_resource::<VoxelHighlightState>()
            .configure_sets(
                PostUpdate,
                ClientVoxelTargetingSet.after(TransformSystems::Propagate),
            )
            .add_systems(
                PostUpdate,
                update_voxel_target.in_set(ClientVoxelTargetingSet),
            );
    }
}

#[derive(SystemSet, Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ClientVoxelTargetingSet;

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClientVoxelRaycastSettings {
    max_distance: f32,
}

impl ClientVoxelRaycastSettings {
    pub(crate) fn new(max_distance: f32) -> Self {
        let mut settings = Self::default();
        settings.set_max_distance(max_distance);
        settings
    }

    pub(crate) fn max_distance(self) -> f32 {
        self.max_distance
    }

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
pub(crate) struct ClientVoxelTarget {
    pub(crate) hit: Option<VoxelRaycastHit>,
}

#[derive(Component)]
struct VoxelTargetHighlight;

#[derive(Resource, Default)]
struct VoxelHighlightState {
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
}

fn update_voxel_target(
    mut commands: Commands,
    player_controller: Res<ClientPlayerController>,
    settings: Res<ClientVoxelRaycastSettings>,
    raycaster: VoxelRaycaster,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    local_coordinates: Query<&GlobalTransform>,
    mut target: ResMut<ClientVoxelTarget>,
    mut highlight: ResMut<VoxelHighlightState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

    let Some(hit) = hit else {
        if let Some(entity) = highlight.entity {
            commands.entity(entity).insert(Visibility::Hidden);
        }
        return;
    };
    let Ok(local_coordinate_transform) = local_coordinates.get(hit.chunk.local_coordinate_entity())
    else {
        if let Some(entity) = highlight.entity {
            commands.entity(entity).insert(Visibility::Hidden);
        }
        return;
    };

    let local_transform = Transform::from_translation(hit.voxel.position.as_vec3() + 0.5);
    let world_transform = local_coordinate_transform.mul_transform(local_transform);
    let entity = match highlight.entity {
        Some(entity) => entity,
        None => {
            let mesh = highlight
                .mesh
                .get_or_insert_with(|| {
                    meshes.add(Cuboid::new(
                        HIGHLIGHT_EDGE_LENGTH,
                        HIGHLIGHT_EDGE_LENGTH,
                        HIGHLIGHT_EDGE_LENGTH,
                    ))
                })
                .clone();
            let material = highlight
                .material
                .get_or_insert_with(|| {
                    materials.add(StandardMaterial {
                        base_color: Color::srgba(1.0, 1.0, 1.0, HIGHLIGHT_ALPHA),
                        alpha_mode: AlphaMode::Blend,
                        unlit: true,
                        ..default()
                    })
                })
                .clone();
            let entity = commands
                .spawn((VoxelTargetHighlight, Mesh3d(mesh), MeshMaterial3d(material)))
                .id();
            highlight.entity = Some(entity);
            entity
        }
    };
    commands.entity(entity).insert((
        world_transform.compute_transform(),
        world_transform,
        Visibility::Visible,
    ));
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
