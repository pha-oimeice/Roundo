//! Stable-view subscriptions consume only changed Chunk IDs. Spatial membership
//! is still resolved through the same broad-phase index and live version checks.
use super::*;
use bevy::prelude::{Entity, Insert, On};

#[derive(Clone, PartialEq)]
pub(crate) struct CoordinateLayout {
    entity: Entity,
    id: LocalCoordinateId,
    scene: SceneId,
    transform: [u32; 12],
    installation: Option<crate::CoordinateIncarnation>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct SubscriptionView {
    observation: Option<ObservationRegion>,
    prediction: PredictionAnchorState,
    coordinates: Vec<CoordinateLayout>,
}
impl SubscriptionView {
    pub(super) fn new(
        observation: Option<ObservationRegion>,
        prediction: PredictionAnchorState,
        coordinates: &[CoordinateLayout],
    ) -> Self {
        Self {
            observation,
            prediction,
            coordinates: coordinates.to_vec(),
        }
    }
}

// An in-place component replacement can retain the same Entity and an unbound
// incarnation. Explicit insertion invalidation closes that cache seam.
pub(super) fn invalidate_installed_coordinate(
    _: On<Insert, LocalCoordinate>,
    world: Option<ResMut<LocalCoordinateServerWorld>>,
) {
    if let Some(mut world) = world {
        for subscription in world.subscriptions.values_mut() {
            subscription.last_view = None;
        }
    }
}

pub(super) fn coordinate_layouts(
    coordinates: &Query<(
        Entity,
        &PcgLocalCoordinate,
        &GlobalTransform,
        &LocalCoordinate,
    )>,
) -> Vec<CoordinateLayout> {
    let mut layouts = coordinates
        .iter()
        .map(
            |(entity, generated, transform, coordinate)| CoordinateLayout {
                entity,
                id: generated.id,
                scene: generated.scene_id,
                transform: transform.affine().to_cols_array().map(f32::to_bits),
                installation: coordinate.incarnation,
            },
        )
        .collect::<Vec<_>>();
    layouts.sort_unstable_by_key(|layout| layout.entity);
    layouts
}

pub(super) fn publish_incremental_changes(
    connection_id: ConnectionId,
    view: &SubscriptionView,
    changed: &HashSet<ChunkId>,
    index: &VirtualChunkIndex,
    coordinates: &Query<(&PcgLocalCoordinate, &LocalCoordinate)>,
    world: &mut LocalCoordinateServerWorld,
    pipe: &LocalCoordinateServerPipe,
) {
    let mut updates = Vec::new();
    let mut removed = Vec::new();
    for &id in changed {
        let current = view
            .coordinates
            .iter()
            .find(|layout| layout.id == id.local_coordinate_id)
            .and_then(|layout| {
                let position = crate::local_coordinate::pcg::local_chunk_position(id.coordinate)?;
                let reference = crate::ChunkReference::new(layout.entity, position);
                let rendering = view.observation.is_some_and(|observation| {
                    layout.scene == observation.scene_id
                        && index.reference_intersects_radius(
                            reference,
                            observation.streaming_center,
                            observation.radius,
                        )
                });
                let prediction = layout.scene == view.prediction.scene_id
                    && index.reference_intersects_radius(
                        reference,
                        view.prediction.position,
                        f64::from(view.prediction.radius_chunks) * CHUNK_EDGE_LENGTH as f64,
                    );
                if !rendering && !prediction {
                    return None;
                }
                streamed_chunk(
                    reference,
                    coordinates,
                    &world.loaded_chunks,
                    &world.chunk_versions,
                    layout.scene,
                )
                .map(|(_, version)| version)
            });
        let previous = world.subscriptions[&connection_id]
            .advertised_chunks
            .get(&id)
            .copied();
        match (previous, current) {
            (Some(_), None) => removed.push(id),
            (_, Some(version)) if previous != current => updates.push(ChunkVersion {
                local_coordinate_id: id.local_coordinate_id,
                coordinate: id.coordinate,
                version,
            }),
            _ => {}
        }
    }
    // The same deterministic nearest-first version ordering as a full rebuild.
    updates.sort_by(|left, right| {
        let distance = |chunk: &ChunkVersion| {
            view.observation.map_or(f64::INFINITY, |observation| {
                nearest_chunk_distance_squared(chunk.coordinate, &[observation])
            })
        };
        distance(left).total_cmp(&distance(right)).then_with(|| {
            (left.local_coordinate_id.0, left.coordinate)
                .cmp(&(right.local_coordinate_id.0, right.coordinate))
        })
    });
    removed.sort_unstable_by_key(|id| (id.local_coordinate_id.0, id.coordinate));
    let subscription = world.subscriptions.get_mut(&connection_id).unwrap();
    let removed_set = removed.iter().copied().collect::<HashSet<_>>();
    if !removed_set.is_empty() {
        subscription
            .pending_payloads
            .retain(|id| !removed_set.contains(id));
    }
    let mut changed_ids = updates.iter().map(|chunk| chunk.id()).collect::<Vec<_>>();
    changed_ids.sort_unstable_by_key(|id| (id.local_coordinate_id.0, id.coordinate));
    queue_subscription_updates(subscription, &changed_ids);
    for chunk in &updates {
        if subscription
            .spawned_coordinates
            .insert(chunk.local_coordinate_id)
        {
            if pipe
                .0
                .try_send(LocalCoordinateServerEvent::Spawned {
                    connection_id,
                    local_coordinate_id: chunk.local_coordinate_id,
                })
                .is_err()
            {
                log::error!(
                    "cannot publish incremental coordinate spawn: connection_id={}, reason=server_bridge_closed",
                    connection_id.0
                );
            }
        }
        subscription
            .advertised_chunks
            .insert(chunk.id(), chunk.version);
    }
    for id in &removed {
        subscription.advertised_chunks.remove(id);
    }
    if !updates.is_empty() {
        if pipe
            .0
            .try_send(LocalCoordinateServerEvent::ChunkVersions {
                connection_id,
                chunks: updates,
            })
            .is_err()
        {
            log::error!(
                "cannot publish incremental Chunk versions: connection_id={}, reason=server_bridge_closed",
                connection_id.0
            );
        }
    }
    for id in removed {
        if pipe
            .0
            .try_send(LocalCoordinateServerEvent::ChunkUnloaded {
                connection_id,
                local_coordinate_id: id.local_coordinate_id,
                coordinate: id.coordinate,
            })
            .is_err()
        {
            log::error!(
                "cannot publish incremental Chunk unload: connection_id={}, reason=server_bridge_closed",
                connection_id.0
            );
        }
    }
}

/// Coalesces deliveries by Chunk ID. Already-advertised edits precede initial
/// terrain, but still pass the ordinary in-flight and exact-version checks.
pub(super) fn queue_subscription_updates(
    subscription: &mut PlayerSubscription,
    changed: &[ChunkId],
) {
    if changed.is_empty() {
        return;
    }
    let priority = changed
        .iter()
        .copied()
        .filter(|id| subscription.advertised_chunks.contains_key(id))
        .collect::<Vec<_>>();
    let priority_set = priority.iter().copied().collect::<HashSet<_>>();
    subscription
        .pending_payloads
        .retain(|id| !priority_set.contains(id));
    let mut queued = subscription
        .pending_payloads
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    for &id in changed {
        if !priority_set.contains(&id) && queued.insert(id) {
            subscription.pending_payloads.push_back(id);
        }
    }
    for id in priority.into_iter().rev() {
        subscription.pending_payloads.push_front(id);
    }
}

pub(super) fn publish_environment_updates(
    connection_id: ConnectionId,
    prediction: PredictionAnchorState,
    environment: &crate::VirtualChunkEnvironmentMap,
    world: &mut LocalCoordinateServerWorld,
    pipe: &LocalCoordinateServerPipe,
) {
    let advertised = &world.subscriptions[&connection_id].advertised_environment_revisions;
    let updates = crate::local_coordinate::virtual_chunk::virtual_chunks_in_radius_for_streaming(
        prediction.position,
        f64::from(prediction.radius_chunks) * CHUNK_EDGE_LENGTH as f64,
    )
    .into_iter()
    .filter_map(|coordinate| {
        let (values, revision) = environment.override_at(coordinate);
        let mut fields = 0;
        if values.gravity.is_some() {
            fields |= 1;
        }
        if values.static_friction.is_some() {
            fields |= 2;
        }
        if values.kinetic_friction.is_some() {
            fields |= 4;
        }
        let previous = advertised.get(&coordinate);
        let changed_override = fields != 0 && previous != Some(&revision);
        let tombstone = fields == 0 && previous.is_some_and(|old| *old < revision);
        (changed_override || tombstone).then_some((
            coordinate,
            revision,
            VirtualChunkEnvironmentOverride {
                coordinate,
                revision: roundo_contracts::EnvironmentRevision(revision),
                fields,
                gravity: values.gravity.unwrap_or_default().to_array(),
                static_friction: values.static_friction.unwrap_or_default(),
                kinetic_friction: values.kinetic_friction.unwrap_or_default(),
            },
        ))
    })
    .collect::<Vec<_>>();
    if !updates.is_empty()
        && pipe
            .0
            .try_send(LocalCoordinateServerEvent::EnvironmentOverrides {
                connection_id,
                overrides: updates.iter().map(|(_, _, value)| *value).collect(),
            })
            .is_ok()
    {
        let subscription = world.subscriptions.get_mut(&connection_id).unwrap();
        for (coordinate, revision, _) in updates {
            subscription
                .advertised_environment_revisions
                .insert(coordinate, revision);
        }
    }
}
