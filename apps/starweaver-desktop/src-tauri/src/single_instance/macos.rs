use std::{
    fs::{self, DirBuilder, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use tauri::{
    Manager, RunEvent, Runtime,
    plugin::{self, TauriPlugin},
};

use super::ActivationCallback;

const ACTIVATION_FRAME: &[u8] = b"SWA1\n";
const ACTIVATION_ACK: &[u8] = b"OK\n";
const SOCKET_NAME: &str = "activation-v1.sock";
const LOCK_NAME: &str = "primary-v1.lock";
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

struct PrimaryLock {
    _file: File,
}
struct SocketPath(PathBuf);

pub fn init<R: Runtime>(callback: Box<ActivationCallback<R>>) -> TauriPlugin<R> {
    plugin::Builder::new("starweaver-single-instance")
        .setup(move |app, _api| {
            let directory = private_runtime_directory()?;
            let socket_path = directory.join(SOCKET_NAME);
            let lock = open_lock_file(&directory.join(LOCK_NAME))?;
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            loop {
                match lock.try_lock() {
                    Ok(()) => break,
                    Err(TryLockError::WouldBlock) => match notify_primary(&socket_path) {
                        Ok(()) => {
                            app.cleanup_before_exit();
                            std::process::exit(0);
                        }
                        Err(error)
                            if transient_startup_error(&error) && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(50));
                        }
                        Err(error) => return Err(error.into()),
                    },
                    Err(TryLockError::Error(error)) => return Err(error.into()),
                }
            }

            let _ = fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path)?;
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
            let app_handle = app.clone();
            thread::Builder::new()
                .name("starweaver-single-instance".to_string())
                .spawn(move || listen_for_activations(listener, app_handle, callback))?;
            app.manage(PrimaryLock { _file: lock });
            app.manage(SocketPath(socket_path));
            Ok(())
        })
        .on_event(|app, event| {
            if matches!(event, RunEvent::Exit)
                && let Some(path) = app.try_state::<SocketPath>()
            {
                let _ = fs::remove_file(&path.0);
            }
        })
        .build()
}

fn private_runtime_directory() -> io::Result<PathBuf> {
    let effective_uid = nix::unistd::geteuid();
    let directory =
        std::env::temp_dir().join(format!("starweaver-desktop-{}", effective_uid.as_raw()));
    match DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid.as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "single-instance directory is not private to the current user",
        ));
    }
    Ok(directory)
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "single-instance lock must not be a symbolic link",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "single-instance lock is not private to the current user",
        ));
    }
    Ok(file)
}

fn transient_startup_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
    )
}

fn notify_primary(socket_path: &Path) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(ACTIVATION_FRAME)?;
    stream.shutdown(Shutdown::Write)?;
    let mut acknowledgement = [0; ACTIVATION_ACK.len()];
    stream.read_exact(&mut acknowledgement)?;
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
    listener: UnixListener,
    app: tauri::AppHandle<R>,
    callback: Box<ActivationCallback<R>>,
) {
    for connection in listener.incoming() {
        let Ok(mut stream) = connection else {
            continue;
        };
        if receive_activation(&mut stream).unwrap_or(false) {
            callback(&app);
            let _ = stream.write_all(ACTIVATION_ACK);
        }
    }
}

fn receive_activation(stream: &mut UnixStream) -> io::Result<bool> {
    if !peer_is_current_user(stream) {
        return Ok(false);
    }
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut frame = [0; ACTIVATION_FRAME.len()];
    stream.read_exact(&mut frame)?;
    Ok(frame == ACTIVATION_FRAME)
}

fn peer_is_current_user(stream: &UnixStream) -> bool {
    nix::unistd::getpeereid(stream).is_ok_and(|(uid, _gid)| uid == nix::unistd::geteuid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_protocol_accepts_only_the_fixed_current_user_frame()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut receiver, mut sender) = UnixStream::pair()?;
        sender.write_all(ACTIVATION_FRAME)?;
        sender.shutdown(Shutdown::Write)?;
        assert!(receive_activation(&mut receiver)?);
        receiver.write_all(ACTIVATION_ACK)?;
        let mut acknowledgement = [0; ACTIVATION_ACK.len()];
        sender.read_exact(&mut acknowledgement)?;
        assert_eq!(acknowledgement, ACTIVATION_ACK);

        let (mut receiver, mut sender) = UnixStream::pair()?;
        sender.write_all(b"private-process-context")?;
        sender.shutdown(Shutdown::Write)?;
        assert!(!receive_activation(&mut receiver)?);
        Ok(())
    }

    #[test]
    fn advisory_lock_elects_only_one_primary() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!(
            "starweaver-desktop-lock-test-{}",
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
}
