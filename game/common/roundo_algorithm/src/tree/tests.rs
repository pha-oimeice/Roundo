use super::{
    CompressionSettings, CompressionThresholdSchedule, DensityAggregation, NO_CHILDREN, Node,
    Octree, OctreeError,
};

#[test]
fn gpu_packed_svo_nodes_have_a_stable_sixteen_byte_layout() {
    let mut source = super::UnoptimizedOctree::new(0, 0_u32);
    source.root.children[3] = Some(Box::new(Node::new(4, 7)));
    let svo =
        super::BreadthFirstLosslessSvo::from_unoptimized_mapped(&source, 1, |data| *data).unwrap();
    let packed = svo.pack_with(|data| *data);

    assert_eq!(std::mem::size_of::<super::PackedSvoNode>(), 16);
    assert_eq!(packed.nodes()[0].child_mask, 1 << 3);
    assert_eq!(packed.nodes()[0].reserved, 1);
    assert_eq!(packed.nodes()[1].data, 7);
    assert_eq!(packed.nodes()[1].reserved, 1);
    assert_eq!(packed.node_bytes().len(), packed.nodes().len() * 16);
}

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

#[test]
fn breadth_first_lossless_svo_preserves_coordinate_values() {
    let mut tree = Octree::new(0, None);
    tree.insert(0, 3, 1, Some(7_u16)).unwrap();
    tree.insert(1, 6, 2, Some(11_u16)).unwrap();

    let view = super::BreadthFirstLosslessSvo::from_unoptimized_mapped(
        &tree.unoptimized_octree,
        2,
        |data| data.unwrap_or_default(),
    )
    .unwrap();

    assert_eq!(view.nodes().len(), 3);
    assert_eq!(view.node_at_path(&[3]).map(|node| node.data), Some(7));
    assert_eq!(view.node_at_path(&[3, 6]).map(|node| node.data), Some(11));
    assert!(view.node_at_path(&[0]).is_none());
    assert_eq!(view.value_at_path(&[0]), Some(&0));
    assert_eq!(view.value_at_path(&[3, 0]), Some(&7));
    assert_eq!(view.value_at_path(&[3, 6]), Some(&11));
}

#[test]
fn breadth_first_lossless_svo_matches_every_source_coordinate() {
    let mut tree = Octree::new(0, 0_u16);
    tree.insert(0, 0, 1, 4).unwrap();
    tree.insert(1, 7, 2, 9).unwrap();
    tree.insert(0, 5, 3, 0).unwrap();
    tree.insert(3, 2, 4, 6).unwrap();
    tree.insert(4, 1, 5, 3).unwrap();
    let maximum_depth = 3;

    let view = super::BreadthFirstLosslessSvo::from_unoptimized_mapped(
        &tree.unoptimized_octree,
        maximum_depth,
        |data| *data,
    )
    .unwrap();

    for z in 0..8 {
        for y in 0..8 {
            for x in 0..8 {
                let coordinates = [x, y, z];
                assert_eq!(
                    view.value_at_coordinates(coordinates),
                    Some(&source_value_at_coordinates(
                        &tree.unoptimized_octree,
                        maximum_depth,
                        coordinates,
                    )),
                    "coordinate {coordinates:?} changed during compression"
                );
            }
        }
    }
}

#[test]
fn breadth_first_lossless_svo_collapses_uniform_regions() {
    let mut tree = Octree::new(0, 0_u16);
    for octant in 0..8 {
        tree.insert(0, octant, u32::from(octant) + 1, 7).unwrap();
    }

    let view = super::BreadthFirstLosslessSvo::from_unoptimized_mapped(
        &tree.unoptimized_octree,
        1,
        |data| *data,
    )
    .unwrap();

    assert_eq!(view.nodes().len(), 1);
    assert_eq!(view.maximum_depth(), 1);
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(view.value_at_coordinates([x, y, z]), Some(&7));
            }
        }
    }
}

#[test]
fn breadth_first_lossless_svo_rejects_coordinates_outside_its_depth() {
    let tree = Octree::new(0, 5_u16);
    let view = super::BreadthFirstLosslessSvo::from_unoptimized_mapped(
        &tree.unoptimized_octree,
        2,
        |data| *data,
    )
    .unwrap();

    assert_eq!(view.value_at_coordinates([3, 3, 3]), Some(&5));
    assert_eq!(view.value_at_coordinates([4, 0, 0]), None);
    assert_eq!(view.value_at_path(&[0, 0, 0]), None);
}

fn source_value_at_coordinates(
    source: &super::UnoptimizedOctree<u16>,
    maximum_depth: u8,
    coordinates: [u32; 3],
) -> u16 {
    let mut node = &source.root;
    let mut value = node.data;
    for depth in 0..maximum_depth {
        let bit = maximum_depth - depth - 1;
        let octant = (((coordinates[0] >> bit) & 1)
            | (((coordinates[1] >> bit) & 1) << 1)
            | (((coordinates[2] >> bit) & 1) << 2)) as usize;
        let Some(child) = node.children[octant].as_deref() else {
            break;
        };
        node = child;
        value = node.data;
    }
    value
}
