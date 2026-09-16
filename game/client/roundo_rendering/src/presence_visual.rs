//! Projects presence entities into lightweight render objects.

use crate::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjectSync, RenderObjects,
    RenderTransform, issue_render_object, remove_render_object, update_render_object_name,
    update_render_object_transform, update_render_object_visibility,
};
use bevy::prelude::{
    App, Entity, IntoScheduleConfigs, Name, Plugin, PostUpdate, Query, ResMut, Resource, Transform,
};
use roundo_presence::{ClientJoinableWorld, ClientPlayerMarker};
use std::collections::{HashMap, HashSet};

/// Installs post-update synchronization before render extraction.
pub struct PresenceVisualPlugin;

impl Plugin for PresenceVisualPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PresenceVisualRegistry>()
            .add_systems(PostUpdate, sync_presence_visuals.before(RenderObjectSync));
    }
}

#[derive(Resource, Default)]
/// Maintains one render-object identity per observed ECS entity.
struct PresenceVisualRegistry {
    objects: HashMap<Entity, RenderObjectId>,
}

// Synchronizes both player markers and joinable-world markers in one pass.
fn sync_presence_visuals(
    mut registry: ResMut<PresenceVisualRegistry>,
    mut render_objects: ResMut<RenderObjects>,
    players: Query<(Entity, &ClientPlayerMarker, &Transform, Option<&Name>)>,
    worlds: Query<(Entity, &ClientJoinableWorld, &Transform, Option<&Name>)>,
) {
    let mut observed = HashSet::new();

    // Players use a compact warm-colored marker and respect presence visibility.
    for (entity, marker, transform, name) in &players {
        observed.insert(entity);
        let render_transform = RenderTransform::from(*transform);
        let id = *registry.objects.entry(entity).or_insert_with(|| {
            issue_render_object(
                &mut render_objects,
                RenderObject::new(
                    RenderMesh::tetrahedron(),
                    RenderMaterial::unlit([1.0, 0.55, 0.05, 1.0]).with_perceptual_roughness(0.55),
                    render_transform,
                )
                .with_name(format!("Player {}", marker.player_id.0)),
            )
        });
        update_render_object_transform(&mut render_objects, id, render_transform);
        update_render_object_visibility(&mut render_objects, id, marker.visible);
        if let Some(name) = name {
            update_render_object_name(&mut render_objects, id, name.as_str());
        }
    }

    // Joinable worlds remain visible and use a distinct spherical marker.
    for (entity, _, transform, name) in &worlds {
        observed.insert(entity);
        let render_transform = RenderTransform::from(*transform);
        let id = *registry.objects.entry(entity).or_insert_with(|| {
            issue_render_object(
                &mut render_objects,
                RenderObject::new(
                    RenderMesh::uv_sphere(1.0, 32, 18),
                    RenderMaterial::unlit([0.12, 0.35, 0.95, 1.0])
                        .with_perceptual_roughness(0.7)
                        .with_metallic(0.1),
                    render_transform,
                )
                .with_name("Joinable World"),
            )
        });
        update_render_object_transform(&mut render_objects, id, render_transform);
        update_render_object_visibility(&mut render_objects, id, true);
        if let Some(name) = name {
            update_render_object_name(&mut render_objects, id, name.as_str());
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
        if let Some(id) = registry.objects.remove(&entity) {
            remove_render_object(&mut render_objects, id);
        }
    }
}
