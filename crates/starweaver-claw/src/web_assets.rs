//! Embedded web console assets for Starweaver Claw.

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use include_dir::{include_dir, Dir};

const WEB_DIST: Dir<'_> = include_dir!("$OUT_DIR/claw-web-dist");
const INDEX_HTML: &str = "index.html";

/// Return whether the compiled binary contains the web console bundle.
#[must_use]
pub fn is_available() -> bool {
    WEB_DIST.get_file(INDEX_HTML).is_some()
}

/// Serve an embedded static asset or fall back to the SPA entry point.
#[must_use]
pub fn serve(uri: &Uri) -> Response {
    if !is_available() {
        return (StatusCode::NOT_FOUND, "web console bundle is not embedded").into_response();
    }

    let path = normalize_path(uri.path());
    if let Some(file) = WEB_DIST.get_file(&path) {
        return asset_response(path, file.contents());
    }

    let Some(index) = WEB_DIST.get_file(INDEX_HTML) else {
        return (StatusCode::NOT_FOUND, "web console index is not embedded").into_response();
    };
    asset_response(INDEX_HTML.to_string(), index.contents())
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        INDEX_HTML.to_string()
    } else {
        trimmed.to_string()
    }
}

fn asset_response(path: String, bytes: &'static [u8]) -> Response {
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let mut response = Body::from(bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::*;

    #[test]
    fn embedded_bundle_contains_index() {
        assert!(is_available());
    }

    #[test]
    fn unknown_frontend_path_falls_back_to_index() {
        let response = serve(&Uri::from_static("/sessions/session_test"));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html")),
        );
    }
}
