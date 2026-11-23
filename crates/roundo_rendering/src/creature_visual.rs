//! Projects opted-in local Creature bodies into debug render objects.
use crate::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjectSync, RenderObjects,
    RenderTransform, issue_render_object, remove_render_object, update_render_object_transform,
    update_render_object_visibility,
};
use bevy::prelude::{
    App, Component, Entity, IntoScheduleConfigs, Plugin, PostUpdate, Quat, Query, ResMut,
    Resource, Transform, Vec3,
};
use roundo_creature::{AabbDimensions, GazeOrientation};
use std::collections::{HashMap, HashSet};

const CREATURE_GAZE_LENGTH: f32 = 4.0;

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct LocalCreatureDebugVisual {
    pub visible: bool,
}

pub struct CreatureVisualPlugin;

impl Plugin for CreatureVisualPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CreatureVisualRegistry>()
            .add_systems(PostUpdate, sync_creature_visuals.before(RenderObjectSync));
    }
}

#[derive(Clone, Copy)]
struct CreatureVisual {
    body: RenderObjectId,
    gaze: RenderObjectId,
}

#[derive(Resource, Default)]
struct CreatureVisualRegistry {
    objects: HashMap<Entity, CreatureVisual>,
}

fn sync_creature_visuals(
    mut registry: ResMut<CreatureVisualRegistry>,
    mut render_objects: ResMut<RenderObjects>,
    creatures: Query<(
        Entity,
        &LocalCreatureDebugVisual,
        &Transform,
        &AabbDimensions,
        &GazeOrientation,
    )>,
) {
    let mut observed = HashSet::new();
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
            .or_insert_with(|| CreatureVisual {
                body: issue_render_object(
                    &mut render_objects,
                    RenderObject::new(
                        RenderMesh::cuboid(Vec3::ONE),
                        RenderMaterial::unlit([1.0, 0.55, 0.05, 1.0]),
                        body_transform,
                    )
                    .with_name("Creature Debug Body")
                    .with_visibility(marker.visible),
                ),
                gaze: issue_render_object(
                    &mut render_objects,
                    RenderObject::new(
                        RenderMesh::sight_line(CREATURE_GAZE_LENGTH),
                        RenderMaterial::unlit([0.2, 1.0, 0.25, 1.0]),
                        gaze_transform,
                    )
                    .with_name("Creature Debug Gaze")
                    .with_visibility(marker.visible),
                ),
            });
        update_render_object_transform(&mut render_objects, visual.body, body_transform);
        update_render_object_visibility(&mut render_objects, visual.body, marker.visible);
        update_render_object_transform(&mut render_objects, visual.gaze, gaze_transform);
        update_render_object_visibility(&mut render_objects, visual.gaze, marker.visible);
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
            remove_render_object(&mut render_objects, visual.gaze);
        }
    }
}
