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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_track_and_update_one_render_objects_data() {
        let mut objects = RenderObjects::default();
        let first = issue_render_object(&mut objects, test_object(3));
        let second = issue_render_object(&mut objects, test_object(6));

        assert_ne!(first, second);
        assert_eq!(
            render_object_mesh(&objects, first).unwrap().vertex_count(),
            3
        );
        assert!(update_render_object_transform(
            &mut objects,
            first,
            RenderTransform::from_translation(Vec3::X),
        ));
        assert_eq!(
            render_object_transform(&objects, first)
                .unwrap()
                .translation,
            Vec3::X
        );
        assert!(remove_render_object(&mut objects, first));
        assert!(render_object(&objects, first).is_none());
    }

    #[test]
    fn runtime_preserves_identity_and_cleans_owned_assets() {
        let mut app = App::new();
        app.add_plugins(RenderObjectPlugin);
        let id = {
            let mut objects = app.world_mut().resource_mut::<RenderObjects>();
            issue_render_object(&mut objects, test_object(3))
        };

        app.update();
        let (entity, mesh, material) = runtime_object(app.world(), id);
        {
            let mut objects = app.world_mut().resource_mut::<RenderObjects>();
            update_render_object_mesh(&mut objects, id, test_mesh(6));
        }
        app.update();
        let (updated_entity, updated_mesh, updated_material) = runtime_object(app.world(), id);

        assert_eq!(updated_entity, entity);
        assert_eq!(updated_mesh, mesh);
        assert_eq!(updated_material, material);
        let mesh_asset = app.world().resource::<Assets<Mesh>>().get(&mesh).unwrap();
        assert_eq!(mesh_asset.count_vertices(), 6);

        remove_render_object(&mut app.world_mut().resource_mut::<RenderObjects>(), id);
        app.update();
        assert!(app.world().get_entity(entity).is_err());
        let mesh_asset = app.world().resource::<Assets<Mesh>>().get(&mesh);
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        let material_asset = materials.get(&material);
        assert!(mesh_asset.is_none());
        assert!(material_asset.is_none());
    }

    fn test_object(vertex_count: usize) -> RenderObject {
        RenderObject::new(
            test_mesh(vertex_count),
            RenderMaterial::unlit([1.0; 4]),
            RenderTransform::default(),
        )
    }

    fn test_mesh(vertex_count: usize) -> RenderMesh {
        let triangle_count = vertex_count / 3;
        let positions = (0..vertex_count)
            .map(|index| [index as f32, 0.0, 0.0])
            .collect::<Vec<_>>();
        let normals = vec![[0.0, 1.0, 0.0]; vertex_count];
        let colors = vec![[1.0; 4]; vertex_count];
        let indices = (0..triangle_count * 3).map(|index| index as u32).collect();
        RenderMesh::triangle_list(positions, normals, colors, indices).unwrap()
    }

    fn runtime_object(
        world: &bevy::prelude::World,
        id: RenderObjectId,
    ) -> (Entity, Handle<Mesh>, Handle<StandardMaterial>) {
        let runtime = world.resource::<RenderObjectRuntime>();
        let object = &runtime.objects[&id];
        (object.entity, object.mesh.clone(), object.material.clone())
    }
}
