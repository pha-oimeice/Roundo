//! The production transaction pump. Native and headless adapters cross this seam.

use crate::UiLifecycleManager;
use crate::state::{NativeActivity, NativeFact, RecoveryId, UiAction, UiOpeningId, UiViewLifetime};
use std::time::Instant;

pub(super) struct NativeInput {
    pub(super) activity: NativeActivity,
    pub(super) facts: Vec<NativeFact>,
    pub(super) focus_requests: Vec<UiOpeningId>,
}

/// Physical ownership only. Decisions about creation, commit, presentation and
/// retirement belong to UiViewLifetime, never to an adapter's collection layout.
/// Every poll must sample actual current activity, not the preceding work batch.
pub(super) trait ViewDriver {
    fn poll(&mut self, lifetime: &UiViewLifetime, manager: &UiLifecycleManager) -> NativeInput;
    fn revoke_stale(&mut self, lifetime: &UiViewLifetime);
    fn execute(&mut self, action: UiAction, manager: &UiLifecycleManager) -> Vec<NativeFact>;
    fn create_recovery(&mut self, recovery: RecoveryId) -> Vec<NativeFact>;
    fn close_recovery(&mut self) -> Vec<NativeFact>;
}

fn observe(lifetime: &mut UiViewLifetime, facts: Vec<NativeFact>) {
    for fact in facts {
        lifetime.observe(fact);
    }
}

/// Executes one outer host update. A result is consumed on a later update with
/// a fresh physical snapshot; responding or dispatching a destroying script can
/// never be followed by Close/Create using the old snapshot in this call.
pub(super) fn drive_views(
    lifetime: &mut UiViewLifetime,
    manager: &mut UiLifecycleManager,
    driver: &mut impl ViewDriver,
    now: Instant,
) {
    driver.revoke_stale(lifetime);
    let input = driver.poll(lifetime, manager);
    for opening in input.focus_requests {
        if let Some(instance) = lifetime.committed_identity(opening) {
            let _ = manager.focus(instance);
        }
    }
    observe(lifetime, input.facts);
    let actions = lifetime.advance(manager, now, &input.activity);
    driver.revoke_stale(lifetime);
    if !actions.is_empty() {
        for action in actions {
            match action {
                UiAction::SettleOpenResponse {
                    opening,
                    sender,
                    value,
                } => {
                    log::trace!("settling exact UI Opening response: {opening:?}");
                    if sender.respond(value).is_err() {
                        log::debug!("UI Open caller disappeared before settlement");
                    }
                }
                action => observe(lifetime, driver.execute(action, manager)),
            }
        }
        return;
    }

    // Poll again: even an otherwise empty update may have issued a native
    // readiness probe or observed a callback. Do not reuse input.activity.
    let fresh = driver.poll(lifetime, manager);
    if !fresh.facts.is_empty() || !fresh.focus_requests.is_empty() {
        observe(lifetime, fresh.facts);
        for opening in fresh.focus_requests {
            if let Some(instance) = lifetime.committed_identity(opening) {
                let _ = manager.focus(instance);
            }
        }
        return;
    }
    if lifetime.can_close_recovery(&fresh.activity) {
        observe(lifetime, driver.close_recovery());
    } else if lifetime.can_create_recovery(&fresh.activity) {
        if let Some((recovery, _)) = lifetime.recovery_request() {
            observe(lifetime, driver.create_recovery(recovery));
        }
    }
}
