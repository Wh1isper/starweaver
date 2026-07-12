//! Python wrappers for Starweaver environment providers.

use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyIterator},
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use starweaver_envd::LocalEnvd;
use starweaver_envd_client::{EnvdClientError, EnvdRpcClient};
use starweaver_envd_core::DEFAULT_ENVIRONMENT_ID;
use starweaver_environment::{
    CompositeEnvironmentProvider, DynEnvironmentProvider, EnvdEnvironmentProvider,
    EnvironmentError, EnvironmentLifecycleSnapshot, EnvironmentMount, EnvironmentMountMode,
    EnvironmentPolicy, EnvironmentProvider as EnvironmentProviderTrait, EnvironmentResult,
    EnvironmentState, FileGlobOptions, FileGrepOptions, FileListOptions, FilePolicy, FileStat,
    LocalEnvironmentProvider, ResourceRef, ShellCommand, ShellOutput, ShellPolicy,
    VirtualEnvironmentProvider,
};

use crate::{
    conversion::{py_to_json, serialize_to_py},
    runtime::{PyFutureError, enter_runtime, spawn_py_future},
};

/// Python-visible environment provider handle.
#[pyclass(name = "EnvironmentProvider", skip_from_py_object)]
#[derive(Clone)]
pub struct PyEnvironmentProvider {
    inner: DynEnvironmentProvider,
}

impl PyEnvironmentProvider {
    pub(crate) fn new(inner: DynEnvironmentProvider) -> Self {
        Self { inner }
    }

    pub(crate) fn provider(&self) -> DynEnvironmentProvider {
        self.inner.clone()
    }
}

struct PythonEnvironmentProvider {
    id: String,
    provider: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonEnvironmentProvider {}
unsafe impl Sync for PythonEnvironmentProvider {}

impl PythonEnvironmentProvider {
    async fn call_method<F>(&self, operation: &str, call: F) -> EnvironmentResult<Py<PyAny>>
    where
        F: for<'py> FnOnce(Python<'py>, &Py<PyAny>) -> PyResult<Py<PyAny>> + Send,
    {
        enum CallbackValue {
            Immediate(Py<PyAny>),
            Future(Py<PyAny>),
        }

        let call_result = Python::attach(|py| -> PyResult<CallbackValue> {
            let value = call(py, &self.provider)?;
            let inspect = py.import("inspect")?;
            let is_awaitable = inspect
                .call_method1("isawaitable", (value.bind(py),))?
                .extract::<bool>()?;
            if !is_awaitable {
                return Ok(CallbackValue::Immediate(value));
            }
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (value, self.event_loop.clone_ref(py)),
            )?;
            Ok(CallbackValue::Future(future.unbind()))
        })
        .map_err(|error| py_err_to_environment_error(operation, error))?;

        let future = match call_result {
            CallbackValue::Immediate(value) => return Ok(value),
            CallbackValue::Future(future) => future,
        };
        let guard_future = Python::attach(|py| future.clone_ref(py));
        let mut cancel_guard = PythonFutureCancelGuard::new(guard_future);
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tick.tick().await;
            let poll = Python::attach(|py| -> PyResult<Option<Py<PyAny>>> {
                let done = future.call_method0(py, "done")?.extract::<bool>(py)?;
                if done {
                    Ok(Some(future.call_method0(py, "result")?))
                } else {
                    Ok(None)
                }
            });
            match poll {
                Ok(Some(value)) => break Ok(value),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
        }
        .map_err(|error| py_err_to_environment_error(operation, error));
        cancel_guard.complete();
        result
    }

    async fn call_unit<F>(&self, operation: &str, call: F) -> EnvironmentResult<()>
    where
        F: for<'py> FnOnce(Python<'py>, &Py<PyAny>) -> PyResult<Py<PyAny>> + Send,
    {
        self.call_method(operation, call).await.map(|_| ())
    }

    async fn call_json<T, F>(&self, operation: &str, call: F) -> EnvironmentResult<T>
    where
        T: DeserializeOwned,
        F: for<'py> FnOnce(Python<'py>, &Py<PyAny>) -> PyResult<Py<PyAny>> + Send,
    {
        let value = self.call_method(operation, call).await?;
        Python::attach(|py| -> PyResult<T> {
            let normalized = py
                .import("starweaver.environment")?
                .getattr("_jsonify")?
                .call1((value.bind(py),))?;
            let value = py_to_json(py, &normalized)?;
            serde_json::from_value::<T>(value).map_err(|error| {
                PyValueError::new_err(format!("invalid {operation} result: {error}"))
            })
        })
        .map_err(|error| py_err_to_environment_error(operation, error))
    }
}

struct PythonFutureCancelGuard {
    future: Py<PyAny>,
    completed: bool,
}

impl PythonFutureCancelGuard {
    fn new(future: Py<PyAny>) -> Self {
        Self {
            future,
            completed: false,
        }
    }

    const fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for PythonFutureCancelGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        Python::attach(|py| {
            let _ = self.future.call_method0(py, "cancel");
        });
    }
}

#[async_trait]
impl EnvironmentProviderTrait for PythonEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        let path = path.to_string();
        self.call_json("read_text", move |py, provider| {
            provider.call_method1(py, "read_text", (path,))
        })
        .await
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        let path = path.to_string();
        let value = self
            .call_method("read_bytes", move |py, provider| {
                provider.call_method1(py, "read_bytes", (path, offset, length))
            })
            .await?;
        Python::attach(|py| extract_bytes(value.bind(py)))
            .map_err(|error| py_err_to_environment_error("read_bytes", error))
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        let path = path.to_string();
        let content = content.to_string();
        self.call_unit("write_text", move |py, provider| {
            provider.call_method1(py, "write_text", (path, content))
        })
        .await
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let path = path.to_string();
        self.call_unit("create_dir", move |py, provider| {
            provider.call_method1(py, "create_dir", (path, parents))
        })
        .await
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let path = path.to_string();
        self.call_unit("delete_path", move |py, provider| {
            provider.call_method1(py, "delete_path", (path, recursive))
        })
        .await
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = src.to_string();
        let dst = dst.to_string();
        self.call_unit("move_path", move |py, provider| {
            provider.call_method1(py, "move_path", (src, dst, overwrite))
        })
        .await
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = src.to_string();
        let dst = dst.to_string();
        self.call_unit("copy_path", move |py, provider| {
            provider.call_method1(py, "copy_path", (src, dst, overwrite))
        })
        .await
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let filename = filename.to_string();
        let content = content.to_vec();
        self.call_json("write_tmp_file", move |py, provider| {
            let content = PyBytes::new(py, &content).unbind().into_any();
            provider.call_method1(py, "write_tmp_file", (filename, content))
        })
        .await
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        let path = path.to_string();
        self.call_json("stat", move |py, provider| {
            provider.call_method1(py, "stat", (path,))
        })
        .await
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        let path = path.to_string();
        self.call_json("list", move |py, provider| {
            provider.call_method1(py, "list", (path,))
        })
        .await
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        self.call_json("run_shell", move |py, provider| {
            let command = serialize_to_py(py, &command)?;
            provider.call_method1(py, "run_shell", (command,))
        })
        .await
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        let method_name = Python::attach(|py| -> PyResult<Option<&'static str>> {
            let provider = self.provider.bind(py);
            if provider.hasattr("render_context")? {
                Ok(Some("render_context"))
            } else if provider.hasattr("render_environment_context")? {
                Ok(Some("render_environment_context"))
            } else {
                Ok(None)
            }
        })
        .map_err(|error| py_err_to_environment_error("render_context", error))?;
        let Some(method_name) = method_name else {
            return Ok(None);
        };
        let value = self
            .call_method("render_context", move |py, provider| {
                provider.call_method0(py, method_name)
            })
            .await?;
        Python::attach(|py| value.extract::<Option<String>>(py))
            .map_err(|error| py_err_to_environment_error("render_context", error))
    }

    async fn inspect_lifecycle(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        if !python_provider_has_method(&self.provider, "inspect_lifecycle")? {
            return Ok(EnvironmentLifecycleSnapshot::ready(self.id()));
        }
        self.call_json("inspect_lifecycle", move |py, provider| {
            provider.call_method0(py, "inspect_lifecycle")
        })
        .await
    }

    async fn prepare(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        if !python_provider_has_method(&self.provider, "prepare")? {
            return self.inspect_lifecycle().await;
        }
        self.call_json("prepare", move |py, provider| {
            provider.call_method0(py, "prepare")
        })
        .await
    }

    async fn stop(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        if !python_provider_has_method(&self.provider, "stop")? {
            return Err(EnvironmentError::Unsupported(format!(
                "provider {} does not support explicit stop",
                self.id()
            )));
        }
        self.call_json("stop", move |py, provider| {
            provider.call_method0(py, "stop")
        })
        .await
    }

    async fn cleanup_idle(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        if !python_provider_has_method(&self.provider, "cleanup_idle")? {
            return Err(EnvironmentError::Unsupported(format!(
                "provider {} does not support idle cleanup",
                self.id()
            )));
        }
        self.call_json("cleanup_idle", move |py, provider| {
            provider.call_method0(py, "cleanup_idle")
        })
        .await
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        self.call_json("export_state", move |py, provider| {
            provider.call_method0(py, "export_state")
        })
        .await
    }
}

#[pymethods]
impl PyEnvironmentProvider {
    #[staticmethod]
    #[pyo3(signature = (provider, event_loop, id=None))]
    fn python_provider(
        py: Python<'_>,
        provider: Py<PyAny>,
        event_loop: Py<PyAny>,
        id: Option<String>,
    ) -> PyResult<Self> {
        let id = match id {
            Some(id) => id,
            None => provider.getattr(py, "id")?.extract::<String>(py)?,
        };
        if id.trim().is_empty() {
            return Err(PyValueError::new_err(
                "environment provider id must not be empty",
            ));
        }
        Ok(Self {
            inner: Arc::new(PythonEnvironmentProvider {
                id,
                provider,
                event_loop,
            }),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (id="virtual", files=None, resources=None, shell_outputs=None, tmp_namespace=None))]
    fn virtual_provider(
        py: Python<'_>,
        id: &str,
        files: Option<&Bound<'_, PyAny>>,
        resources: Option<&Bound<'_, PyAny>>,
        shell_outputs: Option<&Bound<'_, PyAny>>,
        tmp_namespace: Option<String>,
    ) -> PyResult<Self> {
        let mut provider = VirtualEnvironmentProvider::new(id);
        if let Some(namespace) = tmp_namespace {
            provider = provider.with_tmp_namespace(namespace);
        }
        if let Some(files) = files {
            let files: BTreeMap<String, String> = parse_json(py, files, "files")?;
            for (path, content) in files {
                provider = provider.with_file(path, content);
            }
        }
        if let Some(resources) = resources {
            for resource in parse_resource_list(py, resources)? {
                provider = provider.with_resource(resource);
            }
        }
        if let Some(shell_outputs) = shell_outputs {
            let outputs = parse_shell_outputs(py, shell_outputs)?;
            for (command, output) in outputs {
                provider = provider.with_shell_output(command, output);
            }
        }
        Ok(Self {
            inner: Arc::new(provider),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (root, id=None, allowed_paths=None, context_file_tree_roots=None, writable=false, allow_shell=false, allowed_programs=None, tmp_namespace=None))]
    #[allow(clippy::too_many_arguments)]
    fn local(
        root: String,
        id: Option<String>,
        allowed_paths: Option<Vec<String>>,
        context_file_tree_roots: Option<Vec<String>>,
        writable: bool,
        allow_shell: bool,
        allowed_programs: Option<Vec<String>>,
        tmp_namespace: Option<String>,
    ) -> Self {
        let file_policy = if writable {
            FilePolicy::read_write()
        } else {
            FilePolicy::read_only()
        };
        let shell_policy = if allow_shell {
            ShellPolicy {
                allow_execute: true,
                allowed_programs: allowed_programs.unwrap_or_default(),
            }
        } else {
            ShellPolicy::default()
        };
        let mut provider =
            LocalEnvironmentProvider::new(PathBuf::from(root)).with_policy(EnvironmentPolicy {
                files: file_policy,
                shell: shell_policy,
            });
        if let Some(id) = id {
            provider = provider.with_id(id);
        }
        if let Some(paths) = allowed_paths {
            provider = provider.with_allowed_paths(paths.into_iter().map(PathBuf::from));
        }
        if let Some(paths) = context_file_tree_roots {
            provider = provider.with_context_file_tree_roots(paths.into_iter().map(PathBuf::from));
        }
        if let Some(namespace) = tmp_namespace {
            provider = provider.with_tmp_namespace(namespace);
        }
        Self {
            inner: Arc::new(provider),
        }
    }

    #[staticmethod]
    #[pyo3(signature = (mount_ids, providers, id=None, modes=None, defaults=None, default_for_shell=None))]
    fn composite(
        mount_ids: Vec<String>,
        providers: &Bound<'_, PyAny>,
        id: Option<String>,
        modes: Option<Vec<String>>,
        defaults: Option<Vec<bool>>,
        default_for_shell: Option<Vec<bool>>,
    ) -> PyResult<Self> {
        let providers = parse_provider_sequence(providers)?;
        let count = mount_ids.len();
        if providers.len() != count {
            return Err(PyValueError::new_err(
                "mount_ids and providers must have the same length",
            ));
        }
        let modes = optional_vec_with_len(modes, count, "modes")?;
        let defaults = optional_vec_with_len(defaults, count, "defaults")?;
        let default_for_shell =
            optional_vec_with_len(default_for_shell, count, "default_for_shell")?;
        let mut mounts = Vec::with_capacity(count);
        for (index, (mount_id, provider)) in mount_ids.into_iter().zip(providers).enumerate() {
            let mode = modes
                .as_ref()
                .and_then(|values| values.get(index))
                .map(|value| parse_mount_mode(value))
                .transpose()?
                .unwrap_or(EnvironmentMountMode::ReadWrite);
            let mount = EnvironmentMount::new(mount_id, provider)
                .map_err(environment_error_to_py_value)?
                .with_mode(mode)
                .with_default(
                    defaults
                        .as_ref()
                        .and_then(|values| values.get(index))
                        .copied()
                        .unwrap_or(false),
                )
                .with_default_for_shell(
                    default_for_shell
                        .as_ref()
                        .and_then(|values| values.get(index))
                        .copied()
                        .unwrap_or(false),
                );
            mounts.push(mount);
        }
        let provider = match id {
            Some(id) => CompositeEnvironmentProvider::with_id(id, mounts),
            None => CompositeEnvironmentProvider::new(mounts),
        }
        .map_err(environment_error_to_py_value)?;
        Ok(Self {
            inner: Arc::new(provider),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (environment, environment_id=None, id=None))]
    fn envd_local(
        py: Python<'_>,
        environment: &Bound<'_, PyAny>,
        environment_id: Option<String>,
        id: Option<String>,
    ) -> PyResult<Self> {
        let provider = extract_environment_provider(py, Some(environment))?
            .ok_or_else(|| PyValueError::new_err("environment must be an EnvironmentProvider"))?;
        let environment_id = environment_id.unwrap_or_else(|| DEFAULT_ENVIRONMENT_ID.to_string());
        let service =
            Arc::new(LocalEnvd::new(provider.clone()).with_environment_id(environment_id.clone()));
        let provider = build_envd_provider(
            service,
            environment_id,
            id,
            Some(provider.shell_review_context()),
        );
        Ok(Self {
            inner: Arc::new(provider),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (endpoint, environment_id=None, token=None, id=None))]
    fn envd_http(
        endpoint: String,
        environment_id: Option<String>,
        token: Option<String>,
        id: Option<String>,
    ) -> PyResult<Self> {
        let client = match token {
            Some(token) => EnvdRpcClient::http_with_token(endpoint, token),
            None => EnvdRpcClient::http(endpoint),
        }
        .map_err(envd_client_error_to_py_value)?;
        let environment_id = environment_id.unwrap_or_else(|| DEFAULT_ENVIRONMENT_ID.to_string());
        let provider = build_envd_provider(Arc::new(client), environment_id, id, None);
        Ok(Self {
            inner: Arc::new(provider),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (program, args=None, environment_id=None, id=None))]
    fn envd_stdio(
        program: String,
        args: Option<Vec<String>>,
        environment_id: Option<String>,
        id: Option<String>,
    ) -> PyResult<Self> {
        let args = args
            .unwrap_or_default()
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        let client = enter_runtime(|| {
            EnvdRpcClient::spawn_stdio(PathBuf::from(program), args)
                .map_err(envd_client_error_to_py_value)
        })?;
        let environment_id = environment_id.unwrap_or_else(|| DEFAULT_ENVIRONMENT_ID.to_string());
        let provider = build_envd_provider(Arc::new(client), environment_id, id, None);
        Ok(Self {
            inner: Arc::new(provider),
        })
    }

    #[getter]
    fn id(&self) -> String {
        self.inner.id().to_string()
    }

    fn read_text(&self, py: Python<'_>, path: String) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .read_text(&path)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, text| Ok(text.into_pyobject(py)?.unbind().into_any()),
        )
    }

    fn read_bytes(
        &self,
        py: Python<'_>,
        path: String,
        offset: usize,
        length: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .read_bytes(&path, offset, length)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, bytes| Ok(PyBytes::new(py, &bytes).unbind().into_any()),
        )
    }

    fn write_text(&self, py: Python<'_>, path: String, content: String) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .write_text(&path, &content)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn write_tmp_file(
        &self,
        py: Python<'_>,
        filename: String,
        content: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        let content = extract_bytes(content)?;
        spawn_py_future(
            py,
            async move {
                provider
                    .write_tmp_file(&filename, &content)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, path| Ok(path.into_pyobject(py)?.unbind().into_any()),
        )
    }

    fn create_dir(&self, py: Python<'_>, path: String, parents: bool) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .create_dir(&path, parents)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn delete_path(&self, py: Python<'_>, path: String, recursive: bool) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .delete_path(&path, recursive)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn move_path(
        &self,
        py: Python<'_>,
        src: String,
        dst: String,
        overwrite: bool,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .move_path(&src, &dst, overwrite)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn copy_path(
        &self,
        py: Python<'_>,
        src: String,
        dst: String,
        overwrite: bool,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .copy_path(&src, &dst, overwrite)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn list(&self, py: Python<'_>, path: String) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move { provider.list(&path).await.map_err(environment_error_to_py) },
            |py, entries| serialize_to_py(py, &entries),
        )
    }

    #[pyo3(signature = (path, max_entries=0, ignore_patterns=None))]
    fn list_with_options(
        &self,
        py: Python<'_>,
        path: String,
        max_entries: usize,
        ignore_patterns: Option<Vec<String>>,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        let options = FileListOptions {
            ignore_patterns: ignore_patterns.unwrap_or_default(),
            max_entries,
        };
        spawn_py_future(
            py,
            async move {
                provider
                    .list_with_options(&path, options)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, result| serialize_to_py(py, &result),
        )
    }

    fn stat(&self, py: Python<'_>, path: String) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move { provider.stat(&path).await.map_err(environment_error_to_py) },
            |py, stat| serialize_to_py(py, &stat),
        )
    }

    #[pyo3(signature = (path, pattern, include_hidden=false, include_ignored=false, max_results=500))]
    fn glob(
        &self,
        py: Python<'_>,
        path: String,
        pattern: String,
        include_hidden: bool,
        include_ignored: bool,
        max_results: usize,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        let options = FileGlobOptions {
            include_hidden,
            include_ignored,
            max_results,
        };
        spawn_py_future(
            py,
            async move {
                provider
                    .glob(&path, &pattern, options)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, matches| serialize_to_py(py, &matches),
        )
    }

    #[pyo3(signature = (path, pattern, include=None, context_lines=0, max_results=100, max_matches_per_file=20, max_files=50, include_hidden=false, include_ignored=false))]
    #[allow(clippy::too_many_arguments)]
    fn grep(
        &self,
        py: Python<'_>,
        path: String,
        pattern: String,
        include: Option<String>,
        context_lines: usize,
        max_results: usize,
        max_matches_per_file: usize,
        max_files: usize,
        include_hidden: bool,
        include_ignored: bool,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        let options = FileGrepOptions {
            include,
            context_lines,
            max_results,
            max_matches_per_file,
            max_files,
            include_hidden,
            include_ignored,
        };
        spawn_py_future(
            py,
            async move {
                provider
                    .grep(&path, &pattern, options)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, matches| serialize_to_py(py, &matches),
        )
    }

    #[pyo3(signature = (command, timeout_seconds=None, cwd=None, environment=None))]
    fn run_shell(
        &self,
        py: Python<'_>,
        command: String,
        timeout_seconds: Option<u64>,
        cwd: Option<String>,
        environment: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        let environment = match environment {
            Some(value) => parse_json(py, value, "environment")?,
            None => BTreeMap::new(),
        };
        let command = ShellCommand {
            timeout_seconds,
            cwd,
            environment,
            ..ShellCommand::shell(command)
        };
        spawn_py_future(
            py,
            async move {
                provider
                    .run_shell(command)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, output| serialize_to_py(py, &output),
        )
    }

    fn export_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .export_state()
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, state| serialize_to_py(py, &state),
        )
    }

    fn render_context(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .render_environment_context()
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, context| serialize_to_py(py, &context),
        )
    }

    fn inspect_lifecycle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .inspect_lifecycle()
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn prepare(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move { provider.prepare().await.map_err(environment_error_to_py) },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn stop(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move { provider.stop().await.map_err(environment_error_to_py) },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn cleanup_idle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                provider
                    .cleanup_idle()
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    #[pyo3(signature = (command, timeout_seconds=None, cwd=None, environment=None))]
    fn start_process(
        &self,
        py: Python<'_>,
        command: String,
        timeout_seconds: Option<u64>,
        cwd: Option<String>,
        environment: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let provider = process_provider(&self.inner)?;
        let environment = match environment {
            Some(value) => parse_json(py, value, "environment")?,
            None => BTreeMap::new(),
        };
        let command = ShellCommand {
            timeout_seconds,
            cwd,
            environment,
            ..ShellCommand::shell(command)
        };
        spawn_py_future(
            py,
            async move {
                provider
                    .start_process(command)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    #[pyo3(signature = (process_id, timeout_seconds=0))]
    fn wait_process(
        &self,
        py: Python<'_>,
        process_id: String,
        timeout_seconds: u64,
    ) -> PyResult<Py<PyAny>> {
        let provider = process_provider(&self.inner)?;
        spawn_py_future(
            py,
            async move {
                provider
                    .wait_process(&process_id, timeout_seconds)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn list_processes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let provider = process_provider(&self.inner)?;
        spawn_py_future(
            py,
            async move {
                provider
                    .list_processes()
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshots| serialize_to_py(py, &snapshots),
        )
    }

    #[pyo3(signature = (process_id, text, close_stdin=false))]
    fn input_process(
        &self,
        py: Python<'_>,
        process_id: String,
        text: String,
        close_stdin: bool,
    ) -> PyResult<Py<PyAny>> {
        let provider = process_provider(&self.inner)?;
        spawn_py_future(
            py,
            async move {
                provider
                    .input_process(&process_id, &text, close_stdin)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn signal_process(
        &self,
        py: Python<'_>,
        process_id: String,
        signal: i32,
    ) -> PyResult<Py<PyAny>> {
        let provider = process_provider(&self.inner)?;
        spawn_py_future(
            py,
            async move {
                provider
                    .signal_process(&process_id, signal)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn kill_process(&self, py: Python<'_>, process_id: String) -> PyResult<Py<PyAny>> {
        let provider = process_provider(&self.inner)?;
        spawn_py_future(
            py,
            async move {
                provider
                    .kill_process(&process_id)
                    .await
                    .map_err(environment_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }
}

pub(crate) fn extract_environment_provider(
    _py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<DynEnvironmentProvider>> {
    match value {
        Some(value) if !value.is_none() => {
            if let Ok(provider) = value.extract::<PyRef<'_, PyEnvironmentProvider>>() {
                Ok(Some(provider.provider()))
            } else if let Ok(to_native) = value.getattr("to_native") {
                let native = to_native.call0()?;
                let provider = native.extract::<PyRef<'_, PyEnvironmentProvider>>()?;
                Ok(Some(provider.provider()))
            } else {
                Err(PyValueError::new_err(
                    "environment must be an EnvironmentProvider",
                ))
            }
        }
        Some(_) | None => Ok(None),
    }
}

fn parse_json<T>(py: Python<'_>, value: &Bound<'_, PyAny>, name: &str) -> PyResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(py_to_json(py, value)?)
        .map_err(|error| PyValueError::new_err(format!("invalid {name}: {error}")))
}

fn parse_resource_list(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<ResourceRef>> {
    let value = py_to_json(py, value)?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<ResourceRef>(normalize_resource_ref_json(item)?)
                    .map_err(|error| PyValueError::new_err(format!("invalid resource: {error}")))
            })
            .collect(),
        Value::Object(_) => {
            serde_json::from_value::<ResourceRef>(normalize_resource_ref_json(value)?)
                .map(|resource| vec![resource])
                .map_err(|error| PyValueError::new_err(format!("invalid resource: {error}")))
        }
        _ => Err(PyValueError::new_err(
            "resources must be a resource mapping or a list of resource mappings",
        )),
    }
}

fn normalize_resource_ref_json(value: Value) -> PyResult<Value> {
    let Value::Object(mut object) = value else {
        return Err(PyValueError::new_err("resource must be a mapping"));
    };
    if !object.contains_key("id")
        && let Some(Value::String(uri)) = object.get("uri")
    {
        object.insert("id".to_string(), Value::String(uri.clone()));
    }
    object
        .entry("metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    Ok(Value::Object(object))
}

fn parse_shell_outputs(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<BTreeMap<String, ShellOutput>> {
    let Value::Object(object) = py_to_json(py, value)? else {
        return Err(PyValueError::new_err("shell_outputs must be a mapping"));
    };
    object
        .into_iter()
        .map(|(command, value)| {
            let output = match value {
                Value::String(stdout) => ShellOutput {
                    status: 0,
                    stdout,
                    stderr: String::new(),
                    metadata: Map::new(),
                },
                Value::Object(_) => serde_json::from_value(value).map_err(|error| {
                    PyValueError::new_err(format!("invalid shell output for {command}: {error}"))
                })?,
                _ => {
                    return Err(PyValueError::new_err(
                        "shell output values must be strings or mappings",
                    ));
                }
            };
            Ok((command, output))
        })
        .collect()
}

fn extract_bytes(value: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = value.extract::<Vec<u8>>() {
        return Ok(bytes);
    }
    if let Ok(text) = value.extract::<String>() {
        return Ok(text.into_bytes());
    }
    Err(PyValueError::new_err("content must be bytes or string"))
}

fn python_provider_has_method(provider: &Py<PyAny>, method: &str) -> EnvironmentResult<bool> {
    Python::attach(|py| provider.bind(py).hasattr(method))
        .map_err(|error| py_err_to_environment_error(method, error))
}

fn process_provider(
    provider: &DynEnvironmentProvider,
) -> PyResult<starweaver_environment::DynProcessShellProvider> {
    provider
        .clone()
        .process_shell_provider()
        .ok_or_else(|| PyValueError::new_err("environment does not support background processes"))
}

fn parse_provider_sequence(value: &Bound<'_, PyAny>) -> PyResult<Vec<DynEnvironmentProvider>> {
    let iterator = PyIterator::from_object(value)?;
    iterator
        .map(|item| {
            let item = item?;
            if let Ok(provider) = item.extract::<PyRef<'_, PyEnvironmentProvider>>() {
                Ok(provider.provider())
            } else if let Ok(to_native) = item.getattr("to_native") {
                let native = to_native.call0()?;
                let provider = native.extract::<PyRef<'_, PyEnvironmentProvider>>()?;
                Ok(provider.provider())
            } else {
                Err(PyValueError::new_err(
                    "composite providers must be EnvironmentProvider instances",
                ))
            }
        })
        .collect()
}

fn optional_vec_with_len<T>(
    values: Option<Vec<T>>,
    expected_len: usize,
    name: &str,
) -> PyResult<Option<Vec<T>>> {
    if let Some(values) = &values
        && values.len() != expected_len
    {
        return Err(PyValueError::new_err(format!(
            "{name} must have the same length as mount_ids"
        )));
    }
    Ok(values)
}

fn parse_mount_mode(value: &str) -> PyResult<EnvironmentMountMode> {
    match value {
        "read_write" | "read-write" | "rw" => Ok(EnvironmentMountMode::ReadWrite),
        "read_only" | "read-only" | "ro" => Ok(EnvironmentMountMode::ReadOnly),
        other => Err(PyValueError::new_err(format!(
            "unsupported mount mode: {other}"
        ))),
    }
}

fn build_envd_provider(
    service: Arc<dyn starweaver_envd_core::EnvdService>,
    environment_id: String,
    id: Option<String>,
    shell_review_context: Option<starweaver_environment::ShellReviewEnvironmentContext>,
) -> EnvdEnvironmentProvider {
    let mut provider = EnvdEnvironmentProvider::new(service, environment_id);
    if let Some(context) = shell_review_context {
        provider = provider.with_shell_review_context(context);
    }
    if let Some(id) = id {
        provider = provider.with_id(id);
    }
    provider
}

fn envd_client_error_to_py_value(error: EnvdClientError) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn environment_error_to_py_value(error: starweaver_environment::EnvironmentError) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn environment_error_to_py(error: starweaver_environment::EnvironmentError) -> PyFutureError {
    match error {
        EnvironmentError::Unsupported(message) => PyFutureError::StateWithCode {
            code: "unsupported",
            message: format!("unsupported environment operation: {message}"),
        },
        other => PyFutureError::State(other.to_string()),
    }
}

fn py_err_to_environment_error(operation: &str, error: PyErr) -> EnvironmentError {
    let message = error.to_string();
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("not found")
        || lowered.contains("no such file")
        || lowered.contains("filenotfounderror")
    {
        EnvironmentError::NotFound(message)
    } else if lowered.contains("denied") || lowered.contains("permission") {
        EnvironmentError::AccessDenied(message)
    } else if lowered.contains("unsupported") || lowered.contains("notimplemented") {
        EnvironmentError::Unsupported(message)
    } else if lowered.contains("invalid") {
        EnvironmentError::InvalidRequest(message)
    } else {
        EnvironmentError::Provider(format!("{operation} failed: {message}"))
    }
}
