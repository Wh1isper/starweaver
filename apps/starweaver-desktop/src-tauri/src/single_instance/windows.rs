use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use interprocess::{
    local_socket::{GenericNamespaced, Listener, ListenerOptions, Stream, prelude::*},
    os::windows::{local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor},
};
use sysinfo::{Pid, ProcessesToUpdate, System, Uid};
use tauri::{
    Manager, Runtime,
    plugin::{self, TauriPlugin},
};
use uuid::{Uuid, Version};

use super::ActivationCallback;

const ACTIVATION_FRAME: &[u8] = b"SWA1\n";
const ACTIVATION_ACK: &[u8] = b"OK\n";
const ENDPOINT_PREFIX: &str = "io.github.wh1isper.starweaver.activation-v1";
const ENDPOINT_SECURITY: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;BA)";
const LOCK_NAME: &str = "primary-v1.lock";
const RENDEZVOUS_NAME: &str = "activation-v1.nonce";
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

struct PrimaryLock {
    _file: File,
}

pub fn init<R: Runtime>(callback: Box<ActivationCallback<R>>) -> TauriPlugin<R> {
    plugin::Builder::new("starweaver-single-instance")
        .setup(move |app, _api| {
            let current_user = process_user_id(std::process::id())?;
            let private_directory = app.path().app_local_data_dir()?;
            fs::create_dir_all(&private_directory)?;
            let lock = open_lock_file(&private_directory.join(LOCK_NAME))?;
            match lock.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    let endpoint_name = read_rendezvous(&private_directory)?;
                    notify_primary_bounded(endpoint_name, current_user)?;
                    app.cleanup_before_exit();
                    std::process::exit(0);
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }

            let endpoint_name = publish_rendezvous(&private_directory)?;
            let endpoint = endpoint_name.to_ns_name::<GenericNamespaced>()?;
            let security = widestring::U16CString::from_str(ENDPOINT_SECURITY)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            let security = SecurityDescriptor::deserialize(security.as_ucstr())?;
            let listener = ListenerOptions::new()
                .name(endpoint)
                .security_descriptor(security)
                .create_sync()?;
            let app_handle = app.clone();
            let callback: Arc<ActivationCallback<R>> = Arc::from(callback);
            thread::Builder::new()
                .name("starweaver-single-instance".to_string())
                .spawn(move || {
                    listen_for_activations(listener, app_handle, callback, current_user);
                })?;
            app.manage(PrimaryLock { _file: lock });
            Ok(())
        })
        .build()
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn publish_rendezvous(directory: &Path) -> io::Result<String> {
    let nonce = Uuid::new_v4();
    let temporary = directory.join(format!(".{RENDEZVOUS_NAME}-{nonce}.tmp"));
    let destination = directory.join(RENDEZVOUS_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    writeln!(file, "{nonce}")?;
    file.sync_all()?;
    drop(file);
    match fs::remove_file(&destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(format!("{ENDPOINT_PREFIX}-{}", nonce.simple()))
}

fn read_rendezvous(directory: &Path) -> io::Result<String> {
    let path = directory.join(RENDEZVOUS_NAME);
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match fs::read_to_string(&path).and_then(|value| parse_rendezvous(&value)) {
            Ok(endpoint) => return Ok(endpoint),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn parse_rendezvous(value: &str) -> io::Result<String> {
    let nonce = Uuid::parse_str(value.trim()).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid activation rendezvous: {error}"),
        )
    })?;
    if nonce.get_version() != Some(Version::Random) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "activation rendezvous must contain a random UUID",
        ));
    }
    Ok(format!("{ENDPOINT_PREFIX}-{}", nonce.simple()))
}

fn process_user_id(pid: u32) -> io::Result<Uid> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system
        .process(pid)
        .and_then(|process| process.user_id())
        .cloned()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "could not resolve the process user identity",
            )
        })
}

fn peer_is_current_user(stream: &Stream, current_user: &Uid) -> io::Result<bool> {
    let peer_pid = stream.peer_creds()?.pid().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local activation peer did not expose a process identity",
        )
    })?;
    Ok(process_user_id(peer_pid).is_ok_and(|peer_user| peer_user == *current_user))
}

fn notify_primary_bounded(endpoint_name: String, current_user: Uid) -> io::Result<()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("starweaver-activation-connect".to_string())
        .spawn(move || {
            let _ = sender.send(notify_primary_with_retry(&endpoint_name, &current_user));
        })?;
    receiver.recv_timeout(STARTUP_TIMEOUT).map_err(|error| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out connecting to the primary desktop: {error}"),
        )
    })?
}

fn notify_primary_with_retry(endpoint_name: &str, current_user: &Uid) -> io::Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let endpoint = endpoint_name.to_ns_name::<GenericNamespaced>()?;
        match notify_primary(endpoint, current_user) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::WouldBlock
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn notify_primary(
    endpoint: interprocess::local_socket::Name<'_>,
    current_user: &Uid,
) -> io::Result<()> {
    let mut stream = Stream::connect(endpoint)?;
    if !peer_is_current_user(&stream, current_user)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local activation server belongs to a different user",
        ));
    }
    stream.set_nonblocking(true)?;
    write_all_until(&mut stream, ACTIVATION_FRAME, Instant::now() + IO_TIMEOUT)?;
    let mut acknowledgement = [0; ACTIVATION_ACK.len()];
    read_exact_until(
        &mut stream,
        &mut acknowledgement,
        Instant::now() + IO_TIMEOUT,
    )?;
    if acknowledgement != ACTIVATION_ACK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "primary desktop returned an invalid activation acknowledgement",
        ));
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn listen_for_activations<R: Runtime>(
    listener: Listener,
    app: tauri::AppHandle<R>,
    callback: Arc<ActivationCallback<R>>,
    current_user: Uid,
) {
    for connection in listener.incoming() {
        let Ok(stream) = connection else {
            continue;
        };
        let app = app.clone();
        let callback = Arc::clone(&callback);
        let current_user = current_user.clone();
        let _ = thread::Builder::new()
            .name("starweaver-activation-client".to_string())
            .spawn(move || {
                handle_activation(stream, &app, callback.as_ref(), &current_user);
            });
    }
}

fn handle_activation<R: Runtime>(
    mut stream: Stream,
    app: &tauri::AppHandle<R>,
    callback: &ActivationCallback<R>,
    current_user: &Uid,
) {
    if !peer_is_current_user(&stream, current_user).unwrap_or(false)
        || stream.set_nonblocking(true).is_err()
    {
        return;
    }
    let mut frame = [0; ACTIVATION_FRAME.len()];
    if read_exact_until(&mut stream, &mut frame, Instant::now() + IO_TIMEOUT).is_ok()
        && frame == ACTIVATION_FRAME
    {
        callback(app);
        let _ = write_all_until(&mut stream, ACTIVATION_ACK, Instant::now() + IO_TIMEOUT);
    }
}

fn read_exact_until(stream: &mut Stream, buffer: &mut [u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Err(io::ErrorKind::TimedOut.into());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_all_until(stream: &mut Stream, buffer: &[u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        match stream.write(&buffer[offset..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => offset += written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Err(io::ErrorKind::TimedOut.into());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_protocol_contains_no_process_context() {
        assert_eq!(ACTIVATION_FRAME, b"SWA1\n");
        assert_eq!(ACTIVATION_ACK, b"OK\n");
        assert_eq!(ENDPOINT_SECURITY, "D:P(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;BA)");
    }

    #[test]
    fn advisory_lock_elects_only_one_primary() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!(
            "starweaver-desktop-windows-lock-test-{}",
            std::process::id()
        ));
        let first = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let second = OpenOptions::new().read(true).write(true).open(&path)?;
        first.try_lock()?;
        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
        drop(first);
        second.try_lock()?;
        drop(second);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn rendezvous_requires_a_random_uuid() {
        let nonce = Uuid::new_v4();
        assert_eq!(
            parse_rendezvous(&nonce.to_string()).ok(),
            Some(format!("{ENDPOINT_PREFIX}-{}", nonce.simple()))
        );
        assert!(parse_rendezvous("00000000-0000-0000-0000-000000000000").is_err());
        assert!(parse_rendezvous("not-a-uuid").is_err());
    }
}
