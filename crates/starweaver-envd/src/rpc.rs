//! `EnvD` JSON-RPC method dispatch.

use std::{future::Future, sync::Arc};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use starweaver_envd_core::{
    CommandRunRequest, EnvdRpcError, EnvdService, EnvironmentContextRequest, EnvironmentRequest,
    FileCopyRequest, FileCreateDirRequest, FileDeleteRequest, FileGlobRequest, FileGrepRequest,
    FileListRequest, FileMoveRequest, FileReadRequest, FileStatRequest, FileWriteRequest,
    FileWriteTmpRequest, INVALID_PARAMS, InitializeEnvdRequest, METHOD_NOT_FOUND,
    OpenEnvironmentRequest, ProcessInputRequest, ProcessKillRequest, ProcessSignalRequest,
    ProcessStartRequest, ProcessWaitRequest, ShellReviewContextRequest, error_response,
    parse_json_rpc_text, success_response,
};

/// JSON-RPC service facade over an envd service implementation.
#[derive(Clone)]
pub struct EnvdRpcService {
    service: Arc<dyn EnvdService>,
}

impl EnvdRpcService {
    /// Create a JSON-RPC facade.
    #[must_use]
    pub const fn new(service: Arc<dyn EnvdService>) -> Self {
        Self { service }
    }

    /// Handle one JSON-RPC text frame.
    pub async fn handle_text(&self, text: &str) -> (Option<Value>, bool) {
        let request = match parse_json_rpc_text(text) {
            Ok(request) => request,
            Err(response) => return (Some(response), false),
        };
        let id = request.id.clone();
        let result = self.dispatch(&request.method, &request.params).await;
        let shutdown = request.method == "shutdown" && result.is_ok();
        let Some(id) = id else {
            return (None, shutdown);
        };
        let response = match result {
            Ok(result) => success_response(&id, &result),
            Err(error) => error_response(&id, error.code, &error.message),
        };
        (Some(response), shutdown)
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, EnvdRpcError> {
        match method {
            "initialize" => {
                self.call(params, |request| self.service.initialize(request))
                    .await
            }
            "shutdown" => Ok(json!({"status": "shutdown"})),
            "environment.open" => {
                self.call(params, |request| self.service.open_environment(request))
                    .await
            }
            "environment.state" => {
                self.call(params, |request| self.service.environment_state(request))
                    .await
            }
            "environment.prepare" => {
                self.call(params, |request| self.service.prepare_environment(request))
                    .await
            }
            "environment.stop" => {
                self.call(params, |request| self.service.stop_environment(request))
                    .await
            }
            "environment.cleanup_idle" => {
                self.call(params, |request| self.service.cleanup_idle(request))
                    .await
            }
            "file.read" => {
                self.call(params, |request| self.service.file_read(request))
                    .await
            }
            "file.write" => {
                self.call(params, |request| self.service.file_write(request))
                    .await
            }
            "file.create_dir" => {
                self.call(params, |request| self.service.file_create_dir(request))
                    .await
            }
            "file.delete" => {
                self.call(params, |request| self.service.file_delete(request))
                    .await
            }
            "file.move" => {
                self.call(params, |request| self.service.file_move(request))
                    .await
            }
            "file.copy" => {
                self.call(params, |request| self.service.file_copy(request))
                    .await
            }
            "file.write_tmp" => {
                self.call(params, |request| self.service.file_write_tmp(request))
                    .await
            }
            "file.stat" => {
                self.call(params, |request| self.service.file_stat(request))
                    .await
            }
            "file.list" => {
                self.call(params, |request| self.service.file_list(request))
                    .await
            }
            "file.glob" => {
                self.call(params, |request| self.service.file_glob(request))
                    .await
            }
            "file.grep" => {
                self.call(params, |request| self.service.file_grep(request))
                    .await
            }
            "command.run" => {
                self.call(params, |request| self.service.command_run(request))
                    .await
            }
            "process.start" => {
                self.call(params, |request| self.service.process_start(request))
                    .await
            }
            "process.wait" => {
                self.call(params, |request| self.service.process_wait(request))
                    .await
            }
            "process.list" => {
                self.call(params, |request| self.service.process_list(request))
                    .await
            }
            "process.input" => {
                self.call(params, |request| self.service.process_input(request))
                    .await
            }
            "process.signal" => {
                self.call(params, |request| self.service.process_signal(request))
                    .await
            }
            "process.kill" => {
                self.call(params, |request| self.service.process_kill(request))
                    .await
            }
            "context.render" => {
                self.call(params, |request| {
                    self.service.render_environment_context(request)
                })
                .await
            }
            "shell.review_context" => {
                self.call(params, |request| self.service.shell_review_context(request))
                    .await
            }
            "snapshot.export" => {
                self.call(params, |request| self.service.export_snapshot(request))
                    .await
            }
            other => Err(EnvdRpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    async fn call<Request, Response, Fut, Handler>(
        &self,
        params: &Value,
        handler: Handler,
    ) -> Result<Value, EnvdRpcError>
    where
        Request: DeserializeOwned,
        Response: Serialize,
        Fut: Future<Output = starweaver_envd_core::EnvdResult<Response>>,
        Handler: FnOnce(Request) -> Fut,
    {
        let request = decode_params::<Request>(params)?;
        let result = handler(request).await.map_err(EnvdRpcError::from)?;
        serde_json::to_value(result).map_err(|error| {
            EnvdRpcError::new(starweaver_envd_core::SERVER_ERROR, error.to_string())
        })
    }
}

fn decode_params<Request>(params: &Value) -> Result<Request, EnvdRpcError>
where
    Request: DeserializeOwned,
{
    let value = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    serde_json::from_value(value)
        .map_err(|error| EnvdRpcError::new(INVALID_PARAMS, format!("invalid params: {error}")))
}

#[allow(dead_code)]
const fn _method_type_check() {
    let _ = std::any::TypeId::of::<InitializeEnvdRequest>();
    let _ = std::any::TypeId::of::<OpenEnvironmentRequest>();
    let _ = std::any::TypeId::of::<EnvironmentRequest>();
    let _ = std::any::TypeId::of::<FileReadRequest>();
    let _ = std::any::TypeId::of::<FileWriteRequest>();
    let _ = std::any::TypeId::of::<FileCreateDirRequest>();
    let _ = std::any::TypeId::of::<FileDeleteRequest>();
    let _ = std::any::TypeId::of::<FileMoveRequest>();
    let _ = std::any::TypeId::of::<FileCopyRequest>();
    let _ = std::any::TypeId::of::<FileWriteTmpRequest>();
    let _ = std::any::TypeId::of::<FileStatRequest>();
    let _ = std::any::TypeId::of::<FileListRequest>();
    let _ = std::any::TypeId::of::<FileGlobRequest>();
    let _ = std::any::TypeId::of::<FileGrepRequest>();
    let _ = std::any::TypeId::of::<CommandRunRequest>();
    let _ = std::any::TypeId::of::<ProcessStartRequest>();
    let _ = std::any::TypeId::of::<ProcessWaitRequest>();
    let _ = std::any::TypeId::of::<ProcessInputRequest>();
    let _ = std::any::TypeId::of::<ProcessSignalRequest>();
    let _ = std::any::TypeId::of::<ProcessKillRequest>();
    let _ = std::any::TypeId::of::<EnvironmentContextRequest>();
    let _ = std::any::TypeId::of::<ShellReviewContextRequest>();
}
