#![allow(unsafe_code)]

//! Audited macOS session-transition monitoring boundary.
//!
//! A dedicated worker continuously samples `CGSessionCopyCurrentDictionary`,
//! which is the supported synchronous CoreGraphics view of the caller's GUI
//! session. Retaining the previous lock, console, user, audit, and lock-time
//! values lets the worker invalidate an observation even when the session has
//! returned to its original state before the next backend operation.

use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    thread::{self, JoinHandle},
    time::Duration,
};

use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFNumberType, CFRetained, CFString, CFType,
};
use objc2_core_graphics::CGSessionCopyCurrentDictionary;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const FENCE_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionMonitorError {
    RegistrationFailed,
    CheckFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionProbe {
    locked: bool,
    on_console: bool,
    login_done: bool,
    user_id: i64,
    audit_id: Option<i64>,
    locked_time: Option<i64>,
}

enum MonitorCommand {
    Fence(SyncSender<Result<u64, SessionMonitorError>>),
    Shutdown,
}

pub(super) struct SessionTransitionMonitor {
    commands: Option<Sender<MonitorCommand>>,
    worker: Option<JoinHandle<()>>,
    registration_failed: bool,
}

impl SessionTransitionMonitor {
    pub(super) fn new() -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("starweaver-macos-session-monitor".to_owned())
            .spawn(move || run_monitor(&command_rx, &startup_tx));

        let Ok(worker) = worker else {
            return Self {
                commands: None,
                worker: None,
                registration_failed: true,
            };
        };
        let registered = startup_rx.recv_timeout(STARTUP_TIMEOUT) == Ok(Ok(()));
        Self {
            commands: registered.then_some(commands),
            worker: Some(worker),
            registration_failed: !registered,
        }
    }

    pub(super) fn poll_epoch(&self) -> Result<u64, SessionMonitorError> {
        if self.registration_failed {
            return Err(SessionMonitorError::RegistrationFailed);
        }
        let commands = self
            .commands
            .as_ref()
            .ok_or(SessionMonitorError::RegistrationFailed)?;
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        commands
            .send(MonitorCommand::Fence(response_tx))
            .map_err(|_| SessionMonitorError::CheckFailed)?;
        response_rx
            .recv_timeout(FENCE_TIMEOUT)
            .map_err(|_| SessionMonitorError::CheckFailed)?
    }
}

impl Drop for SessionTransitionMonitor {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(MonitorCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_monitor(
    commands: &Receiver<MonitorCommand>,
    startup: &SyncSender<Result<(), SessionMonitorError>>,
) {
    let mut last_probe = match session_probe() {
        Ok(probe) => {
            let _ = startup.send(Ok(()));
            Ok(probe)
        }
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let mut epoch = 0_u64;

    loop {
        match commands.recv_timeout(POLL_INTERVAL) {
            Ok(MonitorCommand::Fence(response)) => {
                record_probe(&mut last_probe, session_probe(), &mut epoch);
                let result = last_probe.as_ref().map(|_| epoch).map_err(|error| *error);
                let _ = response.send(result);
            }
            Ok(MonitorCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                record_probe(&mut last_probe, session_probe(), &mut epoch);
            }
        }
    }
}

fn record_probe(
    previous: &mut Result<SessionProbe, SessionMonitorError>,
    current: Result<SessionProbe, SessionMonitorError>,
    epoch: &mut u64,
) {
    let changed = match (&*previous, &current) {
        (Ok(previous), Ok(current)) => previous != current,
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => true,
        (Err(previous), Err(current)) => previous != current,
    };
    if changed {
        *epoch = epoch.saturating_add(1);
    }
    *previous = current;
}

fn session_probe() -> Result<SessionProbe, SessionMonitorError> {
    let dictionary = CGSessionCopyCurrentDictionary().ok_or(SessionMonitorError::CheckFailed)?;
    // SAFETY: CoreGraphics documents this as a property-list dictionary. Its
    // keys are CFStrings and all property-list values are CFType instances;
    // Core Foundation dictionary runtime identity does not encode generics.
    let dictionary =
        unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(dictionary) };

    Ok(SessionProbe {
        locked: bool_or_false_if_absent(&dictionary, "CGSSessionScreenIsLocked")?,
        on_console: required_bool(&dictionary, "kCGSSessionOnConsoleKey")?,
        login_done: required_bool(&dictionary, "kCGSessionLoginDoneKey")?,
        user_id: required_number(&dictionary, "kCGSSessionUserIDKey")?,
        audit_id: optional_number(&dictionary, "kCGSSessionAuditIDKey")?,
        locked_time: optional_number(&dictionary, "CGSSessionScreenLockedTime")?,
    })
}

fn required_bool(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Result<bool, SessionMonitorError> {
    dictionary_bool(dictionary, key)?.ok_or(SessionMonitorError::CheckFailed)
}

fn bool_or_false_if_absent(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Result<bool, SessionMonitorError> {
    Ok(dictionary_bool(dictionary, key)?.unwrap_or(false))
}

fn dictionary_bool(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Result<Option<bool>, SessionMonitorError> {
    let Some(value) = dictionary.get(&CFString::from_str(key)) else {
        return Ok(None);
    };
    value
        .downcast::<CFBoolean>()
        .map(|value| Some(value.value()))
        .map_err(|_| SessionMonitorError::CheckFailed)
}

fn required_number(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Result<i64, SessionMonitorError> {
    optional_number(dictionary, key)?.ok_or(SessionMonitorError::CheckFailed)
}

fn optional_number(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Result<Option<i64>, SessionMonitorError> {
    let Some(value) = dictionary.get(&CFString::from_str(key)) else {
        return Ok(None);
    };
    let value = value
        .downcast::<CFNumber>()
        .map_err(|_| SessionMonitorError::CheckFailed)?;
    let mut output = 0_i64;
    // SAFETY: `output` is writable and correctly aligned for the requested
    // signed 64-bit representation for this synchronous conversion.
    let converted = unsafe {
        value.value(
            CFNumberType::SInt64Type,
            (&raw mut output).cast::<std::ffi::c_void>(),
        )
    };
    converted
        .then_some(Some(output))
        .ok_or(SessionMonitorError::CheckFailed)
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};

    use super::{
        SessionMonitorError, SessionProbe, SessionTransitionMonitor, bool_or_false_if_absent,
        record_probe, session_probe,
    };

    const LOCKED_KEY: &str = "CGSSessionScreenIsLocked";

    fn probe(locked: bool, on_console: bool) -> SessionProbe {
        SessionProbe {
            locked,
            on_console,
            login_done: true,
            user_id: 501,
            audit_id: Some(100_003),
            locked_time: locked.then_some(123),
        }
    }

    #[test]
    fn absent_lock_key_means_unlocked_but_wrong_type_fails_closed() {
        let absent = CFDictionary::<CFString, CFType>::empty();
        assert_eq!(bool_or_false_if_absent(&absent, LOCKED_KEY), Ok(false));

        let key = CFString::from_str(LOCKED_KEY);
        let present =
            CFDictionary::<CFString, CFBoolean>::from_slices(&[&key], &[CFBoolean::new(true)]);
        // SAFETY: The dictionary contains CFString keys and CFBoolean values;
        // erasing the value generic to their CFType base preserves identity.
        let present =
            unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(present) };
        assert_eq!(bool_or_false_if_absent(&present, LOCKED_KEY), Ok(true));

        let wrong = CFString::from_str("not-a-boolean");
        let malformed = CFDictionary::<CFString, CFString>::from_slices(&[&key], &[&wrong]);
        // SAFETY: Both strings are CFType instances; the erased dictionary is
        // used to verify that the production downcast rejects the wrong type.
        let malformed =
            unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(malformed) };
        assert_eq!(
            bool_or_false_if_absent(&malformed, LOCKED_KEY),
            Err(SessionMonitorError::CheckFailed)
        );
    }

    #[test]
    fn native_session_probe_is_available() {
        assert!(session_probe().is_ok());
    }

    #[test]
    fn session_monitor_starts_and_fences_the_live_probe() {
        let monitor = SessionTransitionMonitor::new();
        assert!(monitor.poll_epoch().is_ok());
    }

    #[test]
    fn retained_probe_history_detects_an_aba_round_trip() {
        let original = probe(false, true);
        let mut previous = Ok(original);
        let mut epoch = 0;
        record_probe(&mut previous, Ok(probe(true, true)), &mut epoch);
        assert_eq!(epoch, 1);
        record_probe(&mut previous, Ok(original), &mut epoch);
        assert_eq!(epoch, 2);
    }

    #[test]
    fn probe_failure_and_recovery_each_invalidate_the_epoch() {
        let mut previous = Ok(probe(false, true));
        let mut epoch = 0;
        record_probe(
            &mut previous,
            Err(SessionMonitorError::CheckFailed),
            &mut epoch,
        );
        assert_eq!(epoch, 1);
        record_probe(&mut previous, Ok(probe(false, true)), &mut epoch);
        assert_eq!(epoch, 2);
    }
}
