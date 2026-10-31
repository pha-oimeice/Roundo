//! Projects presence markers and opted-in Creature bodies into debug render objects.

use crate::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjectSync, RenderObjects,
    RenderTransform, issue_render_object, remove_render_object, update_render_object_name,
    update_render_object_transform, update_render_object_visibility,
};
use bevy::prelude::{
    App, Component, Entity, IntoScheduleConfigs, Name, Plugin, PostUpdate, Quat, Query, Res,
    ResMut, Resource, Transform, Vec3,
};
use roundo_creature::{AabbDimensions, GazeOrientation};
use roundo_presence::{ClientJoinableWorld, ClientPlayerMarker, LocalPlayerIdentity};
use std::collections::{HashMap, HashSet};

const CREATURE_MODEL_SIZE: Vec3 = Vec3::new(0.8, 1.8, 0.8);
const CREATURE_GAZE_LENGTH: f32 = 4.0;

/// Renders the local predicted Creature instead of its Presence approximation.
/// Requires Transform, AabbDimensions, and GazeOrientation on the same entity.
/// The caller controls visibility; rendering never changes motion.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct LocalCreatureDebugVisual {
    /// Show the body and gaze when inspecting outside the first-person camera.
    pub visible: bool,
}

/// Installs post-update synchronization before render extraction.
pub struct PresenceVisualPlugin;

impl Plugin for PresenceVisualPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PresenceVisualRegistry>()
            .add_systems(PostUpdate, sync_presence_visuals.before(RenderObjectSync));
    }
}

#[derive(Clone, Copy)]
struct PresenceVisual {
    body: RenderObjectId,
    gaze: Option<RenderObjectId>,
}

#[derive(Resource, Default)]
/// Maintains the complete visual identity set for each observed ECS projection.
struct PresenceVisualRegistry {
    objects: HashMap<Entity, PresenceVisual>,
}

// Synchronizes source entities in one pass so removals share the same lifetime.
fn sync_presence_visuals(
    mut registry: ResMut<PresenceVisualRegistry>,
    mut render_objects: ResMut<RenderObjects>,
    players: Query<(Entity, &ClientPlayerMarker, &Transform, Option<&Name>)>,
    worlds: Query<(Entity, &ClientJoinableWorld, &Transform, Option<&Name>)>,
    creatures: Query<(
        Entity,
        &LocalCreatureDebugVisual,
        &Transform,
        &AabbDimensions,
        &GazeOrientation,
    )>,
    local_identity: Option<Res<LocalPlayerIdentity>>,
) {
    let mut observed = HashSet::new();
    let predicted_player = if creatures.is_empty() {
        None
    } else {
        local_identity
            .as_ref()
            .and_then(|identity| identity.player_id())
    };

    // Presence is a visual projection, not the physical Creature. The upright box
    // symbolically matches the Creature's AABB bounds without pretending to be
    // its collider; body orientation therefore does not spin with gaze.
    for (entity, marker, transform, name) in &players {
        // The local Execution has the actual collision pose. Do not also draw
        // its independently interpolated, fixed-size Presence approximation.
        if Some(marker.player_id) == predicted_player {
            continue;
        }
        observed.insert(entity);
        // Presence position is the logical bottom center. The centered render
        // AABB and gaze marker retain an explicit model-center offset.
        let model_center =
            transform.translation + Vec3::Y * (CREATURE_MODEL_SIZE.y * 0.5 * transform.scale.y);
        let body_transform =
            RenderTransform::from_parts(model_center, Quat::IDENTITY, transform.scale);
        let gaze_transform =
            RenderTransform::from_parts(model_center, transform.rotation, transform.scale);
        let visual = *registry.objects.entry(entity).or_insert_with(|| {
            let body = issue_render_object(
                &mut render_objects,
                RenderObject::new(
                    RenderMesh::cuboid(CREATURE_MODEL_SIZE),
                    RenderMaterial::unlit([1.0, 0.55, 0.05, 1.0]).with_perceptual_roughness(0.55),
                    body_transform,
                )
                .with_name(format!("Creature {}", marker.player_id.0)),
            );
            let gaze = issue_render_object(
                &mut render_objects,
                RenderObject::new(
                    RenderMesh::sight_line(CREATURE_GAZE_LENGTH),
                    RenderMaterial::unlit([0.2, 1.0, 0.25, 1.0]),
                    gaze_transform,
                )
                .with_name(format!("Creature {} Gaze", marker.player_id.0)),
            );
            PresenceVisual {
                body,
                gaze: Some(gaze),
            }
        });
        update_render_object_transform(&mut render_objects, visual.body, body_transform);
        update_render_object_visibility(&mut render_objects, visual.body, marker.visible);
        if let Some(gaze) = visual.gaze {
            update_render_object_transform(&mut render_objects, gaze, gaze_transform);
            // The authoritative gaze marker remains observable in first person.
            update_render_object_visibility(&mut render_objects, gaze, true);
        }
        if let Some(name) = name {
            update_render_object_name(&mut render_objects, visual.body, name.as_str());
            if let Some(gaze) = visual.gaze {
                update_render_object_name(
                    &mut render_objects,
                    gaze,
                    format!("{} Gaze", name.as_str()),
                );
            }
        }
    }

    for (entity, marker, transform, aabb, gaze) in &creatures {
        observed.insert(entity);
        // Movement size and world-axis alignment come from the same geometry
        // as the solver; neither gaze nor the locomotion frame rotates the box.
        let center = aabb.center_at(transform.translation);
        let body_transform = RenderTransform::from_parts(center, Quat::IDENTITY, aabb.size());
        let gaze_transform = RenderTransform::from_parts(center, gaze.0, transform.scale);
        let visual = *registry
            .objects
            .entry(entity)
            .or_insert_with(|| PresenceVisual {
                body: issue_render_object(
                    &mut render_objects,
                    RenderObject::new(
                        RenderMesh::cuboid(Vec3::ONE),
                        RenderMaterial::unlit([1.0, 0.55, 0.05, 1.0]),
                        body_transform,
                    )
                    .with_name("Predicted Creature Debug Body")
                    .with_visibility(marker.visible),
                ),
                gaze: Some(issue_render_object(
                    &mut render_objects,
                    RenderObject::new(
                        RenderMesh::sight_line(CREATURE_GAZE_LENGTH),
                        RenderMaterial::unlit([0.2, 1.0, 0.25, 1.0]),
                        gaze_transform,
                    )
                    .with_name("Predicted Creature Debug Gaze")
                    .with_visibility(marker.visible),
                )),
            });
        update_render_object_transform(&mut render_objects, visual.body, body_transform);
        update_render_object_visibility(&mut render_objects, visual.body, marker.visible);
        if let Some(gaze) = visual.gaze {
            update_render_object_transform(&mut render_objects, gaze, gaze_transform);
            update_render_object_visibility(&mut render_objects, gaze, marker.visible);
        }
    }

    // Joinable worlds remain visible and use a distinct spherical marker.
    for (entity, _, transform, name) in &worlds {
        observed.insert(entity);
        let render_transform = RenderTransform::from(*transform);
        let visual = *registry
            .objects
            .entry(entity)
            .or_insert_with(|| PresenceVisual {
                body: issue_render_object(
                    &mut render_objects,
                    RenderObject::new(
                        RenderMesh::uv_sphere(1.0, 32, 18),
                        RenderMaterial::unlit([0.12, 0.35, 0.95, 1.0])
                            .with_perceptual_roughness(0.7)
                            .with_metallic(0.1),
                        render_transform,
                    )
                    .with_name("Joinable World"),
                ),
                gaze: None,
            });
        update_render_object_transform(&mut render_objects, visual.body, render_transform);
        update_render_object_visibility(&mut render_objects, visual.body, true);
        if let Some(name) = name {
            update_render_object_name(&mut render_objects, visual.body, name.as_str());
        }
    }

    // Render objects outliving their source entity are removed in the same update.
    let removed = registry
        .objects
        .keys()
        .filter(|entity| !observed.contains(entity))
        .copied()
        .collect::<Vec<_>>();
    for entity in removed {
        if let Some(visual) = registry.objects.remove(&entity) {
            remove_render_object(&mut render_objects, visual.body);
            if let Some(gaze) = visual.gaze {
                remove_render_object(&mut render_objects, gaze);
            }
        }
    }
}
