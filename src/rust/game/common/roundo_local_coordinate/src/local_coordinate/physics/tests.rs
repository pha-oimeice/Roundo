use super::*;

// A count bound on queue admission, not a bound on collider construction time.
#[test]
fn collider_derivation_has_a_hard_per_update_budget() {
    let mut state = LocalCoordinatePhysicsState::default();
    for x in 0..(MAX_COLLIDER_CHUNKS_PER_UPDATE as i32 + 10) {
        enqueue_collider_chunk(&mut state, IVec3::new(x, 0, 0));
    }
    let admitted = take_pending_collider_chunks(&mut state);
    assert_eq!(admitted.len(), MAX_COLLIDER_CHUNKS_PER_UPDATE);
    assert_eq!(state.pending_chunks.len(), 10);
}
