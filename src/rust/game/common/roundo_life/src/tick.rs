use bevy::prelude::{App, FixedUpdate, IntoScheduleConfigs, Plugin, ResMut, Resource, SystemSet};

/// The sole authoritative server logic clock used by Life data.
#[derive(Resource, Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct AuthoritativeFixedTick(u64);

impl AuthoritativeFixedTick {
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Startup-only logical history bound copied into each new State Slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateHistoryConfig {
    max_history: usize,
}

impl StateHistoryConfig {
    pub const fn new(max_history: usize) -> Self {
        Self { max_history }
    }

    pub const fn max_history(self) -> usize {
        self.max_history
    }
}

impl Default for StateHistoryConfig {
    fn default() -> Self {
        Self::new(3)
    }
}

#[derive(Resource)]
pub(crate) struct FrozenStateHistoryConfig(StateHistoryConfig);

impl FrozenStateHistoryConfig {
    pub(crate) const fn max_history(&self) -> usize {
        self.0.max_history()
    }
}

/// Runtime maintenance ordering points. Event users run after `ExpireEvents`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum LifeFixedSet {
    AdvanceTick,
    ExpireEvents,
    /// Serial Factory/typed structure commit; enqueue before, consume results after.
    CommitFactories,
}

/// Installs process-wide Life runtime resources and maintenance.
pub struct LifeRuntimePlugin {
    state_history: StateHistoryConfig,
}

impl LifeRuntimePlugin {
    pub const fn new(state_history: StateHistoryConfig) -> Self {
        Self { state_history }
    }
}

impl Default for LifeRuntimePlugin {
    fn default() -> Self {
        Self::new(StateHistoryConfig::default())
    }
}

impl Plugin for LifeRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuthoritativeFixedTick>()
            .init_resource::<crate::management::FactoryRequests>()
            .init_resource::<crate::admin::RegistrationReverseIndex>()
            .init_resource::<crate::resource::DataResources>()
            .insert_resource(FrozenStateHistoryConfig(self.state_history))
            .configure_sets(
                FixedUpdate,
                (
                    LifeFixedSet::AdvanceTick,
                    LifeFixedSet::ExpireEvents,
                    LifeFixedSet::CommitFactories,
                )
                    .chain(),
            )
            .add_systems(FixedUpdate, advance_tick.in_set(LifeFixedSet::AdvanceTick))
            .add_systems(
                FixedUpdate,
                crate::management::commit_factory_requests.in_set(LifeFixedSet::CommitFactories),
            );
    }

    fn finish(&self, app: &mut App) {
        crate::data::close_life_data_catalogue(app)
            .expect("Life data catalogue closes after all plugins register source types");
    }
}

fn advance_tick(mut tick: ResMut<AuthoritativeFixedTick>) {
    tick.0 = tick
        .0
        .checked_add(1)
        .expect("authoritative fixed tick exhausted");
}
