//! Owns the fixed Creature recipe's complete success/compensation protocol.
//! The FIFO crosses this seam once; native phases and exact Life/Controller
//! commits retain their own restricted interfaces and identity rules.

use super::*;
use bevy::ecs::system::RunSystemOnce;

/// The Player that receives the completed production assembly.
enum AssemblyTarget {
    Player(Entity),
}

/// Only successful completions enter this ledger. These facts are construction
/// progress, not Factory identity, registration truth or authorization grants.
struct CreatureAssembly {
    scope: LifeCommitScope,
    controller_scope: Option<ControllerCommitScope>,
    life: Option<LifeHandle>,
    intent: Option<ConceptBodyHandle>,
    execution: Option<ConceptBodyHandle>,
    record: Option<ConceptBodyHandle>,
    movement: Option<ControllerId>,
    gaze: Option<ControllerId>,
    interaction: Option<ControllerId>,
}

impl CreatureAssembly {
    fn begin(world: &mut World, target: &AssemblyTarget) -> Result<Self, CreatureAssemblyError> {
        let (scope, controller_scope) = match target {
            AssemblyTarget::Player(_) => {
                let (life, controllers) = world
                    .run_system_once(lifecycle::begin_bootstrap_scopes)
                    .expect("bootstrap scopes have their declared lifecycle resources")?;
                (life, Some(controllers))
            }
        };
        Ok(Self {
            scope,
            controller_scope,
            life: None,
            intent: None,
            execution: None,
            record: None,
            movement: None,
            gaze: None,
            interaction: None,
        })
    }

    fn construct(
        &mut self,
        world: &mut World,
        target: AssemblyTarget,
        translation: bevy::prelude::Vec3,
        environment: EnvironmentSample,
    ) -> Result<UnpublishedCreatureLife, CreatureAssemblyError> {
        world.init_resource::<BodyInstanceIds>();
        world.init_resource::<CreatureAssemblyRegistry>();
        let (life, recipe) =
            lifecycle::create_test_life(world, self.scope, translation, environment)?;
        self.life = Some(life);
        let intent = lifecycle::created_body(world, self.scope, recipe.intent)?;
        self.intent = Some(intent);
        let execution = lifecycle::created_body(world, self.scope, recipe.execution)?;
        self.execution = Some(execution);
        let record = lifecycle::created_body(world, self.scope, recipe.record)?;
        self.record = Some(record);
        let assembly = UnpublishedCreatureLife {
            life,
            members: CreatureAssemblyMembers {
                intent,
                execution,
                record,
            },
        };
        let baseline = world
            .run_system_once_with(prepare_creature_data, assembly)
            .expect("bootstrap preparation resources are installed")?;
        lifecycle::created_slot(
            world,
            self.scope,
            life,
            intent,
            MovementCommandState::default(),
        )?;
        lifecycle::created_slot(world, self.scope, life, intent, GazeCommandState::default())?;
        world
            .run_system_once_with(prepare_creature_experience, assembly)
            .expect("bootstrap typed management resources are installed")?;
        lifecycle::created_slot(world, self.scope, life, execution, baseline)?;
        world
            .run_system_once_with(finish_creature_data, assembly)
            .expect("bootstrap finalization resources are installed")?;

        let player = match target {
            AssemblyTarget::Player(player) => player,
        };
        let controller_scope = self
            .controller_scope
            .expect("Player assembly reserves both scopes before construction");
        world
            .run_system_once_with(attach_block_interaction, assembly)
            .expect("bootstrap attachment resources are installed")?;
        let movement = lifecycle::create_controller(
            world,
            controller_scope,
            intent,
            IntentDomain::movement(),
        )?;
        self.movement = Some(movement);
        let gaze =
            lifecycle::create_controller(world, controller_scope, intent, IntentDomain::gaze())?;
        self.gaze = Some(gaze);
        let interaction = lifecycle::create_controller(
            world,
            controller_scope,
            intent,
            IntentDomain::block_interaction(),
        )?;
        self.interaction = Some(interaction);
        world
            .run_system_once_with(attach_interaction_controller, (assembly, interaction))
            .expect("interaction attachment has declared resources")?;
        let controllers = TestLifeControllers { movement, gaze };
        world
            .run_system_once_with(attach_test_life_controllers, (assembly, controllers))
            .expect("bootstrap Controller attachment resources are installed")?;
        world
            .run_system_once_with(publish_test_life, (assembly, player, controllers))
            .expect("bootstrap publication resources are installed")?;
        Ok(assembly)
    }

    fn compensate(&self, world: &mut World) {
        let Some(life) = self.life else {
            return;
        };
        if let Some(execution) = self.execution {
            // Err/panic can occur after a native phase published derived facts.
            // Never condition this idempotent cleanup on a publication flag.
            world
                .run_system_once_with(clear_failed_bootstrap, (life, execution))
                .expect("assembly compensation has its declared resources");
        }
        if let Some(scope) = self.controller_scope {
            lifecycle::remove_controllers(
                world,
                scope,
                [self.interaction, self.gaze, self.movement]
                    .into_iter()
                    .flatten(),
            );
        }
        lifecycle::remove_instances(
            world,
            self.scope,
            life,
            [self.record, self.execution, self.intent]
                .into_iter()
                .flatten(),
        );
    }

    fn finish(&self, world: &mut World) {
        if let Some(scope) = self.controller_scope {
            world
                .run_system_once_with(lifecycle::finish_controller_scope, scope)
                .expect("bootstrap Controller scope has its declared resource")
                .expect("bootstrap consumed every Controller result before finishing");
        }
        finish_bootstrap_scope(world, self.scope);
    }
}

/// A closed, fixed recipe, not a general World callback host. The sole unwind
/// owner covers every successful instance, native phase and publication. Kernel
/// helpers consume Panicked before rethrowing so compensation can use its slot.
fn run_assembly(
    world: &mut World,
    target: AssemblyTarget,
    translation: bevy::prelude::Vec3,
    environment: EnvironmentSample,
) -> Result<UnpublishedCreatureLife, CreatureAssemblyError> {
    let mut assembly = CreatureAssembly::begin(world, &target)?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assembly.construct(world, target, translation, environment)
    }));
    if !matches!(&result, Ok(Ok(_))) {
        assembly.compensate(world);
    }
    assembly.finish(world);
    match result {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Completes the fixed recipe synchronously at the caller's FIFO position.
/// Success includes flushed markers, mapping and unacquired ReadWrite grants.
/// Err/unwind compensates successful instances in reverse order and ends both
/// scopes; arbitrary hook side effects or a second cleanup panic are not rolled
/// back. No scheduled queue is flushed and the Life clock is never advanced.
pub(super) fn assemble_test_life_for_player(
    world: &mut World,
    player: Entity,
    translation: bevy::prelude::Vec3,
    environment: EnvironmentSample,
) -> Result<LifeHandle, CreatureAssemblyError> {
    Ok(run_assembly(
        world,
        AssemblyTarget::Player(player),
        translation,
        environment,
    )?
    .life)
}

pub(super) fn finish_bootstrap_scope(world: &mut World, scope: LifeCommitScope) {
    world
        .run_system_once_with(lifecycle::finish_scope, scope)
        .expect("bootstrap scope has its declared lifecycle resource")
        .expect("bootstrap consumed every structural result before finishing");
}
