//! Python wrappers for Starweaver skill registries.

use pyo3::{exceptions::PyValueError, prelude::*};
use serde_json::Value;
use starweaver_agent::{
    SkillPackage, SkillRegistry, SkillSourceKind, SkillSourceScope, parse_skill_markdown,
};

use crate::{
    conversion::{json_to_py, py_to_json, serialize_to_py},
    environment::PyEnvironmentProvider,
    runtime::{PyFutureError, spawn_py_future},
    toolset::PyToolset,
};

/// Python-visible Starweaver skill package.
#[pyclass(name = "SkillPackage", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PySkillPackage {
    inner: SkillPackage,
}

impl PySkillPackage {
    fn new(inner: SkillPackage) -> Self {
        Self { inner }
    }

    fn package(&self) -> SkillPackage {
        self.inner.clone()
    }
}

#[pymethods]
impl PySkillPackage {
    #[new]
    #[pyo3(signature = (name, description, path, body=None, metadata=None))]
    fn new_py(
        py: Python<'_>,
        name: String,
        description: String,
        path: String,
        body: Option<String>,
        metadata: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let metadata = match metadata {
            Some(value) => match py_to_json(py, value)? {
                Value::Object(object) => object,
                _ => return Err(PyValueError::new_err("metadata must be a mapping")),
            },
            None => serde_json::Map::new(),
        };
        Ok(Self::new(SkillPackage {
            name,
            description,
            path,
            body,
            metadata,
        }))
    }

    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    #[getter]
    fn path(&self) -> &str {
        &self.inner.path
    }

    #[getter]
    fn body(&self) -> Option<String> {
        self.inner.body.clone()
    }

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Object(self.inner.metadata.clone()))
    }

    fn summary_line(&self) -> String {
        self.inner.summary_line()
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner)
    }
}

/// Python-visible Starweaver skill registry.
#[pyclass(name = "SkillRegistry", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PySkillRegistry {
    inner: SkillRegistry,
}

impl PySkillRegistry {
    pub(crate) fn registry(&self) -> SkillRegistry {
        self.inner.clone()
    }
}

#[pymethods]
impl PySkillRegistry {
    #[new]
    #[pyo3(signature = (packages=None))]
    fn new(py: Python<'_>, packages: Option<Vec<Py<PySkillPackage>>>) -> PyResult<Self> {
        let mut registry = SkillRegistry::new();
        for package in packages.unwrap_or_default() {
            registry.insert(package.borrow(py).package());
        }
        Ok(Self { inner: registry })
    }

    #[staticmethod]
    fn parse(path: String, content: String) -> PyResult<PySkillPackage> {
        parse_skill_markdown(&path, &content)
            .map(PySkillPackage::new)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    #[staticmethod]
    #[pyo3(signature = (environment, scopes=None))]
    fn scan(
        py: Python<'_>,
        environment: Py<PyEnvironmentProvider>,
        scopes: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let provider = environment.borrow(py).provider();
        let scopes = parse_scopes(py, scopes)?;
        spawn_py_future(
            py,
            async move {
                SkillRegistry::scan(provider, &scopes)
                    .await
                    .map_err(skill_error_to_py)
            },
            |py, registry| Ok(Py::new(py, PySkillRegistry { inner: registry })?.into_any()),
        )
    }

    #[staticmethod]
    #[pyo3(signature = (environment, scopes=None))]
    fn scan_with_report(
        py: Python<'_>,
        environment: Py<PyEnvironmentProvider>,
        scopes: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let provider = environment.borrow(py).provider();
        let scopes = parse_scopes(py, scopes)?;
        spawn_py_future(
            py,
            async move {
                SkillRegistry::scan_with_report(provider, &scopes)
                    .await
                    .map_err(skill_error_to_py)
            },
            |py, report| serialize_to_py(py, &report),
        )
    }

    #[staticmethod]
    fn activate(
        py: Python<'_>,
        environment: Py<PyEnvironmentProvider>,
        path: String,
    ) -> PyResult<Py<PyAny>> {
        let provider = environment.borrow(py).provider();
        spawn_py_future(
            py,
            async move {
                SkillRegistry::activate(provider, &path)
                    .await
                    .map_err(skill_error_to_py)
            },
            |py, package| Ok(Py::new(py, PySkillPackage::new(package))?.into_any()),
        )
    }

    fn insert(&mut self, package: Py<PySkillPackage>, py: Python<'_>) {
        self.inner.insert(package.borrow(py).package());
    }

    fn get(&self, name: &str) -> Option<PySkillPackage> {
        self.inner.get(name).cloned().map(PySkillPackage::new)
    }

    #[getter]
    fn packages(&self) -> Vec<PySkillPackage> {
        self.inner
            .packages()
            .into_iter()
            .map(PySkillPackage::new)
            .collect()
    }

    #[getter]
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn toolset(&self) -> PyToolset {
        PyToolset::new(self.inner.toolset())
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner)
    }
}

pub(crate) fn extract_skill_registry(
    _py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<SkillRegistry>> {
    match value {
        Some(value) if !value.is_none() => {
            if let Ok(registry) = value.extract::<PyRef<'_, PySkillRegistry>>() {
                Ok(Some(registry.registry()))
            } else if let Ok(to_native) = value.getattr("to_native") {
                let native = to_native.call0()?;
                let registry = native.extract::<PyRef<'_, PySkillRegistry>>()?;
                Ok(Some(registry.registry()))
            } else {
                Err(PyValueError::new_err("skills must be a SkillRegistry"))
            }
        }
        Some(_) | None => Ok(None),
    }
}

fn parse_scopes(
    py: Python<'_>,
    scopes: Option<&Bound<'_, PyAny>>,
) -> PyResult<Vec<SkillSourceScope>> {
    let Some(scopes) = scopes else {
        return Ok(vec![SkillSourceScope::new("")]);
    };
    if scopes.is_none() {
        return Ok(vec![SkillSourceScope::new("")]);
    }
    let value = py_to_json(py, scopes)?;
    match value {
        Value::String(root) => Ok(vec![SkillSourceScope::new(root)]),
        Value::Array(items) => items.into_iter().map(parse_scope_value).collect(),
        Value::Object(_) => parse_scope_value(value).map(|scope| vec![scope]),
        _ => Err(PyValueError::new_err(
            "skill scopes must be a string, mapping, or list",
        )),
    }
}

fn parse_scope_value(value: Value) -> PyResult<SkillSourceScope> {
    match value {
        Value::String(root) => Ok(SkillSourceScope::new(root)),
        Value::Object(mut object) => {
            let root = object
                .remove("root")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            let mut scope = SkillSourceScope::new(root);
            if let Some(source) = object.remove("source") {
                let source = source
                    .as_str()
                    .ok_or_else(|| PyValueError::new_err("skill scope source must be a string"))
                    .and_then(|source| parse_source_kind(source).map_err(PyValueError::new_err))?;
                scope = scope.with_source(source);
            }
            if let Some(Value::Array(directories)) = object.remove("directories") {
                scope = scope.with_directories(
                    directories
                        .into_iter()
                        .filter_map(|value| value.as_str().map(ToOwned::to_owned)),
                );
            }
            Ok(scope)
        }
        _ => Err(PyValueError::new_err(
            "skill scope entries must be strings or mappings",
        )),
    }
}

fn parse_source_kind(value: &str) -> Result<SkillSourceKind, String> {
    match value {
        "built_in" => Ok(SkillSourceKind::BuiltIn),
        "user_shared" => Ok(SkillSourceKind::UserShared),
        "user_tool" => Ok(SkillSourceKind::UserTool),
        "workspace_shared" => Ok(SkillSourceKind::WorkspaceShared),
        "workspace_tool" => Ok(SkillSourceKind::WorkspaceTool),
        "custom" => Ok(SkillSourceKind::Custom),
        other => Err(format!("unsupported skill source kind: {other}")),
    }
}

fn skill_error_to_py(error: starweaver_agent::SkillError) -> PyFutureError {
    PyFutureError::State(error.to_string())
}
