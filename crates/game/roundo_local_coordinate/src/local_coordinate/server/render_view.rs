use super::super::pcg::local_chunk_position;
use crate::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, CoordinateIncarnation, LocalCoordinate, LocalCoordinateId,
};
use bevy::prelude::{Component, Vec2};
use roundo_infrastructure::user_config::{MAX_CHUNK_VIEW_DISTANCE, MIN_CHUNK_VIEW_DISTANCE};

/// Opts a procedural coordinate into view-driven generation and rendering.
/// The default is an inactive view: physics can retain chunks, but none are
/// drawn. A coordinate without this component keeps its ordinary presentation.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct PcgRenderView(Option<HorizontalView>);

#[derive(Clone, Copy, Debug, PartialEq)]
struct HorizontalView {
    installation: CoordinateIncarnation,
    center: [f64; 2],
    layer: i64,
    radius_squared: f64,
    minimum: [i64; 2],
    maximum: [i64; 2],
}

impl PcgRenderView {
    /// Selects whole chunks that intersect a horizontal disk in the specified
    /// chunk layer. Center and radius are in coordinate-local voxel units.
    /// Rejects a radius outside the client setting's range or an invalid origin.
    pub fn horizontal(
        installation: CoordinateIncarnation,
        center: Vec2,
        layer: i32,
        radius: f32,
    ) -> Result<Self, &'static str> {
        if !(MIN_CHUNK_VIEW_DISTANCE..=MAX_CHUNK_VIEW_DISTANCE).contains(&radius) {
            return Err("render radius is outside the client setting's range");
        }
        if !center.is_finite() {
            return Err("render view origin must be finite");
        }
        let center = center.to_array().map(f64::from);
        let radius = f64::from(radius);
        let layer = i64::from(layer);
        if local_chunk_position([chunk_axis(center[0]), layer, chunk_axis(center[1])]).is_none() {
            return Err("render view origin is outside local chunk storage");
        }
        Ok(Self(Some(HorizontalView {
            installation,
            center,
            layer,
            radius_squared: radius * radius,
            minimum: center.map(|value| chunk_axis(value - radius)),
            maximum: center.map(|value| chunk_axis(value + radius)),
        })))
    }

    pub(crate) fn for_installation(
        self,
        coordinate: &LocalCoordinate,
        identity: LocalCoordinateId,
    ) -> Self {
        if self.0.is_some_and(|view| {
            coordinate.incarnation == Some(view.installation)
                && identity == view.installation.identity()
        }) {
            self
        } else {
            Self::default()
        }
    }

    pub(crate) fn contains(self, position: ChunkCoordinate) -> bool {
        self.0.is_some_and(|view| view.contains(position))
    }

    pub(super) fn positions(self) -> impl Iterator<Item = ChunkCoordinate> {
        self.0.into_iter().flat_map(|view| {
            (view.minimum[1]..=view.maximum[1]).flat_map(move |z| {
                (view.minimum[0]..=view.maximum[0])
                    .map(move |x| [x, view.layer, z])
                    .filter(move |&position| {
                        view.contains(position) && local_chunk_position(position).is_some()
                    })
            })
        })
    }

    pub(super) fn distance_squared(self, position: ChunkCoordinate) -> f64 {
        self.0.map_or(0.0, |view| view.distance_squared(position))
    }
}

impl HorizontalView {
    fn contains(self, [x, y, z]: ChunkCoordinate) -> bool {
        y == self.layer
            && (self.minimum[0]..=self.maximum[0]).contains(&x)
            && (self.minimum[1]..=self.maximum[1]).contains(&z)
            && self.distance_squared([x, y, z]) <= self.radius_squared
    }

    fn distance_squared(self, [x, _, z]: ChunkCoordinate) -> f64 {
        let edge = CHUNK_EDGE_LENGTH as f64;
        self.center
            .into_iter()
            .zip([x, z])
            .map(|(center, axis)| {
                let minimum = axis as f64 * edge;
                let delta = center - center.clamp(minimum, minimum + edge);
                delta * delta
            })
            .sum()
    }
}

fn chunk_axis(value: f64) -> i64 {
    (value / CHUNK_EDGE_LENGTH as f64).floor() as i64
}
