use super::{
    CompressionSettings, CompressionThresholdSchedule, DensityAggregation, NO_CHILDREN, Node,
    Octree, OctreeError,
};

fn node_ids<T>(nodes: Vec<&Node<T, 8>>) -> Vec<u32> {
    nodes.into_iter().map(|node| node.id).collect()
}

#[test]
fn supports_crud_operations() {
    let mut tree = Octree::new(0, "root");

    tree.insert(0, 0, 1, "child").unwrap();
    tree.insert(1, 7, 2, "grandchild").unwrap();

    assert_eq!(tree.len(), 3);
    assert_eq!(tree.get(2), Some(&"grandchild"));
    assert_eq!(tree.update(2, "updated").unwrap(), "grandchild");
    assert_eq!(tree.get(2), Some(&"updated"));

    assert_eq!(tree.remove(1).unwrap(), "child");
    assert_eq!(tree.len(), 1);
    assert!(!tree.contains(1));
    assert!(!tree.contains(2));
}

#[test]
fn reports_invalid_mutations() {
    let mut tree = Octree::new(0, ());

    assert_eq!(
        tree.insert(0, 8, 1, ()),
        Err(OctreeError::ChildIndexOutOfBounds { index: 8 })
    );

    tree.insert(0, 0, 1, ()).unwrap();

    assert_eq!(
        tree.insert(0, 1, 1, ()),
        Err(OctreeError::DuplicateNodeId { id: 1 })
    );
    assert_eq!(
        tree.insert(0, 0, 2, ()),
        Err(OctreeError::ChildSlotOccupied {
            parent_id: 0,
            child_index: 0,
        })
    );
    assert_eq!(
        tree.update(99, ()),
        Err(OctreeError::NodeNotFound { id: 99 })
    );
    assert_eq!(tree.remove(0), Err(OctreeError::CannotRemoveRoot));
}

#[test]
fn traverses_nodes_in_prefix_infix_and_postfix_order() {
    let mut tree = Octree::new(0, ());

    tree.insert(0, 0, 1, ()).unwrap();
    tree.insert(0, 3, 3, ()).unwrap();
    tree.insert(0, 4, 4, ()).unwrap();
    tree.insert(0, 7, 7, ()).unwrap();
    tree.insert(1, 0, 10, ()).unwrap();
    tree.insert(1, 6, 16, ()).unwrap();

    assert_eq!(
        node_ids(tree.prefix_traversal()),
        vec![0, 1, 10, 16, 3, 4, 7]
    );
    assert_eq!(
        node_ids(tree.infix_traversal()),
        vec![10, 1, 16, 3, 0, 4, 7]
    );
    assert_eq!(
        node_ids(tree.postfix_traversal()),
        vec![10, 16, 1, 3, 4, 7, 0]
    );
}

#[test]
fn builds_a_lossless_breadth_first_svo_cache() {
    let mut tree = Octree::new(0, ());

    tree.insert(0, 0, 1, ()).unwrap();
    tree.insert(0, 3, 3, ()).unwrap();
    tree.insert(0, 4, 4, ()).unwrap();
    tree.insert(0, 7, 7, ()).unwrap();
    tree.insert(1, 0, 10, ()).unwrap();
    tree.insert(1, 6, 16, ()).unwrap();
    tree.rebuild_optimized_cache().unwrap();

    let cache = tree.optimized_octree.as_ref().unwrap();
    let cached_ids = cache.nodes.iter().map(|node| node.id).collect::<Vec<_>>();

    assert_eq!(cached_ids, vec![0, 1, 3, 4, 7, 10, 16]);
    assert_eq!(&cache.level_offsets[..], &[0, 1, 5, 7]);
    assert_eq!(cache.nodes[0].child_mask, 0b1001_1001);
    assert_eq!(cache.nodes[0].first_child, 1);
    assert_eq!(cache.nodes[1].child_mask, 0b0100_0001);
    assert_eq!(cache.nodes[1].first_child, 5);
    assert_eq!(cache.nodes[2].first_child, NO_CHILDREN);
    assert_eq!(cache.child_index(0, 3), Some(2));
    assert_eq!(cache.child_index(1, 6), Some(6));
    assert_eq!(cache.node_at_path(&[0, 6]).map(|node| node.id), Some(16));

    tree.update(16, ()).unwrap();
    assert!(tree.optimized_octree.is_none());
}

#[test]
fn full_solid_compression_omits_fully_occupied_descendants() {
    let mut tree = Octree::new(0, ());

    for octant in 0..8 {
        tree.insert(0, octant, u32::from(octant) + 1, ()).unwrap();
    }
    tree.rebuild_optimized_cache_with_settings(CompressionSettings::full_solid())
        .unwrap();

    let cache = tree.optimized_octree.as_ref().unwrap();
    let root = &cache.nodes[cache.root_index as usize];

    assert_eq!(cache.nodes.len(), 1);
    assert_eq!(root.density, 8.0);
    assert!(root.is_compressed_solid);
    assert_eq!(root.child_mask, 0);
    assert_eq!(root.first_child, NO_CHILDREN);
}

#[test]
fn average_density_can_use_a_constant_threshold() {
    let mut tree = Octree::new(0, ());

    for octant in 0..8 {
        tree.insert(0, octant, u32::from(octant) + 1, ()).unwrap();
    }
    tree.rebuild_optimized_cache_with_settings(CompressionSettings {
        base_solid_compression_threshold: 1.0,
        density_aggregation: DensityAggregation::Average,
        threshold_schedule: CompressionThresholdSchedule::Constant,
    })
    .unwrap();

    let root = &tree.optimized_octree.as_ref().unwrap().nodes[0];

    assert_eq!(root.density, 1.0);
    assert!(root.is_compressed_solid);
}
