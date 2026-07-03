//! Python-backed media uploader adapter.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict},
};
use serde_json::{Map, Value};
use starweaver_agent::{MediaUploadRequest, MediaUploader};
use starweaver_model::ContentPart;

use crate::{conversion::py_to_json, conversion::serialize_to_py};

/// Python-visible media uploader wrapper.
#[pyclass(name = "MediaUploader", skip_from_py_object)]
#[derive(Clone)]
pub struct PyMediaUploader {
    inner: Arc<PythonMediaUploader>,
}

impl PyMediaUploader {
    pub(crate) fn uploader(&self) -> Arc<dyn MediaUploader> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyMediaUploader {
    #[new]
    fn new(callback: Py<PyAny>, event_loop: Py<PyAny>) -> Self {
        Self {
            inner: Arc::new(PythonMediaUploader {
                callback,
                event_loop,
            }),
        }
    }
}

struct PythonMediaUploader {
    callback: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonMediaUploader {}
unsafe impl Sync for PythonMediaUploader {}

#[async_trait]
impl MediaUploader for PythonMediaUploader {
    async fn upload(&self, request: MediaUploadRequest) -> Result<ContentPart, String> {
        let future = Python::attach(|py| -> PyResult<Py<PyAny>> {
            let request_dict = PyDict::new(py);
            request_dict.set_item("data", PyBytes::new(py, &request.data))?;
            request_dict.set_item("media_type", request.media_type)?;
            request_dict.set_item("preflight", serialize_to_py(py, &request.preflight)?)?;
            let coroutine = self.callback.call1(py, (request_dict,))?;
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (coroutine, self.event_loop.clone_ref(py)),
            )?;
            Ok(future.unbind())
        })
        .map_err(|error| error.to_string())?;

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
        };
        cancel_guard.complete();

        match result {
            Ok(value) => Python::attach(|py| py_value_to_content_part(py, value.bind(py))),
            Err(error) => Err(error.to_string()),
        }
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

fn py_value_to_content_part(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> Result<ContentPart, String> {
    let value = py_to_json(py, value).map_err(|error| error.to_string())?;
    if let Ok(part) = serde_json::from_value::<ContentPart>(value.clone()) {
        return Ok(part);
    }
    let Value::Object(mut object) = value else {
        return Err("media uploader must return a content-part mapping".to_string());
    };
    if let Some(data_url) = take_string(&mut object, "data_url") {
        let media_type = take_string(&mut object, "media_type").unwrap_or_default();
        return Ok(ContentPart::DataUrl {
            data_url,
            media_type,
        });
    }
    if let Some(uri) = take_string(&mut object, "uri") {
        let media_type = take_string(&mut object, "media_type").unwrap_or_default();
        let resource_type = take_string(&mut object, "resource_type")
            .or_else(|| take_string(&mut object, "type"))
            .unwrap_or_else(|| "media".to_string());
        let metadata = take_metadata(object.remove("metadata"))?;
        return Ok(ContentPart::ResourceRef {
            uri,
            media_type,
            resource_type,
            metadata,
        });
    }
    if let Some(url) = take_string(&mut object, "url") {
        let media_type = take_string(&mut object, "media_type").unwrap_or_default();
        if media_type.starts_with("image/") || media_type.is_empty() {
            return Ok(ContentPart::ImageUrl { url });
        }
        return Ok(ContentPart::FileUrl { url, media_type });
    }
    Err(
        "media uploader result must include uri, url, data_url, or a serialized ContentPart"
            .to_string(),
    )
}

fn take_string(object: &mut Map<String, Value>, key: &str) -> Option<String> {
    object
        .remove(key)
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
}

fn take_metadata(value: Option<Value>) -> Result<Map<String, Value>, String> {
    match value {
        Some(Value::Object(object)) => Ok(object),
        Some(_) => Err("media uploader metadata must be a mapping".to_string()),
        None => Ok(Map::new()),
    }
}

pub(crate) fn extract_media_uploader(
    _py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Arc<dyn MediaUploader>>> {
    match value {
        Some(value) if !value.is_none() => {
            if let Ok(uploader) = value.extract::<PyRef<'_, PyMediaUploader>>() {
                Ok(Some(uploader.uploader()))
            } else if let Ok(to_native) = value.getattr("to_native") {
                let native = to_native.call0()?;
                let uploader = native.extract::<PyRef<'_, PyMediaUploader>>()?;
                Ok(Some(uploader.uploader()))
            } else {
                Err(PyValueError::new_err(
                    "media_uploader must be a MediaUploader",
                ))
            }
        }
        Some(_) | None => Ok(None),
    }
}
