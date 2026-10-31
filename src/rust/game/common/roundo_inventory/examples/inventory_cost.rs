//! Public-interface cost fixture, not a capacity promise or item ID issuer.
//! The example executable measures operations and validates the resulting report.
//! Allocation counts and memory use are not measured. No latency thresholds.

use bevy::{ecs::system::SystemState, prelude::App};
use roundo_inventory::{
    Inventories, InventoryError, InventoryHandle, InventoryOwner, InventoryPlugin, ItemId,
};
use roundo_life::{
    ConceptBodyFactory, ConceptBodyHandle, ConceptBodyPrototype, LifeRuntimePlugin,
    close_life_data_catalogue, with_life_administration,
};
use std::{collections::HashSet, hint::black_box, time::Instant};

#[derive(Clone, Copy)]
struct Dataset {
    name: &'static str,
    // Distinguishes all five declared fixture groups, not a production issuer.
    imported_group: u128,
    unrelated: usize,
    owner_count: usize,
    inventory_count: usize,
    total_items: usize,
    target_items: usize,
}

const DATASETS: [Dataset; 5] = [
    Dataset {
        name: "multi",
        imported_group: 1,
        unrelated: 0,
        owner_count: 32,
        inventory_count: 64,
        total_items: 16_384,
        target_items: 512,
    },
    Dataset {
        name: "large",
        imported_group: 2,
        unrelated: 0,
        owner_count: 1,
        inventory_count: 2,
        total_items: 100_000,
        target_items: 100_000,
    },
    Dataset {
        name: "unrelated",
        imported_group: 3,
        unrelated: 0,
        owner_count: 2,
        inventory_count: 2,
        total_items: 512,
        target_items: 512,
    },
    Dataset {
        name: "unrelated",
        imported_group: 4,
        unrelated: 1_000,
        owner_count: 2,
        inventory_count: 1_002,
        total_items: 1_512,
        target_items: 512,
    },
    Dataset {
        name: "unrelated",
        imported_group: 5,
        unrelated: 10_000,
        owner_count: 2,
        inventory_count: 10_002,
        total_items: 10_512,
        target_items: 512,
    },
];

fn access<R>(app: &mut App, operation: impl FnOnce(&mut Inventories<'_, '_>) -> R) -> R {
    // SystemParam setup and application are outside every measured interval.
    let mut state = SystemState::<Inventories>::new(app.world_mut());
    let result = {
        let mut inventories = state
            .get_mut(app.world_mut())
            .expect("inventory cost fixture requires valid Inventories");
        operation(&mut inventories)
    };
    state.apply(app.world_mut());
    result
}

struct Fixture {
    app: App,
    bodies: Vec<ConceptBodyHandle>,
    owners: Vec<InventoryOwner>,
    handles: Vec<InventoryHandle>,
    owner_indices: Vec<usize>,
    initial: Vec<Vec<ItemId>>,
    // Every ID is visited once from its original source, never re-transferred.
    transfers: Vec<(usize, usize, ItemId)>,
}

fn prepare(dataset: Dataset, sample: usize) -> Fixture {
    let mut app = App::new();
    app.add_plugins((LifeRuntimePlugin::default(), InventoryPlugin));
    close_life_data_catalogue(&mut app).unwrap();
    let bodies = with_life_administration(app.world_mut(), |world, admin| {
        (0..dataset.owner_count)
            .map(|_| {
                ConceptBodyFactory::create(world, admin, ConceptBodyPrototype::new(())).unwrap()
            })
            .collect::<Vec<_>>()
    });
    let owner_indices = (0..dataset.inventory_count)
        .map(|index| match dataset.name {
            "multi" => index / 2,
            "large" => 0,
            "unrelated" => usize::from(index >= 2),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let (owners, handles) = access(&mut app, |i| {
        let owners = bodies
            .iter()
            .map(|&body| i.bind_owner(body).unwrap())
            .collect::<Vec<_>>();
        let handles = owner_indices
            .iter()
            .map(|&owner| i.create(owners[owner]).unwrap())
            .collect();
        (owners, handles)
    });
    let initial = (0..dataset.inventory_count)
        .map(|inventory| {
            let count = match dataset.name {
                "multi" => 256,
                "large" => {
                    if inventory == 0 {
                        100_000
                    } else {
                        0
                    }
                }
                "unrelated" => {
                    if inventory < 2 {
                        256
                    } else {
                        1
                    }
                }
                _ => unreachable!(),
            };
            (0..count)
                .map(|sequence| {
                    // Declared external test identity inputs: disjoint group,
                    // sample, inventory and sequence fields. No reuse across
                    // these fresh Apps, global issuance or cross-domain custody.
                    ItemId::from_u128(
                        (dataset.imported_group << 96)
                            | ((sample as u128) << 80)
                            | ((inventory as u128) << 32)
                            | (sequence as u128 + 1),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let transfers = initial
        .iter()
        .enumerate()
        .filter(|(index, _)| dataset.name != "unrelated" || *index < 2)
        .flat_map(|(source, ids)| ids.iter().map(move |&id| (source, source ^ 1, id)))
        .collect::<Vec<_>>();
    assert_eq!(
        initial.iter().map(Vec::len).sum::<usize>(),
        dataset.total_items
    );
    assert_eq!(
        initial[..2].iter().map(Vec::len).sum::<usize>(),
        dataset.target_items
    );
    let all = initial.iter().flatten().copied().collect::<HashSet<_>>();
    assert_eq!(all.len(), dataset.total_items);
    Fixture {
        app,
        bodies,
        owners,
        handles,
        owner_indices,
        initial,
        transfers,
    }
}

fn checksum(ids: impl IntoIterator<Item = ItemId>) -> u128 {
    ids.into_iter().map(ItemId::as_u128).sum()
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    items: Vec<Result<HashSet<ItemId>, InventoryError>>,
    holders: Vec<Result<Option<InventoryHandle>, InventoryError>>,
    buckets: Vec<Result<HashSet<InventoryHandle>, InventoryError>>,
    len: Result<usize, InventoryError>,
}

fn snapshot(fixture: &mut Fixture) -> Snapshot {
    access(&mut fixture.app, |i| Snapshot {
        items: fixture
            .handles
            .iter()
            .map(|&h| i.items(h).map(|v| v.into_iter().collect()))
            .collect(),
        holders: fixture
            .initial
            .iter()
            .flatten()
            .map(|&id| i.holder(id))
            .collect(),
        buckets: fixture
            .owners
            .iter()
            .map(|&o| i.for_body(o).map(|v| v.into_iter().collect()))
            .collect(),
        len: i.len(),
    })
}

fn assert_state(fixture: &mut Fixture, expected: &[HashSet<ItemId>]) -> Snapshot {
    let actual = snapshot(fixture);
    assert_eq!(actual.len, Ok(fixture.handles.len() + 1)); // includes Public
    assert_eq!(
        actual.items,
        expected.iter().cloned().map(Ok).collect::<Vec<_>>()
    );
    let mut expected_holders = std::collections::HashMap::new();
    for (index, ids) in expected.iter().enumerate() {
        for &id in ids {
            assert_eq!(expected_holders.insert(id, fixture.handles[index]), None);
        }
    }
    assert_eq!(
        actual.holders,
        fixture
            .initial
            .iter()
            .flatten()
            .map(|id| Ok(Some(expected_holders[id])))
            .collect::<Vec<_>>()
    );
    for owner in 0..fixture.owners.len() {
        let bucket = fixture
            .owner_indices
            .iter()
            .enumerate()
            .filter(|(_, o)| **o == owner)
            .map(|(h, _)| fixture.handles[h])
            .collect::<HashSet<_>>();
        assert_eq!(actual.buckets[owner], Ok(bucket));
    }
    let observed_checksum = checksum(
        actual
            .items
            .iter()
            .flat_map(|items| items.as_ref().unwrap().iter().copied()),
    );
    assert_eq!(
        observed_checksum,
        checksum(fixture.initial.iter().flatten().copied())
    );
    actual
}

struct Measurement {
    dataset: Dataset,
    sample: usize,
    operation: &'static str,
    operation_count: usize,
    elapsed_ns: u128,
    checksum: u128,
    measurement_scope: &'static str,
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output.push('"');
    output
}

impl Measurement {
    fn print(&self) {
        let d = self.dataset;
        println!(
            "{{\"schema_version\":1,\"dataset\":{},\"sample\":{},\"U\":{},\"owner_count\":{},\"inventory_count\":{},\"total_items\":{},\"target_inventories\":2,\"target_items\":{},\"operation\":{},\"operation_count\":{},\"elapsed_ns\":{},\"checksum\":{},\"profile\":\"dev\",\"measurement_scope\":{},\"allocations_measured\":false}}",
            json_string(d.name),
            self.sample,
            d.unrelated,
            d.owner_count,
            d.inventory_count,
            d.total_items,
            d.target_items,
            json_string(self.operation),
            self.operation_count,
            self.elapsed_ns,
            self.checksum,
            json_string(self.measurement_scope),
        );
    }
}

fn measure_sample(dataset: Dataset, sample: usize) -> Vec<Measurement> {
    let mut f = prepare(dataset, sample);
    let total_checksum = checksum(f.initial.iter().flatten().copied());
    let target_checksum = checksum(f.initial[..2].iter().flatten().copied());
    let transfer_checksum = checksum(f.transfers.iter().map(|&(_, _, id)| id));
    let mut expected = f
        .initial
        .iter()
        .map(|ids| ids.iter().copied().collect::<HashSet<_>>())
        .collect::<Vec<_>>();
    // Prove empty preparation without charging the oracle to insertion.
    access(&mut f.app, |i| {
        assert_eq!(i.len(), Ok(dataset.inventory_count + 1));
        for &h in &f.handles {
            assert!(i.items(h).unwrap().is_empty());
        }
        for &id in f.initial.iter().flatten() {
            assert_eq!(i.holder(id), Ok(None));
        }
    });
    let insert_ns = access(&mut f.app, |i| {
        let start = Instant::now();
        for (index, ids) in f.initial.iter().enumerate() {
            for &id in ids {
                black_box(i.insert(f.handles[index], id).unwrap());
            }
        }
        start.elapsed().as_nanos()
    });
    let inserted = assert_state(&mut f, &expected);
    let mut found = Vec::with_capacity(dataset.total_items);
    let holder_ns = access(&mut f.app, |i| {
        let start = Instant::now();
        for &id in f.initial.iter().flatten() {
            found.push(black_box(i.holder(id).unwrap()));
        }
        start.elapsed().as_nanos()
    });
    assert_eq!(found.len(), dataset.total_items);
    assert_eq!(
        found.into_iter().map(Ok).collect::<Vec<_>>(),
        inserted.holders
    );
    assert_eq!(assert_state(&mut f, &expected), inserted);
    let transfer_ns = access(&mut f.app, |i| {
        let start = Instant::now();
        for &(source, target, id) in &f.transfers {
            black_box(
                i.transfer(f.handles[source], f.handles[target], id)
                    .unwrap(),
            );
        }
        start.elapsed().as_nanos()
    });
    for &(source, target, id) in &f.transfers {
        assert!(expected[source].remove(&id));
        assert!(expected[target].insert(id));
    }
    // Both source and destination sets, all holders, owner buckets and len.
    let transferred = assert_state(&mut f, &expected);
    let (items_ns, listed) = access(&mut f.app, |i| {
        let start = Instant::now();
        let listed = [
            black_box(i.items(f.handles[0]).unwrap()),
            black_box(i.items(f.handles[1]).unwrap()),
        ];
        (start.elapsed().as_nanos(), listed)
    });
    assert_eq!(
        listed.iter().map(Vec::len).sum::<usize>(),
        dataset.target_items
    );
    assert_eq!(checksum(listed.iter().flatten().copied()), target_checksum);
    for index in 0..2 {
        assert_eq!(
            listed[index].iter().copied().collect::<HashSet<_>>(),
            expected[index]
        );
    }
    assert_eq!(assert_state(&mut f, &expected), transferred);
    let target_body = f.bodies[0];
    // The timer includes the administration call, actual Factory identity
    // invalidation, notification, flush, Life/observer work, unwrap and black_box.
    // It is explicitly NOT a pure inventory-kernel cleanup measurement.
    let start = Instant::now();
    with_life_administration(f.app.world_mut(), |world, admin| {
        black_box(ConceptBodyFactory::remove(world, admin, target_body).unwrap());
    });
    let cleanup_ns = start.elapsed().as_nanos();
    assert!(!ConceptBodyFactory::is_live(f.app.world(), target_body));
    for &body in &f.bodies[1..] {
        assert!(ConceptBodyFactory::is_live(f.app.world(), body));
    }
    let mut cleaned = transferred;
    cleaned.items[0] = Err(InventoryError::InvalidInventory);
    cleaned.items[1] = Err(InventoryError::InvalidInventory);
    cleaned.buckets[0] = Err(InventoryError::OwnerNotLive);
    cleaned.len = Ok(dataset.inventory_count - 2 + 1);
    let public = access(&mut f.app, |i| i.public_inventory().unwrap());
    for holder in &mut cleaned.holders {
        if *holder == Ok(Some(f.handles[0])) || *holder == Ok(Some(f.handles[1])) {
            *holder = Ok(Some(public));
        }
    }
    // Includes exact unchanged unrelated sets/holders/owner associations.
    assert_eq!(snapshot(&mut f), cleaned);
    let spilled = access(&mut f.app, |i| i.items(public).unwrap());
    assert_eq!(spilled.len(), dataset.target_items);
    assert_eq!(checksum(spilled), target_checksum);
    access(&mut f.app, |i| {
        assert_eq!(i.bind_owner(target_body), Err(InventoryError::OwnerNotLive));
        assert_eq!(i.create(f.owners[0]), Err(InventoryError::OwnerNotLive));
        for &h in &f.handles[..2] {
            assert_eq!(i.remove(h), Err(InventoryError::InvalidInventory));
        }
    });
    assert_eq!(snapshot(&mut f), cleaned);
    let phases = [
        (
            "insert",
            dataset.total_items,
            insert_ns,
            total_checksum,
            "typed_insert_loop_including_result_unwrap_and_black_box",
        ),
        (
            "holder",
            dataset.total_items,
            holder_ns,
            total_checksum,
            "typed_holder_loop_including_result_unwrap_black_box_and_preallocated_output_writes",
        ),
        (
            "transfer",
            f.transfers.len(),
            transfer_ns,
            transfer_checksum,
            "typed_transfer_loop_including_result_unwrap_and_black_box",
        ),
        (
            "items",
            2,
            items_ns,
            target_checksum,
            "two_typed_owned_snapshots_including_allocation_result_unwrap_and_black_box",
        ),
        (
            "factory_remove_target_owner",
            1,
            cleanup_ns,
            target_checksum,
            "life_administration_factory_identity_invalidation_notification_flush_life_observer_public_spill_result_unwrap_black_box_not_pure_kernel",
        ),
    ];
    phases
        .into_iter()
        .map(
            |(operation, operation_count, elapsed_ns, checksum, measurement_scope)| Measurement {
                dataset,
                sample,
                operation,
                operation_count,
                elapsed_ns,
                checksum,
                measurement_scope,
            },
        )
        .collect()
}

fn measurements() -> Vec<Measurement> {
    let mut report = Vec::new();
    for dataset in DATASETS {
        for sample in 0..3 {
            // Every repetition constructs and drops a fresh real App.
            let measured = measure_sample(dataset, sample);
            for row in &measured {
                row.print();
            }
            report.extend(measured);
        }
    }
    report
}

fn validate_report(report: &[Measurement]) {
    assert_eq!(report.len(), 5 * 3 * 5);
    for (group, dataset) in DATASETS.iter().enumerate() {
        for sample in 0..3 {
            let rows = &report[(group * 3 + sample) * 5..(group * 3 + sample + 1) * 5];
            let operations = [
                "insert",
                "holder",
                "transfer",
                "items",
                "factory_remove_target_owner",
            ];
            let counts = [
                dataset.total_items,
                dataset.total_items,
                if dataset.name == "unrelated" {
                    512
                } else {
                    dataset.total_items
                },
                2,
                1,
            ];
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(row.dataset.imported_group, dataset.imported_group);
                assert_eq!(row.sample, sample);
                assert_eq!(row.operation, operations[index]);
                assert_eq!(row.operation_count, counts[index]);
                assert!(row.checksum > 0);
                assert!(!row.measurement_scope.is_empty());
                // No elapsed-time threshold, capacity inference or memory claim.
            }
            assert_eq!(rows[0].checksum, rows[1].checksum);
            assert_eq!(rows[3].checksum, rows[4].checksum);
        }
    }
}

fn main() {
    validate_report(&measurements());
}
