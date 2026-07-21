use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::Serialize;
use tauri::ipc::Channel;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActivation {
    pub kind: ActivationKind,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationKind {
    SecondaryLaunch,
}

/// Process-owned state that survives renderer reloads and window recreation.
const MAX_ACTIVATION_SUBSCRIPTIONS: usize = 16;

pub struct DesktopState {
    launch_generation: AtomicU64,
    next_subscription_token: AtomicU64,
    activation_subscriptions: Mutex<BTreeMap<u64, Channel<DesktopActivation>>>,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            launch_generation: AtomicU64::new(1),
            next_subscription_token: AtomicU64::new(1),
            activation_subscriptions: Mutex::new(BTreeMap::new()),
        }
    }
}

impl DesktopState {
    pub fn launch_generation(&self) -> u64 {
        self.launch_generation.load(Ordering::Acquire)
    }

    pub fn record_secondary_launch(&self) -> DesktopActivation {
        DesktopActivation {
            kind: ActivationKind::SecondaryLaunch,
            generation: self.launch_generation.fetch_add(1, Ordering::AcqRel) + 1,
        }
    }

    pub fn subscribe_to_activations(&self, channel: Channel<DesktopActivation>) -> u64 {
        let token = self.next_subscription_token.fetch_add(1, Ordering::AcqRel);
        let mut subscriptions = self
            .activation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while subscriptions.len() >= MAX_ACTIVATION_SUBSCRIPTIONS {
            subscriptions.pop_first();
        }
        subscriptions.insert(token, channel);
        token
    }

    pub fn unsubscribe_from_activations(&self, token: u64) {
        self.activation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&token);
    }

    pub fn publish_activation(&self, activation: DesktopActivation) {
        let mut subscriptions = self
            .activation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = subscriptions.len();
        subscriptions.retain(|_, channel| channel.send(activation).is_ok());
        let failed = before - subscriptions.len();
        drop(subscriptions);
        if failed > 0 {
            eprintln!("failed to notify {failed} renderer activation subscription(s)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_launch_advances_generation() {
        let state = DesktopState::default();

        assert_eq!(state.launch_generation(), 1);
        assert_eq!(
            state.record_secondary_launch(),
            DesktopActivation {
                kind: ActivationKind::SecondaryLaunch,
                generation: 2,
            }
        );
        assert_eq!(state.launch_generation(), 2);
    }
}
