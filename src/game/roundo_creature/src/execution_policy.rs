//! Caller-owned input/readiness policy, retained across application teardown.
use crate::FactoryMotionRoute;
use bevy::prelude::Component;
use roundo_life::AuthoritativeFixedTick;

/// Exactly one jump interpretation can drive an Execution. Legacy Bodies with
/// no explicit mode keep held-up behavior; the new recipe requires Discrete.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub enum JumpInputMode {
    HeldUp,
    Discrete,
}
/// RequireReadiness never falls back to free/non-Life solve when its route or
/// gate disappears. This permanent policy belongs to the caller's prototype.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryMotionPolicy {
    Ungated,
    RequireReadiness,
}
/// Local gate written only by trusted composition/world adapters. This value is
/// not a readiness proof: adapters must check actual revisions and current route.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct FactoryMotionReadiness {
    ready: Option<(FactoryMotionRoute, AuthoritativeFixedTick)>,
}
impl FactoryMotionReadiness {
    pub const fn is_blocked(&self) -> bool {
        self.ready.is_none()
    }
    pub fn block(&mut self) {
        self.ready = None;
    }
    pub fn set_ready(&mut self, route: FactoryMotionRoute, tick: AuthoritativeFixedTick) {
        self.ready = Some((route, tick));
    }
    pub(crate) fn permits(&self, route: FactoryMotionRoute, tick: AuthoritativeFixedTick) -> bool {
        self.ready == Some((route, tick))
    }
}
