use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Meshable},
    prelude::{
        AlphaMode, App, Assets, Color, Commands, Component, Cuboid, Entity, GlobalTransform,
        Handle, IntoScheduleConfigs, Mesh, Mesh3d, MeshMaterial3d, Name, Plugin, PostUpdate, Quat,
        Res, ResMut, Resource, Sphere, StandardMaterial, SystemSet, Tetrahedron, Transform, Vec3,
        Visibility,
    },
    render::render_resource::PrimitiveTopology,
    transform::TransformSystems,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RenderObjectId(u64);

impl RenderObjectId {
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl RenderTransform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub const fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub const fn from_parts(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    pub fn compose(self, child: Self) -> Self {
        Self::from(Transform::from(self).mul_transform(Transform::from(child)))
    }
}

impl Default for RenderTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<Transform> for RenderTransform {
    fn from(value: Transform) -> Self {
        Self::from_parts(value.translation, value.rotation, value.scale)
    }
}

impl From<RenderTransform> for Transform {
    fn from(value: RenderTransform) -> Self {
        Self {
            translation: value.translation,
            rotation: value.rotation,
            scale: value.scale,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderMesh(Mesh);

impl RenderMesh {
    pub fn triangle_list(
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        colors: Vec<[f32; 4]>,
        indices: Vec<u32>,
    ) -> Result<Self, RenderMeshError> {
        if positions.len() != normals.len() || positions.len() != colors.len() {
            return Err(RenderMeshError::AttributeLengthMismatch);
        }
        if !indices.len().is_multiple_of(3) {
            return Err(RenderMeshError::IncompleteTriangle);
        }
        if indices
            .iter()
            .any(|index| *index as usize >= positions.len())
        {
            return Err(RenderMeshError::IndexOutOfBounds);
        }

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));
        Ok(Self(mesh))
    }

    pub fn tetrahedron() -> Self {
        Self(Mesh::from(Tetrahedron::default()))
    }

    pub fn uv_sphere(radius: f32, sectors: u32, stacks: u32) -> Self {
        Self(Sphere::new(radius).mesh().uv(sectors, stacks))
    }

    pub fn cuboid(size: Vec3) -> Self {
        Self(Mesh::from(Cuboid::new(size.x, size.y, size.z)))
    }

    pub fn vertex_count(&self) -> usize {
        self.0.count_vertices()
    }

    fn as_bevy(&self) -> &Mesh {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderMeshError {
    AttributeLengthMismatch,
    IncompleteTriangle,
    IndexOutOfBounds,
}

#[derive(Clone, Debug)]
pub struct RenderMaterial(StandardMaterial);

impl RenderMaterial {
    pub fn unlit(color: [f32; 4]) -> Self {
        Self(StandardMaterial {
            base_color: Color::srgba(color[0], color[1], color[2], color[3]),
            unlit: true,
            ..Default::default()
        })
    }

    pub fn with_perceptual_roughness(mut self, roughness: f32) -> Self {
        self.0.perceptual_roughness = roughness;
        self
    }

    pub fn with_metallic(mut self, metallic: f32) -> Self {
        self.0.metallic = metallic;
        self
    }

    pub fn with_alpha_blend(mut self) -> Self {
        self.0.alpha_mode = AlphaMode::Blend;
        self
    }

    fn as_bevy(&self) -> &StandardMaterial {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct RenderObject {
    name: String,
    transform: RenderTransform,
    material: RenderMaterial,
    mesh: RenderMesh,
    visible: bool,
}

impl RenderObject {
    pub fn new(mesh: RenderMesh, material: RenderMaterial, transform: RenderTransform) -> Self {
        Self {
            name: "Render Object".to_string(),
            transform,
            material,
            mesh,
            visible: true,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_visibility(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn transform(&self) -> RenderTransform {
        self.transform
    }

    pub const fn material(&self) -> &RenderMaterial {
        &self.material
    }

    pub const fn mesh(&self) -> &RenderMesh {
        &self.mesh
    }

    pub const fn is_visible(&self) -> bool {
        self.visible
    }
}

struct RenderObjectEntry {
    object: RenderObject,
    name_revision: u64,
    transform_revision: u64,
    material_revision: u64,
    mesh_revision: u64,
    visibility_revision: u64,
}

impl RenderObjectEntry {
    fn new(object: RenderObject) -> Self {
        Self {
            object,
            name_revision: 1,
            transform_revision: 1,
            material_revision: 1,
            mesh_revision: 1,
            visibility_revision: 1,
        }
    }
}

#[derive(Resource)]
pub struct RenderObjects {
    next_id: u64,
    objects: HashMap<RenderObjectId, RenderObjectEntry>,
}

impl Default for RenderObjects {
    fn default() -> Self {
        Self {
            next_id: 1,
            objects: HashMap::new(),
        }
    }
}

pub fn issue_render_object(
    render_objects: &mut RenderObjects,
    object: RenderObject,
) -> RenderObjectId {
    let id = RenderObjectId(render_objects.next_id);
    render_objects.next_id = render_objects
        .next_id
        .checked_add(1)
        .expect("render object id space exhausted");
    render_objects
        .objects
        .insert(id, RenderObjectEntry::new(object));
    id
}

pub fn remove_render_object(render_objects: &mut RenderObjects, id: RenderObjectId) -> bool {
    render_objects.objects.remove(&id).is_some()
}

pub fn render_object(render_objects: &RenderObjects, id: RenderObjectId) -> Option<&RenderObject> {
    render_objects.objects.get(&id).map(|entry| &entry.object)
}

pub fn render_object_transform(
    render_objects: &RenderObjects,
    id: RenderObjectId,
) -> Option<RenderTransform> {
    render_object(render_objects, id).map(RenderObject::transform)
}

pub fn render_object_material(
    render_objects: &RenderObjects,
    id: RenderObjectId,
) -> Option<&RenderMaterial> {
    render_object(render_objects, id).map(RenderObject::material)
}

pub fn render_object_mesh(
    render_objects: &RenderObjects,
    id: RenderObjectId,
) -> Option<&RenderMesh> {
    render_object(render_objects, id).map(RenderObject::mesh)
}

pub fn update_render_object_name(
    render_objects: &mut RenderObjects,
    id: RenderObjectId,
    name: impl Into<String>,
) -> bool {
    let Some(entry) = render_objects.objects.get_mut(&id) else {
        return false;
    };
    entry.object.name = name.into();
    entry.name_revision = entry.name_revision.wrapping_add(1);
    true
}

pub fn update_render_object_transform(
    render_objects: &mut RenderObjects,
    id: RenderObjectId,
    transform: RenderTransform,
) -> bool {
    let Some(entry) = render_objects.objects.get_mut(&id) else {
        return false;
    };
    if entry.object.transform == transform {
        return true;
    }
    entry.object.transform = transform;
    entry.transform_revision = entry.transform_revision.wrapping_add(1);
    true
}

pub fn update_render_object_material(
    render_objects: &mut RenderObjects,
    id: RenderObjectId,
    material: RenderMaterial,
) -> bool {
    let Some(entry) = render_objects.objects.get_mut(&id) else {
        return false;
    };
    entry.object.material = material;
    entry.material_revision = entry.material_revision.wrapping_add(1);
    true
}

pub fn update_render_object_mesh(
    render_objects: &mut RenderObjects,
    id: RenderObjectId,
    mesh: RenderMesh,
) -> bool {
    let Some(entry) = render_objects.objects.get_mut(&id) else {
        return false;
    };
    entry.object.mesh = mesh;
    entry.mesh_revision = entry.mesh_revision.wrapping_add(1);
    true
}

pub fn update_render_object_visibility(
    render_objects: &mut RenderObjects,
    id: RenderObjectId,
    visible: bool,
) -> bool {
    let Some(entry) = render_objects.objects.get_mut(&id) else {
        return false;
    };
    if entry.object.visible == visible {
        return true;
    }
    entry.object.visible = visible;
    entry.visibility_revision = entry.visibility_revision.wrapping_add(1);
    true
}

pub struct RenderObjectPlugin;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct RenderObjectSync;

mod runtime;
