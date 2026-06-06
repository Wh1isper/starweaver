//! SSE response helpers for replay-backed service routes.

use std::convert::Infallible;

use axum::{
    response::{sse::Event, IntoResponse, Sse},
    Json,
};
use futures_util::stream;
use starweaver_stream::{replay_sse_frames, ReplaySseFrame};

use crate::{controller::EventListResponse, ClawError};

/// Render an event-list response either as JSON or replay SSE frames.
pub fn events_response(events: EventListResponse, as_sse: bool) -> axum::response::Response {
    if as_sse {
        return match replay_sse_frames(&events.events) {
            Ok(frames) => Sse::new(stream::iter(
                frames
                    .into_iter()
                    .map(|frame| Ok::<_, Infallible>(axum_sse_event(frame))),
            ))
            .into_response(),
            Err(error) => ClawError::Failed(error.to_string()).into_response(),
        };
    }
    Json(events).into_response()
}

/// Return an SSE response with a ready frame.
pub fn ready_sse_response() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    Sse::new(stream::iter([Ok::<_, Infallible>(axum_sse_event(
        ReplaySseFrame::ready(),
    ))]))
}

fn axum_sse_event(frame: ReplaySseFrame) -> Event {
    let event = Event::default().event(frame.event).data(frame.data);
    if frame.id.is_empty() {
        event
    } else {
        event.id(frame.id)
    }
}
