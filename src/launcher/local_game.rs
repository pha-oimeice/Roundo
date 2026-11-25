use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::{PhysicsPlugins, SpatialQuery, SpatialQueryFilter};
use bevy::{
    input::InputSystems,
    prelude::*,
    transform::TransformSystems,
    ui::IsDefaultUiCamera,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use bevy_enhanced_input::prelude::*;
use roundo_creature::{
    AabbDimensions, ContactTolerance, CreatureMotionPlugin, CreatureMotionSet, EnvironmentSample,
    GazeAction, GazeOrientation, GroundLocomotion, KinematicVelocity, MovementAction,
    TestExecutionBodyPrototype,
};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, CoordinateIncarnation, LocalCoordinateServerPlugin,
    LocalCoordinateServerSet, LocalPhysicsDemands, PcgLocalCoordinate, PhysicsDemandBudget,
    PhysicsDemandId, PhysicsDemandRegion, PhysicsRegionReadiness, SuperflatGenerator,
    VoxelEditBudget, WorldInteractionPlugin, WorldInteractionRuntimeBudgets,
};
use roundo_rendering::DebugCamera;

use super::{
    config_store::ClientConfigStore,
    local_input::{
        LocalBindings, LocalCharacter, LocalIntent, refresh_character_bindings, sample_actions,
    },
};

pub(super) struct LocalGamePlugin;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, States)]
pub(super) enum LocalScreen {
    #[default]
    Launcher,
    Game,
}

#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalControl {
    #[default]
    Loading,
    Playing,
    Menu,
}

#[derive(Component)]
struct LocalView;

#[derive(Resource, Default)]
struct LocalTerrain {
    interest: Option<TerrainInterest>,
    ready_for_motion: bool,
}

struct TerrainInterest {
    coordinate: CoordinateIncarnation,
    demand: PhysicsDemandId,
    requested: PhysicsDemandRegion,
}

// Keep a one-chunk buffer around the character on the superflat surface.
// These are demand topology limits, not per-user key or movement defaults.
const CHUNK_MARGIN: IVec3 = IVec3::new(1, 0, 1);
const REGION_CAPACITY: usize = ((2 * CHUNK_MARGIN.x + 1) * (2 * CHUNK_MARGIN.z + 1)) as usize;

impl Plugin for LocalGamePlugin {
    fn build(&self, app: &mut App) {
        let bindings =
            LocalBindings::resolve(app.world().resource::<ClientConfigStore>().settings())
                .unwrap_or_else(|error| panic!("Invalid client/input.toml: {error}"));
        let physics =
            PhysicsDemandBudget::new(1, REGION_CAPACITY, REGION_CAPACITY, REGION_CAPACITY)
                .expect("one local terrain region");
        // No edit consumer is installed by the local host.
        let edits = VoxelEditBudget::new(1, 1).expect("nonzero runtime edit limits");
        app.init_state::<LocalScreen>()
            .init_resource::<LocalControl>()
            .init_resource::<LocalTerrain>()
            .insert_resource(bindings)
            .add_plugins((
                PhysicsPlugins::default(),
                CreatureMotionPlugin,
                LocalCoordinateServerPlugin::default(),
                WorldInteractionPlugin::new(WorldInteractionRuntimeBudgets::new(edits, physics)),
                EnhancedInputPlugin,
            ))
            .add_input_context::<LocalCharacter>()
            .add_systems(OnEnter(LocalScreen::Game), start_local_terrain)
            .add_systems(OnExit(LocalScreen::Game), stop_local_game)
            .add_systems(
                PreUpdate,
                (
                    (refresh_character_bindings, sync_capture)
                        .chain()
                        .after(InputSystems)
                        .before(EnhancedInputSystems::Update),
                    sample_actions.after(EnhancedInputSystems::Apply),
                )
                    .run_if(in_state(LocalScreen::Game)),
            )
            .add_systems(
                Update,
                (follow_terrain, spawn_character)
                    .chain()
                    .before(LocalCoordinateServerSet::Prepare)
                    .run_if(in_state(LocalScreen::Game)),
            )
            .add_systems(
                FixedUpdate,
                (check_motion_region, project_intent)
                    .chain()
                    .before(CreatureMotionSet::Commit)
                    .run_if(in_state(LocalScreen::Game)),
            )
            .configure_sets(
                FixedUpdate,
                CreatureMotionSet::Commit
                    .run_if(|terrain: Res<LocalTerrain>| terrain.ready_for_motion),
            )
            .add_systems(PostUpdate, sync_view.before(TransformSystems::Propagate));
    }
}

fn spawn_position() -> Vec3 {
    // Horizontal center of the origin voxel, one unit above its top surface.
    Vec3::new(0.5, SuperflatGenerator::default().height as f32 + 2.0, 0.5)
}

fn ground_chunk(position: Vec3) -> IVec3 {
    let edge = CHUNK_EDGE_LENGTH as f32;
    IVec3::new(
        (position.x / edge).floor() as i32,
        SuperflatGenerator::default()
            .height
            .div_euclid(CHUNK_EDGE_LENGTH as i64) as i32,
        (position.z / edge).floor() as i32,
    )
}

fn buffered_region(demands: &LocalPhysicsDemands, position: Vec3) -> PhysicsDemandRegion {
    let center = ground_chunk(position);
    demands
        .budgets()
        .expect("local terrain demand runtime is installed")
        .physics_demands()
        .region(center - CHUNK_MARGIN, center + CHUNK_MARGIN)
        .expect("buffered local terrain fits its demand budget")
}

fn start_local_terrain(
    ground: Single<Entity, With<PcgLocalCoordinate>>,
    mut demands: LocalPhysicsDemands,
    mut terrain: ResMut<LocalTerrain>,
) {
    let coordinate = demands
        .bind_coordinate(*ground)
        .expect("local ground coordinate");
    let requested = buffered_region(&demands, spawn_position());
    let demand = demands
        .register(coordinate, requested)
        .expect("local terrain demand");
    terrain.interest = Some(TerrainInterest {
        coordinate,
        demand,
        requested,
    });
}

fn follow_terrain(
    character: Option<Single<&Transform, With<LocalCharacter>>>,
    mut demands: LocalPhysicsDemands,
    mut terrain: ResMut<LocalTerrain>,
) {
    let Some(character) = character else { return };
    let interest = terrain
        .interest
        .as_mut()
        .expect("an active character has a terrain demand");
    let requested = buffered_region(&demands, character.translation);
    if interest.requested != requested {
        demands
            .update(interest.demand, requested)
            .expect("live local terrain demand");
        interest.requested = requested;
    }
}

fn spawn_character(
    mut commands: Commands,
    mut control: ResMut<LocalControl>,
    terrain: Res<LocalTerrain>,
    demands: LocalPhysicsDemands,
    spatial: SpatialQuery,
    bindings: Res<LocalBindings>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut debug: Single<&mut Camera, With<DebugCamera>>,
) {
    if *control != LocalControl::Loading {
        return;
    }
    let interest = terrain.interest.as_ref().expect("loading local terrain");
    if demands
        .readiness(interest.coordinate, interest.requested)
        .expect("live local terrain")
        != PhysicsRegionReadiness::Ready
    {
        return;
    }
    let position = spawn_position();
    // ECS collider components can be ready before Avian's next spatial update.
    if spatial
        .cast_ray(
            position,
            Dir3::NEG_Y,
            2.0,
            true,
            &SpatialQueryFilter::default(),
        )
        .is_none()
    {
        return;
    }

    let prototype = TestExecutionBodyPrototype::default();
    let character = commands
        .spawn((
            Name::new("Local Character"),
            LocalCharacter,
            LocalIntent::default(),
            ContextActivity::<LocalCharacter>::INACTIVE,
            DespawnOnExit(LocalScreen::Game),
            prototype.materialize_runtime(position, EnvironmentSample::default()),
        ))
        .id();
    bindings.attach(&mut commands, character);
    commands.spawn((
        Name::new("Local First-Person View"),
        LocalView,
        Camera3d::default(),
        IsDefaultUiCamera,
        Transform::from_translation(
            Vec3::Y * (prototype.aabb.size().y - prototype.contact_tolerance),
        ),
        ChildOf(character),
    ));
    debug.is_active = false;
    *control = if window.focused {
        LocalControl::Playing
    } else {
        LocalControl::Menu
    };
}

fn sync_capture(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut control: ResMut<LocalControl>,
    character: Option<Single<(Entity, &ContextActivity<LocalCharacter>)>>,
) {
    let (window, cursor) = &mut *window;
    // Escape is reserved by the host in the existing input schema.
    if *control == LocalControl::Playing && (!window.focused || keys.just_pressed(KeyCode::Escape))
    {
        *control = LocalControl::Menu;
    }
    let Some(character) = character else { return };
    let (entity, activity) = character.into_inner();
    let captured = *control == LocalControl::Playing;
    if **activity != captured {
        commands
            .entity(entity)
            .insert(ContextActivity::<LocalCharacter>::new(captured));
    }
    set_cursor(cursor, captured);
}

fn set_cursor(cursor: &mut Mut<CursorOptions>, captured: bool) {
    let mode = if captured {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    if cursor.grab_mode != mode || cursor.visible == captured {
        cursor.visible = !captured;
        cursor.grab_mode = mode;
    }
}

fn check_motion_region(
    mut terrain: ResMut<LocalTerrain>,
    demands: LocalPhysicsDemands,
    time: Res<Time<Fixed>>,
    character: Option<
        Single<
            (
                &Transform,
                &AabbDimensions,
                &KinematicVelocity,
                &GroundLocomotion,
                &EnvironmentSample,
                &ContactTolerance,
            ),
            With<LocalCharacter>,
        >,
    >,
) {
    let ready = character.is_some_and(|character| {
        let (pose, aabb, velocity, locomotion, environment, tolerance) = *character;
        let radius = aabb.half_extents()
            + Vec3::splat(
                locomotion.sweep_distance_bound(velocity.0, environment.gravity, time.delta_secs())
                    + tolerance.0,
            );
        let interest = terrain
            .interest
            .as_ref()
            .expect("an active character has a terrain demand");
        let region = demands
            .budgets()
            .expect("local terrain runtime")
            .physics_demands()
            .region(
                ground_chunk(pose.translation - radius),
                ground_chunk(pose.translation + radius),
            )
            .expect("one local motion sweep fits the loaded terrain budget");
        demands
            .readiness(interest.coordinate, region)
            .expect("live local terrain")
            == PhysicsRegionReadiness::Ready
    });
    terrain.ready_for_motion = ready;
}

fn project_intent(
    terrain: Res<LocalTerrain>,
    config: Res<ClientConfigStore>,
    character: Option<
        Single<
            (
                &GazeOrientation,
                &mut LocalIntent,
                &mut MovementAction,
                &mut GazeAction,
            ),
            With<LocalCharacter>,
        >,
    >,
) {
    let Some(character) = character else { return };
    let (gaze, mut intent, mut movement, mut look) = character.into_inner();
    if !terrain.ready_for_motion {
        return;
    }
    *movement = MovementAction::new(intent.take_movement().to_array())
        .expect("validated local movement axis");
    let delta =
        -std::mem::take(&mut intent.pending_look) * config.settings().controls.mouse_sensitivity;
    let (_, current_pitch, _) = gaze.0.to_euler(EulerRot::YXZ);
    let limit = FRAC_PI_2 - 0.01;
    let pitch_delta = (current_pitch + delta.y).clamp(-limit, limit) - current_pitch;
    *look = GazeAction::new(delta.x, pitch_delta).expect("finite local mouse motion");
}

fn sync_view(
    gaze: Query<&GazeOrientation, With<LocalCharacter>>,
    mut views: Query<(&ChildOf, &mut Transform), With<LocalView>>,
) {
    for (parent, mut view) in &mut views {
        view.rotation = gaze
            .get(parent.parent())
            .expect("local view parent has gaze")
            .0;
    }
}

fn stop_local_game(
    mut terrain: ResMut<LocalTerrain>,
    mut demands: LocalPhysicsDemands,
    mut control: ResMut<LocalControl>,
    cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut debug: Single<&mut Camera, With<DebugCamera>>,
) {
    let interest = terrain
        .interest
        .take()
        .expect("an active local session owns a demand");
    demands
        .remove(interest.demand)
        .expect("live local terrain demand");
    terrain.ready_for_motion = false;
    *control = LocalControl::Loading;
    set_cursor(&mut cursor.into_inner(), false);
    debug.is_active = true;
}
