//! Python wrappers for Starweaver environment providers.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use pyo3::{exceptions::PyValueError, prelude::*, types::PyBytes};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use starweaver_environment::{
    DynEnvironmentProvider, EnvironmentPolicy, FileGlobOptions, FileGrepOptions, FileListOptions,
    FilePolicy, LocalEnvironmentProvider, ResourceRef, ShellCommand, ShellOutput, ShellPolicy,
    VirtualEnvironmentProvider,
};

use crate::{
    conversion::{py_to_json, serialize_to_py},
    runtime::{PyFutureError, spawn_py_future},
};

/// Python-visible environment provider handle.
#[pyclass(name = "EnvironmentProvider", skip_from_py_object)]
#[derive(Clone)]
pub struct PyEnvironmentProvider {
    inner: DynEnvironmentProvider,
}

impl PyEnvironmentProvider {
    pub(crate) fn provider(&self) -> DynEnvironmentProvider {
        self.inner.clone()
    }
}

#[pymethods]
impl PyEnvironmentProvider {
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
            command,
            timeout_seconds,
            cwd,
            environment,
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

fn environment_error_to_py(error: starweaver_environment::EnvironmentError) -> PyFutureError {
    PyFutureError::State(error.to_string())
}
