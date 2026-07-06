//! Python wrappers for Starweaver model configuration and provider models.

use std::{collections::BTreeMap, env, path::PathBuf, sync::Arc};

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::PyDict,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use starweaver_model::{
    ModelAdapter, ModelError, ModelProfile, ModelRequestParameters, ModelSettings,
    ProfileOverrideModel, ProtocolFamily, ProtocolModelClient, ReqwestHttpClient,
    anthropic_http_config, gemini_http_config, get_model_config, get_model_settings,
    openai_chat_http_config, openai_responses_http_config,
};
use starweaver_oauth::{OAuthError, OAuthStore};

use crate::conversion::{py_to_json, serialize_to_py};

/// Python wrapper around provider-neutral model settings.
#[pyclass(name = "ModelSettings", skip_from_py_object)]
#[derive(Clone)]
pub struct PyModelSettings {
    inner: ModelSettings,
}

impl PyModelSettings {
    pub(crate) fn settings(&self) -> ModelSettings {
        self.inner.clone()
    }
}

#[pymethods]
impl PyModelSettings {
    #[new]
    #[pyo3(signature = (value=None, **kwargs))]
    fn new(
        py: Python<'_>,
        value: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: parse_json_object_input(py, value, kwargs, "model settings")?,
        })
    }

    #[staticmethod]
    fn preset(name: String) -> PyResult<Self> {
        let inner =
            get_model_settings(&name).map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { inner })
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner)
    }
}

/// Python wrapper around provider-neutral request parameters.
#[pyclass(name = "RequestParams", skip_from_py_object)]
#[derive(Clone)]
pub struct PyRequestParams {
    inner: ModelRequestParameters,
}

impl PyRequestParams {
    pub(crate) fn params(&self) -> ModelRequestParameters {
        self.inner.clone()
    }
}

#[pymethods]
impl PyRequestParams {
    #[new]
    #[pyo3(signature = (value=None, **kwargs))]
    fn new(
        py: Python<'_>,
        value: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: parse_json_object_input(py, value, kwargs, "request params")?,
        })
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner)
    }
}

/// Python wrapper around a production provider model adapter.
#[pyclass(name = "ProviderModel", skip_from_py_object)]
#[derive(Clone)]
pub struct PyProviderModel {
    inner: Arc<dyn ModelAdapter>,
}

impl PyProviderModel {
    pub(crate) fn model(&self) -> Arc<dyn ModelAdapter> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyProviderModel {
    #[staticmethod]
    #[pyo3(signature = (model_id, api_key=None, api_key_env=None, model_config_preset=None, model_settings=None, base_url=None, endpoint_path=None))]
    #[allow(clippy::too_many_arguments)]
    fn from_model_id(
        py: Python<'_>,
        model_id: String,
        api_key: Option<String>,
        api_key_env: Option<String>,
        model_config_preset: Option<String>,
        model_settings: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
        endpoint_path: Option<String>,
    ) -> PyResult<Self> {
        if let Some(rest) = model_id.strip_prefix("oauth@") {
            let (provider_name, model_name) = rest.split_once(':').ok_or_else(|| {
                PyValueError::new_err(format!(
                    "invalid OAuth model id {model_id:?}; expected oauth@provider:model"
                ))
            })?;
            if provider_name.is_empty() || model_name.is_empty() {
                return Err(PyValueError::new_err(format!(
                    "invalid OAuth model id {model_id:?}; expected oauth@provider:model"
                )));
            }
            return build_oauth_model(
                provider_name,
                model_name,
                extract_model_settings(py, model_settings)?,
                None,
            );
        }

        let (provider_name, model_name) = model_id.split_once(':').ok_or_else(|| {
            PyValueError::new_err(format!(
                "invalid model id {model_id:?}; expected provider:model"
            ))
        })?;
        if provider_name.is_empty() || model_name.is_empty() {
            return Err(PyValueError::new_err(format!(
                "invalid model id {model_id:?}; expected provider:model"
            )));
        }

        match provider_name {
            "openai" | "openai_responses" => Self::openai_responses(
                py,
                model_name.to_string(),
                api_key,
                api_key_env,
                model_config_preset,
                model_settings,
                base_url,
                endpoint_path,
            ),
            "openai_chat" => Self::openai_chat(
                py,
                model_name.to_string(),
                api_key,
                api_key_env,
                model_config_preset,
                model_settings,
                base_url,
                endpoint_path,
            ),
            "anthropic" => Self::anthropic(
                py,
                model_name.to_string(),
                api_key,
                api_key_env,
                model_config_preset,
                model_settings,
                base_url,
                endpoint_path,
            ),
            "gemini" => Self::gemini(
                py,
                model_name.to_string(),
                api_key,
                api_key_env,
                model_config_preset,
                model_settings,
                base_url,
                endpoint_path,
            ),
            other => Err(PyValueError::new_err(format!(
                "unsupported model provider prefix {other:?}"
            ))),
        }
    }

    #[staticmethod]
    #[pyo3(signature = (model_name, model_settings=None, auth_file=None))]
    fn codex_oauth(
        py: Python<'_>,
        model_name: String,
        model_settings: Option<&Bound<'_, PyAny>>,
        auth_file: Option<String>,
    ) -> PyResult<Self> {
        build_oauth_model(
            "codex",
            &model_name,
            extract_model_settings(py, model_settings)?,
            auth_file,
        )
    }

    #[staticmethod]
    #[pyo3(signature = (model_name, api_key=None, api_key_env=None, model_config_preset=None, model_settings=None, base_url=None, endpoint_path=None))]
    #[allow(clippy::too_many_arguments)]
    fn openai_responses(
        py: Python<'_>,
        model_name: String,
        api_key: Option<String>,
        api_key_env: Option<String>,
        model_config_preset: Option<String>,
        model_settings: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
        endpoint_path: Option<String>,
    ) -> PyResult<Self> {
        let mut http_config =
            openai_responses_http_config(resolve_api_key(api_key, api_key_env, "OPENAI_API_KEY")?);
        apply_http_overrides(&mut http_config, base_url, endpoint_path);
        build_protocol_model(
            "openai",
            model_name,
            ProtocolFamily::OpenAiResponses,
            http_config,
            model_config_preset,
            extract_model_settings(py, model_settings)?,
        )
    }

    #[staticmethod]
    #[pyo3(signature = (model_name, api_key=None, api_key_env=None, model_config_preset=None, model_settings=None, base_url=None, endpoint_path=None))]
    #[allow(clippy::too_many_arguments)]
    fn openai_chat(
        py: Python<'_>,
        model_name: String,
        api_key: Option<String>,
        api_key_env: Option<String>,
        model_config_preset: Option<String>,
        model_settings: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
        endpoint_path: Option<String>,
    ) -> PyResult<Self> {
        let mut http_config =
            openai_chat_http_config(resolve_api_key(api_key, api_key_env, "OPENAI_API_KEY")?);
        apply_http_overrides(&mut http_config, base_url, endpoint_path);
        build_protocol_model(
            "openai",
            model_name,
            ProtocolFamily::OpenAiChatCompletions,
            http_config,
            model_config_preset,
            extract_model_settings(py, model_settings)?,
        )
    }

    #[staticmethod]
    #[pyo3(signature = (model_name, api_key=None, api_key_env=None, model_config_preset=None, model_settings=None, base_url=None, endpoint_path=None))]
    #[allow(clippy::too_many_arguments)]
    fn anthropic(
        py: Python<'_>,
        model_name: String,
        api_key: Option<String>,
        api_key_env: Option<String>,
        model_config_preset: Option<String>,
        model_settings: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
        endpoint_path: Option<String>,
    ) -> PyResult<Self> {
        let mut http_config =
            anthropic_http_config(resolve_api_key(api_key, api_key_env, "ANTHROPIC_API_KEY")?);
        apply_http_overrides(&mut http_config, base_url, endpoint_path);
        build_protocol_model(
            "anthropic",
            model_name,
            ProtocolFamily::AnthropicMessages,
            http_config,
            model_config_preset,
            extract_model_settings(py, model_settings)?,
        )
    }

    #[staticmethod]
    #[pyo3(signature = (model_name, api_key=None, api_key_env=None, model_config_preset=None, model_settings=None, base_url=None, endpoint_path=None))]
    #[allow(clippy::too_many_arguments)]
    fn gemini(
        py: Python<'_>,
        model_name: String,
        api_key: Option<String>,
        api_key_env: Option<String>,
        model_config_preset: Option<String>,
        model_settings: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
        endpoint_path: Option<String>,
    ) -> PyResult<Self> {
        let mut http_config = gemini_http_config(
            resolve_api_key(api_key, api_key_env, "GEMINI_API_KEY")?,
            model_name.clone(),
        );
        apply_http_overrides(&mut http_config, base_url, endpoint_path);
        build_protocol_model(
            "gemini",
            model_name,
            ProtocolFamily::GeminiGenerateContent,
            http_config,
            model_config_preset,
            extract_model_settings(py, model_settings)?,
        )
    }
}

#[pyfunction]
#[pyo3(signature = (provider_name, auth_file=None))]
pub(crate) fn oauth_provider_status(
    py: Python<'_>,
    provider_name: String,
    auth_file: Option<String>,
) -> PyResult<Py<PyAny>> {
    let store = oauth_store(auth_file);
    let record = store
        .get_provider(&provider_name)
        .map_err(oauth_error_to_py)?;
    let status = match record {
        Some(record) => {
            let record_status = record.status_value();
            json!({
                "provider_name": provider_name,
                "auth_file": store.path(),
                "logged_in": true,
                "account": record.account,
                "has_access_token": !record.tokens.access_token.trim().is_empty(),
                "has_refresh_token": record.tokens.refresh_token.as_ref().is_some_and(|token| !token.trim().is_empty()),
                "last_refresh_at": record.last_refresh_at,
                "record": record_status,
            })
        }
        None => json!({
            "provider_name": provider_name,
            "auth_file": store.path(),
            "logged_in": false,
            "account": null,
            "has_access_token": false,
            "has_refresh_token": false,
            "last_refresh_at": null,
            "record": null,
        }),
    };
    serialize_to_py(py, &status)
}

#[pyfunction]
#[pyo3(signature = (provider_name, auth_file=None))]
pub(crate) fn oauth_provider_redacted_record(
    py: Python<'_>,
    provider_name: String,
    auth_file: Option<String>,
) -> PyResult<Py<PyAny>> {
    let store = oauth_store(auth_file);
    let record = store
        .get_provider(&provider_name)
        .map_err(oauth_error_to_py)?
        .map(|record| record.redacted_value());
    serialize_to_py(py, &record)
}

pub(crate) fn extract_model_settings(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<ModelSettings>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    if let Ok(value) = value.extract::<PyRef<'_, PyModelSettings>>() {
        return Ok(Some(value.settings()));
    }
    serde_json::from_value(py_to_json(py, value)?)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid model settings: {error}")))
}

pub(crate) fn extract_request_params(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<ModelRequestParameters>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    if let Ok(value) = value.extract::<PyRef<'_, PyRequestParams>>() {
        return Ok(Some(value.params()));
    }
    serde_json::from_value(py_to_json(py, value)?)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid request params: {error}")))
}

fn parse_json_object_input<T>(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
    kwargs: Option<&Bound<'_, PyDict>>,
    label: &str,
) -> PyResult<T>
where
    T: DeserializeOwned,
{
    let mut object = match value {
        Some(value) if !value.is_none() => match py_to_json(py, value)? {
            Value::Object(object) => object,
            _ => return Err(PyValueError::new_err(format!("{label} must be a mapping"))),
        },
        Some(_) | None => Map::new(),
    };
    if let Some(kwargs) = kwargs {
        for (key, value) in kwargs.iter() {
            let key: String = key.extract()?;
            object.insert(key, py_to_json(py, &value)?);
        }
    }
    serde_json::from_value(Value::Object(object))
        .map_err(|error| PyValueError::new_err(format!("invalid {label}: {error}")))
}

fn resolve_api_key(
    api_key: Option<String>,
    api_key_env: Option<String>,
    default_env: &str,
) -> PyResult<String> {
    if let Some(api_key) = api_key.filter(|value| !value.trim().is_empty()) {
        return Ok(api_key);
    }
    let env_name = api_key_env.unwrap_or_else(|| default_env.to_string());
    env::var(&env_name).map_err(|_| {
        PyValueError::new_err(format!(
            "missing {env_name}; pass api_key=... or set the environment variable"
        ))
    })
}

fn apply_http_overrides(
    http_config: &mut starweaver_model::HttpModelConfig,
    base_url: Option<String>,
    endpoint_path: Option<String>,
) {
    if let Some(base_url) = base_url {
        http_config.set_base_url(base_url);
    }
    if let Some(endpoint_path) = endpoint_path {
        http_config.set_endpoint_path(endpoint_path);
    }
}

fn build_protocol_model(
    provider_name: impl Into<String>,
    model_name: String,
    fallback_protocol: ProtocolFamily,
    http_config: starweaver_model::HttpModelConfig,
    model_config_preset: Option<String>,
    model_settings: Option<ModelSettings>,
) -> PyResult<PyProviderModel> {
    let profile = provider_profile(model_config_preset.as_deref(), fallback_protocol)?;
    let http_client =
        ReqwestHttpClient::new().map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    let mut client = ProtocolModelClient::new(
        provider_name,
        model_name,
        profile,
        http_config,
        Arc::new(http_client),
    );
    if let Some(settings) = model_settings {
        client = client.with_default_settings(settings);
    }
    Ok(PyProviderModel {
        inner: Arc::new(client),
    })
}

fn build_oauth_model(
    provider_name: &str,
    model_name: &str,
    model_settings: Option<ModelSettings>,
    auth_file: Option<String>,
) -> PyResult<PyProviderModel> {
    let model = match (provider_name, auth_file) {
        ("codex", Some(auth_file)) => starweaver_oauth_provider::build_codex_model_with_store(
            model_name,
            OAuthStore::new(PathBuf::from(auth_file)),
            BTreeMap::new(),
        )
        .map_err(model_error_to_py)?,
        _ => starweaver_oauth_provider::infer_oauth_model(provider_name, model_name)
            .map_err(model_error_to_py)?,
    };
    let mut inner: Arc<dyn ModelAdapter> = Arc::new(model);
    if let Some(settings) = model_settings {
        let profile = inner.profile().clone();
        inner = Arc::new(ProfileOverrideModel::new(inner, profile).with_default_settings(settings));
    }
    Ok(PyProviderModel { inner })
}

fn provider_profile(
    model_config_preset: Option<&str>,
    fallback_protocol: ProtocolFamily,
) -> PyResult<ModelProfile> {
    let Some(preset) = model_config_preset else {
        return Ok(ModelProfile::for_protocol(fallback_protocol));
    };
    get_model_config(preset)
        .map(|config| config.profile)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

#[allow(dead_code)]
fn model_error_to_py(error: ModelError) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}

fn oauth_store(auth_file: Option<String>) -> OAuthStore {
    auth_file.map_or_else(OAuthStore::default_store, |path| {
        OAuthStore::new(PathBuf::from(path))
    })
}

fn oauth_error_to_py(error: OAuthError) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}
