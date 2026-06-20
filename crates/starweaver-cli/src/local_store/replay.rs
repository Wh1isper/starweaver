//! Local display replay windows over shared stream cursor contracts.

use starweaver_stream::{DisplayMessage, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayScope};

use super::LocalStore;
use crate::{CliError, CliResult};

/// Display replay events and the next sequence for live tail continuation.
#[derive(Clone, Debug)]
pub struct DisplayReplayWindow {
    /// Replay scope used by the events.
    pub scope: ReplayScope,
    /// Replay events after the requested cursor.
    pub events: Vec<ReplayEvent>,
    /// Next sequence number for live events in this scope.
    pub next_sequence: usize,
}

impl LocalStore {
    /// Replay display messages as scoped replay events.
    pub fn replay_display_window(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        cursor: Option<&ReplayCursor>,
    ) -> CliResult<DisplayReplayWindow> {
        let scope = run_id.map_or_else(|| ReplayScope::session(session_id), ReplayScope::run);
        if let Some(cursor) = cursor {
            cursor
                .validate_scope(&scope)
                .map_err(|error| CliError::Usage(error.to_string()))?;
        }
        let messages = self.replay_display(session_id, run_id, None)?;
        let (events, next_sequence) = if run_id.is_some() {
            let next_sequence = messages
                .last()
                .map_or(0, |message| message.sequence.saturating_add(1));
            (
                messages
                    .into_iter()
                    .map(|message| display_replay_event(&scope, message.sequence, message))
                    .collect::<Vec<_>>(),
                next_sequence,
            )
        } else {
            let next_sequence = messages.len();
            (
                messages
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, message)| display_replay_event(&scope, sequence, message))
                    .collect::<Vec<_>>(),
                next_sequence,
            )
        };
        Ok(DisplayReplayWindow {
            scope,
            events: filter_replay_events(events, cursor),
            next_sequence,
        })
    }
}

fn display_replay_event(
    scope: &ReplayScope,
    sequence: usize,
    message: DisplayMessage,
) -> ReplayEvent {
    ReplayEvent::new(
        scope.clone(),
        sequence,
        ReplayEventKind::DisplayMessage(Box::new(message)),
    )
}

fn filter_replay_events(
    events: Vec<ReplayEvent>,
    cursor: Option<&ReplayCursor>,
) -> Vec<ReplayEvent> {
    events
        .into_iter()
        .filter(|event| cursor.map_or(true, |cursor| event.sequence > cursor.sequence))
        .collect()
}
