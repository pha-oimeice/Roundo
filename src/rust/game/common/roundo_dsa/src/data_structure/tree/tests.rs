//! Bounded algorithm evidence: exhaustive fixture domains and payload-operation counts.
//! These checks do not prove correctness or asymptotic complexity for all inputs.

use super::{FixedDepthOctree, Octree};

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
        tree.source(),
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
                        tree.source(),
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

    let view =
        super::BreadthFirstLosslessSvo::from_unoptimized_mapped(tree.source(), 1, |data| *data)
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

#[test]
fn fixed_depth_reuses_heap_path_ids() {
    fn leaf_id(tree: &FixedDepthOctree<Option<u16>>, a: u8, b: u8) -> u32 {
        tree.source().root.children[usize::from(a)]
            .as_deref()
            .unwrap()
            .children[usize::from(b)]
        .as_deref()
        .unwrap()
        .id
    }

    let mut tree = FixedDepthOctree::new(2, None).unwrap();
    let mut original_ids = Vec::new();
    for a in 0..8_u8 {
        for b in 0..8_u8 {
            assert_eq!(tree.set(&[a, b], Some(7)).unwrap(), Some(None));
            let id = leaf_id(&tree, a, b);
            assert_eq!(id, (u32::from(a) + 1) * 8 + u32::from(b) + 1);
            assert!(!original_ids.contains(&id));
            original_ids.push(id);
        }
    }
    assert_eq!(original_ids.len(), 64);
    assert_eq!(tree.len(), 73);
    for a in 0..8_u8 {
        for b in 0..8_u8 {
            assert_eq!(tree.set(&[a, b], None).unwrap(), Some(Some(7)));
        }
    }
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.source().root.id, 0);
    let mut rebuilt_ids = Vec::new();
    for a in 0..8_u8 {
        for b in 0..8_u8 {
            assert_eq!(tree.set(&[a, b], Some(9)).unwrap(), Some(None));
            rebuilt_ids.push(leaf_id(&tree, a, b));
        }
    }
    assert_eq!(rebuilt_ids, original_ids);
    for a in 0..8_u8 {
        for b in 0..8_u8 {
            assert_eq!(tree.set(&[a, b], None).unwrap(), Some(Some(9)));
        }
    }
    assert_eq!(tree.len(), 1);
    for _ in 0..256 {
        assert_eq!(tree.set(&[3, 0], Some(11)).unwrap(), Some(None));
        assert_eq!(leaf_id(&tree, 3, 0), 33);
        assert_eq!(tree.set(&[3, 0], None).unwrap(), Some(Some(11)));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.source().root.id, 0);
    }
}

#[test]
fn fixed_depth_edit_touches_only_path_payloads() {
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Counts {
        clones: Cell<usize>,
        comparisons: Cell<usize>,
    }

    struct Payload {
        value: u8,
        counts: Rc<Counts>,
    }

    impl Clone for Payload {
        fn clone(&self) -> Self {
            self.counts.clones.set(self.counts.clones.get() + 1);
            Self {
                value: self.value,
                counts: Rc::clone(&self.counts),
            }
        }
    }

    impl PartialEq for Payload {
        fn eq(&self, other: &Self) -> bool {
            self.counts
                .comparisons
                .set(self.counts.comparisons.get() + 1);
            self.value == other.value
        }
    }

    let depth = 4_u8;
    let target = [0, 0, 0, 0];
    for many_unrelated_paths in [false, true] {
        let counts = Rc::new(Counts::default());
        let payload = |value| Payload {
            value,
            counts: Rc::clone(&counts),
        };
        let mut tree = FixedDepthOctree::new(depth, payload(0)).unwrap();
        assert_eq!(
            tree.set(&target, payload(1)).unwrap().map(|old| old.value),
            Some(0)
        );
        if many_unrelated_paths {
            for a in 1..8 {
                for b in 0..8 {
                    for c in 0..8 {
                        for d in 0..8 {
                            assert_eq!(
                                tree.set(&[a, b, c, d], payload(3))
                                    .unwrap()
                                    .map(|old| old.value),
                                Some(0)
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(tree.len(), if many_unrelated_paths { 4100 } else { 5 });

        counts.clones.set(0);
        counts.comparisons.set(0);
        let previous = tree.set(&target, payload(2)).unwrap().map(|old| old.value);
        assert_eq!(previous, Some(1));
        assert!(counts.comparisons.get() <= 4 * usize::from(depth) + 4);
        assert_eq!(
            counts.clones.get(),
            0,
            "existing-path replacement clones no payloads"
        );

        counts.clones.set(0);
        counts.comparisons.set(0);
        let previous = tree.set(&target, payload(0)).unwrap().map(|old| old.value);
        assert_eq!(previous, Some(2));
        assert!(counts.comparisons.get() <= 4 * usize::from(depth) + 4);
        assert_eq!(
            counts.clones.get(),
            0,
            "existing-path clearing clones no payloads"
        );
        assert!(tree.source().root.children[0].is_none());
        assert_eq!(tree.len(), if many_unrelated_paths { 4096 } else { 1 });
        if many_unrelated_paths {
            let mut unrelated = &tree.source().root;
            for octant in [7, 7, 7, 7] {
                unrelated = unrelated.children[octant].as_deref().unwrap();
            }
            assert_eq!(unrelated.data.value, 3);
        }
    }
    // Payload counters do not detect pure-ID scans; control-flow review remains necessary.
}
