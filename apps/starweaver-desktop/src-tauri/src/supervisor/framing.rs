use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    process::{ChildStdin, ChildStdout},
};

use super::{MAX_HOST_FRAME_BYTES, SupervisorError};

pub(super) async fn write_frame(
    stdin: &mut ChildStdin,
    frame: &[u8],
) -> Result<(), SupervisorError> {
    if frame.len() > MAX_HOST_FRAME_BYTES {
        return Err(SupervisorError::transport());
    }
    stdin
        .write_all(frame)
        .await
        .map_err(|_| SupervisorError::transport())?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| SupervisorError::transport())?;
    stdin
        .flush()
        .await
        .map_err(|_| SupervisorError::transport())
}

pub(super) async fn read_frame(
    stdout: &mut BufReader<ChildStdout>,
) -> Result<Vec<u8>, SupervisorError> {
    let mut frame = Vec::new();
    let read = stdout
        .take(u64::try_from(MAX_HOST_FRAME_BYTES).unwrap_or(u64::MAX) + 2)
        .read_until(b'\n', &mut frame)
        .await
        .map_err(|_| SupervisorError::transport())?;
    if read == 0 || frame.len() > MAX_HOST_FRAME_BYTES + 1 || frame.last() != Some(&b'\n') {
        return Err(SupervisorError::transport());
    }
    frame.pop();
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    if frame.is_empty() || frame.len() > MAX_HOST_FRAME_BYTES {
        return Err(SupervisorError::transport());
    }
    Ok(frame)
}
