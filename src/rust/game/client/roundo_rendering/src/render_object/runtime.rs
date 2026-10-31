//! Materializes logical render objects as Bevy entities and assets.

use super::*;

// Synchronization runs after transforms and before rendering extraction.
impl Plugin for RenderObjectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<RenderObjects>()
            .init_resource::<RenderObjectRuntime>()
            .add_systems(
                PostUpdate,
                sync_render_objects
                    .in_set(RenderObjectSync)
                    .after(TransformSystems::Propagate),
            );
    }
}

#[derive(Component)]
/// Marks entities owned exclusively by the render-object runtime.
struct RenderObjectEntity;

/// Runtime handles and applied revisions for one logical object.
struct RuntimeObject {
    entity: Entity,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    name_revision: u64,
    transform_revision: u64,
    material_revision: u64,
    mesh_revision: u64,
    visibility_revision: u64,
}

#[derive(Resource, Default)]
/// Maps stable object identities to their Bevy runtime state.
struct RenderObjectRuntime {
    objects: HashMap<RenderObjectId, RuntimeObject>,
}

// Reconciles removals, creations, and revision-scoped updates.
fn sync_render_objects(
    mut commands: Commands,
    render_objects: Res<RenderObjects>,
    mut runtime: ResMut<RenderObjectRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Runtime entities own their mesh and material assets.
    let removed = runtime
        .objects
        .keys()
        .filter(|id| !render_objects.objects.contains_key(id))
        .copied()
        .collect::<Vec<_>>();
    for id in removed {
        let removed_object = runtime.objects.remove(&id);
        let object = removed_object.expect("collected runtime render object must exist");
        commands.entity(object.entity).despawn();
        let removed_mesh = meshes.remove(object.mesh.id());
        let removed_material = materials.remove(object.material.id());
        if removed_mesh.is_none() || removed_material.is_none() {
            bevy::log::warn!(
                "Render Object runtime assets were already absent: id={id:?}, mesh_present={}, material_present={}",
                removed_mesh.is_some(),
                removed_material.is_some()
            );
        }
    }

    // New objects are fully materialized before incremental revision checks.
    for (&id, entry) in &render_objects.objects {
        let Some(runtime_object) = runtime.objects.get_mut(&id) else {
            let mesh = meshes.add(entry.object.mesh.as_bevy().clone());
            let material = materials.add(entry.object.material.as_bevy().clone());
            let transform = Transform::from(entry.object.transform);
            let entity = commands
                .spawn((
                    RenderObjectEntity,
                    Name::new(entry.object.name.clone()),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    transform,
                    GlobalTransform::from(transform),
                    visibility(entry.object.visible),
                ))
                .id();
            runtime.objects.insert(
                id,
                RuntimeObject {
                    entity,
                    mesh,
                    material,
                    name_revision: entry.name_revision,
                    transform_revision: entry.transform_revision,
                    material_revision: entry.material_revision,
                    mesh_revision: entry.mesh_revision,
                    visibility_revision: entry.visibility_revision,
                },
            );
            continue;
        };

        // Each property revision avoids replacing unrelated Bevy components.
        if runtime_object.name_revision != entry.name_revision {
            commands
                .entity(runtime_object.entity)
                .insert(Name::new(entry.object.name.clone()));
            runtime_object.name_revision = entry.name_revision;
        }
        if runtime_object.transform_revision != entry.transform_revision {
            let transform = Transform::from(entry.object.transform);
            commands
                .entity(runtime_object.entity)
                .insert((transform, GlobalTransform::from(transform)));
            runtime_object.transform_revision = entry.transform_revision;
        }
        // Preserve handles when the backing asset remains available.
        if runtime_object.mesh_revision != entry.mesh_revision {
            if let Some(mut mesh) = meshes.get_mut(&runtime_object.mesh) {
                *mesh = entry.object.mesh.as_bevy().clone();
            } else {
                runtime_object.mesh = meshes.add(entry.object.mesh.as_bevy().clone());
                commands
                    .entity(runtime_object.entity)
                    .insert(Mesh3d(runtime_object.mesh.clone()));
            }
            runtime_object.mesh_revision = entry.mesh_revision;
        }
        // Recreate externally removed assets without changing object identity.
        if runtime_object.material_revision != entry.material_revision {
            if let Some(mut material) = materials.get_mut(&runtime_object.material) {
                *material = entry.object.material.as_bevy().clone();
            } else {
                runtime_object.material = materials.add(entry.object.material.as_bevy().clone());
                commands
                    .entity(runtime_object.entity)
                    .insert(MeshMaterial3d(runtime_object.material.clone()));
            }
            runtime_object.material_revision = entry.material_revision;
        }
        if runtime_object.visibility_revision != entry.visibility_revision {
            commands
                .entity(runtime_object.entity)
                .insert(visibility(entry.object.visible));
            runtime_object.visibility_revision = entry.visibility_revision;
        }
    }
}

// Logical visibility maps directly to explicit Bevy visibility.
fn visibility(visible: bool) -> Visibility {
    if visible {
        Visibility::Visible
    } else {
        Visibility::Hidden
    }
}
