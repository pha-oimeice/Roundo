//! Projects presence entities into lightweight render objects.

use crate::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjectSync, RenderObjects,
    RenderTransform, issue_render_object, remove_render_object, update_render_object_name,
    update_render_object_transform, update_render_object_visibility,
};
use bevy::prelude::{
    App, Entity, IntoScheduleConfigs, Name, Plugin, PostUpdate, Quat, Query, ResMut, Resource,
    Transform, Vec3,
};
use roundo_presence::{ClientJoinableWorld, ClientPlayerMarker};
use std::collections::{HashMap, HashSet};

const CREATURE_MODEL_SIZE: Vec3 = Vec3::new(0.8, 1.8, 0.8);
const CREATURE_GAZE_LENGTH: f32 = 4.0;

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

// Synchronizes both player markers and joinable-world markers in one pass.
fn sync_presence_visuals(
    mut registry: ResMut<PresenceVisualRegistry>,
    mut render_objects: ResMut<RenderObjects>,
    players: Query<(Entity, &ClientPlayerMarker, &Transform, Option<&Name>)>,
    worlds: Query<(Entity, &ClientJoinableWorld, &Transform, Option<&Name>)>,
) {
    let mut observed = HashSet::new();

    // Presence is a visual projection, not the physical Creature. The upright box
    // symbolically matches the Creature's capsule bounds without pretending to be
    // its collider; body orientation therefore does not spin with gaze.
    for (entity, marker, transform, name) in &players {
        observed.insert(entity);
        let body_transform =
            RenderTransform::from_parts(transform.translation, Quat::IDENTITY, transform.scale);
        let gaze_transform =
            RenderTransform::from_parts(transform.translation, transform.rotation, transform.scale);
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
            update_render_object_visibility(&mut render_objects, gaze, marker.visible);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_object;
    use bevy::prelude::{App, PostUpdate};
    use roundo_presence::PlayerId;

    #[test]
    fn player_visual_has_upright_body_and_gaze_line() {
        let mut app = App::new();
        app.init_resource::<RenderObjects>()
            .init_resource::<PresenceVisualRegistry>()
            .add_systems(PostUpdate, sync_presence_visuals);
        let gaze = Quat::from_rotation_y(0.75);
        let player = app
            .world_mut()
            .spawn((
                ClientPlayerMarker {
                    player_id: PlayerId(7),
                    visible: true,
                },
                Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(gaze),
            ))
            .id();

        app.world_mut().run_schedule(PostUpdate);

        let registry = app.world().resource::<PresenceVisualRegistry>();
        let visual = registry.objects[&player];
        let objects = app.world().resource::<RenderObjects>();
        let body = render_object(objects, visual.body).unwrap();
        let gaze_object = render_object(objects, visual.gaze.unwrap()).unwrap();
        assert_eq!(body.transform().rotation, Quat::IDENTITY);
        assert_eq!(body.mesh().vertex_count(), 24);
        assert_eq!(gaze_object.transform().rotation, gaze);
        assert_eq!(gaze_object.mesh().vertex_count(), 2);
    }
}
