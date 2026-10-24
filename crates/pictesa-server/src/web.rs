//! Serves the web UI: embedded into the binary at build time, or from a folder in development.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::AppState;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Ui;

pub async fn serve(State(s): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/').to_string();
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    if let Some(dir) = &s.opts.web_dir {
        let index = dir.join("index.html");
        return match ServeDir::new(dir).fallback(ServeFile::new(index)).oneshot(req).await {
            Ok(r) => r.map(Body::new),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
    }
    let (file, name) = match Ui::get(&path).filter(|_| !path.is_empty()) {
        Some(f) => (f, path.as_str()),
        // Client-side routes (/library, /settings, ...) all load the app shell.
        None => match Ui::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return (StatusCode::NOT_FOUND, "UI not built").into_response(),
        },
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let cache = if name.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    let mut resp = Response::new(Body::from(file.data.into_owned()));
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if name == "index.html" {
        h.insert("x-frame-options", HeaderValue::from_static("DENY"));
        h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    }
    resp
}
