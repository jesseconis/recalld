use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::Rng;
use rand::distr::Alphanumeric;
use serde::{Deserialize, Serialize};

use crate::daemon::DaemonState;

const SESSION_COOKIE_NAME: &str = "recalld_session";

const INDEX_HTML: &str = include_str!("static/index.html");
const APP_JS: &str = include_str!("static/app.js");
const STYLES_CSS: &str = include_str!("static/styles.css");

#[derive(Clone)]
struct WebState {
    daemon: Arc<DaemonState>,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    session_ttl: Duration,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    passphrase: String,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    ok: bool,
}

#[derive(Debug, Deserialize)]
struct PaginationParams {
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GalleryParams {
    page: Option<u32>,
    per_page: Option<u32>,
    from_timestamp: Option<i64>,
    to_timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Serialize)]
struct PageMeta {
    page: u32,
    per_page: u32,
    total: u64,
    total_pages: u32,
}

#[derive(Debug, Serialize)]
struct GalleryItem {
    id: i64,
    app: String,
    title: String,
    timestamp: i64,
    screenshot_filename: String,
}

#[derive(Debug, Serialize)]
struct SearchItem {
    id: i64,
    app: String,
    title: String,
    text: String,
    timestamp: i64,
    similarity: f32,
    screenshot_filename: String,
}

#[derive(Debug, Serialize)]
struct EntryDetailResponse {
    id: i64,
    app: String,
    title: String,
    text: String,
    timestamp: i64,
    screenshot_filename: String,
}

#[derive(Debug, Serialize)]
struct PagedResponse<T> {
    meta: PageMeta,
    items: Vec<T>,
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    running: bool,
    uptime_seconds: i64,
    total_entries: i64,
    last_capture_timestamp: i64,
    capture_backend: String,
}

#[derive(Debug, Serialize)]
struct ConfigResponse {
    config_toml: String,
}

pub async fn serve(
    state: Arc<DaemonState>,
    addr: SocketAddr,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let session_ttl = Duration::from_secs(state.config.http.session_ttl_secs);
    let web_state = WebState {
        daemon: state,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        session_ttl,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/api/session/login", post(login))
        .route("/api/gallery", get(gallery))
        .route("/api/search", get(search))
        .route("/api/entry/{id}", get(entry_detail))
        .route("/api/screenshot/{filename}", get(screenshot))
        .route("/api/status", get(status))
        .route("/api/config", get(config))
        .with_state(web_state);

    tracing::info!(%addr, "HTTP server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await?;

    Ok(())
}

async fn shutdown_signal(rx: tokio::sync::watch::Receiver<bool>) {
    let mut rx = rx;
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn app_js() -> impl IntoResponse {
    ([ (header::CONTENT_TYPE, "application/javascript; charset=utf-8") ], APP_JS)
}

async fn styles_css() -> impl IntoResponse {
    ([ (header::CONTENT_TYPE, "text/css; charset=utf-8") ], STYLES_CSS)
}

async fn login(
    State(state): State<WebState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Response, StatusCode> {
    if payload.passphrase.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let key_path = state.daemon.config.key_path();
    let passphrase = payload.passphrase;
    let unlock_result = tokio::task::spawn_blocking(move || {
        crate::storage::crypto::unlock(passphrase.as_bytes(), &key_path).map(|_| ())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if unlock_result.is_err() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();

    let expires_at = Instant::now() + state.session_ttl;
    {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.insert(token.clone(), expires_at);
    }

    let mut response = Json(LoginResponse { ok: true }).into_response();
    let cookie = format!(
        "{name}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}",
        name = SESSION_COOKIE_NAME,
        value = token,
        max_age = state.session_ttl.as_secs()
    );
    let cookie_value =
        HeaderValue::from_str(&cookie).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    response.headers_mut().insert(header::SET_COOKIE, cookie_value);

    Ok(response)
}

async fn gallery(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(params): Query<GalleryParams>,
) -> Result<Json<PagedResponse<GalleryItem>>, StatusCode> {
    require_auth(&headers, &state)?;

    let (page, per_page, offset) = normalize_pagination(
        PaginationParams {
            page: params.page,
            per_page: params.per_page,
        },
        &state,
    );

    let from = params.from_timestamp.unwrap_or(0);
    let to = params.to_timestamp.unwrap_or(i64::MAX);
    let storage = Arc::clone(&state.daemon.storage);

    let timeline_page = tokio::task::spawn_blocking(move || {
        storage.timeline_paged(from, to, per_page, offset)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let items = timeline_page
        .entries
        .into_iter()
        .map(|entry| GalleryItem {
            id: entry.id,
            app: entry.app,
            title: entry.title,
            timestamp: entry.timestamp,
            screenshot_filename: entry.screenshot_filename,
        })
        .collect::<Vec<_>>();

    let meta = build_meta(page, per_page, timeline_page.total.max(0) as u64);
    Ok(Json(PagedResponse { meta, items }))
}

async fn search(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Result<Json<PagedResponse<SearchItem>>, StatusCode> {
    require_auth(&headers, &state)?;

    let query = params.q.trim().to_string();
    if query.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (page, per_page, offset) = normalize_pagination(
        PaginationParams {
            page: params.page,
            per_page: params.per_page,
        },
        &state,
    );

    let storage = Arc::clone(&state.daemon.storage);
    let search_page = tokio::task::spawn_blocking(move || {
        let embedding = crate::embedding::embed(&query)?;
        storage.search_paged(&embedding, per_page as usize, offset as usize)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let items = search_page
        .results
        .into_iter()
        .map(|entry| SearchItem {
            id: entry.id,
            app: entry.app,
            title: entry.title,
            text: entry.text,
            timestamp: entry.timestamp,
            similarity: entry.similarity,
            screenshot_filename: entry.screenshot_filename,
        })
        .collect::<Vec<_>>();

    let meta = build_meta(page, per_page, search_page.total as u64);
    Ok(Json(PagedResponse { meta, items }))
}

async fn entry_detail(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<EntryDetailResponse>, StatusCode> {
    require_auth(&headers, &state)?;

    let storage = Arc::clone(&state.daemon.storage);
    let detail = tokio::task::spawn_blocking(move || storage.entry_detail(id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let Some(detail) = detail else {
        return Err(StatusCode::NOT_FOUND);
    };

    Ok(Json(EntryDetailResponse {
        id: detail.id,
        app: detail.app,
        title: detail.title,
        text: detail.text,
        timestamp: detail.timestamp,
        screenshot_filename: detail.screenshot_filename,
    }))
}

async fn screenshot(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(filename): Path<String>,
) -> Result<Response, StatusCode> {
    require_auth(&headers, &state)?;

    if !safe_screenshot_filename(&filename) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let storage = Arc::clone(&state.daemon.storage);
    let filename_for_storage = filename.clone();
    let data = tokio::task::spawn_blocking(move || storage.get_screenshot(&filename_for_storage))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let mut response = data.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/webp"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn status(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<StatusResponse>, StatusCode> {
    require_auth(&headers, &state)?;

    let storage = Arc::clone(&state.daemon.storage);
    let (total, last_ts) = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>((storage.count()?, storage.latest_timestamp()?))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let uptime = state.daemon.start_time.elapsed().as_secs() as i64;
    Ok(Json(StatusResponse {
        running: true,
        uptime_seconds: uptime,
        total_entries: total,
        last_capture_timestamp: last_ts,
        capture_backend: state.daemon.config.capture.backend.clone(),
    }))
}

async fn config(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<ConfigResponse>, StatusCode> {
    require_auth(&headers, &state)?;

    let config_toml = toml::to_string_pretty(&state.daemon.config)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(ConfigResponse { config_toml }))
}

fn normalize_pagination(params: PaginationParams, state: &WebState) -> (u32, u32, u32) {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params
        .per_page
        .unwrap_or(state.daemon.config.http.default_page_size)
        .clamp(1, state.daemon.config.http.max_page_size);
    let offset = page.saturating_sub(1).saturating_mul(per_page);

    (page, per_page, offset)
}

fn build_meta(page: u32, per_page: u32, total: u64) -> PageMeta {
    let total_pages = if total == 0 {
        0
    } else {
        ((total + per_page as u64 - 1) / per_page as u64) as u32
    };

    PageMeta {
        page,
        per_page,
        total,
        total_pages,
    }
}

fn require_auth(headers: &HeaderMap, state: &WebState) -> Result<(), StatusCode> {
    let Some(token) = read_cookie(headers, SESSION_COOKIE_NAME) else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let now = Instant::now();
    let mut sessions = state.sessions.lock().unwrap();
    sessions.retain(|_, expiry| *expiry > now);

    if let Some(expiry) = sessions.get_mut(token) {
        *expiry = now + state.session_ttl;
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn read_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;

    raw.split(';')
        .map(|part| part.trim())
        .find_map(|part| {
            let (k, v) = part.split_once('=')?;
            if k == name { Some(v) } else { None }
        })
}

fn safe_screenshot_filename(filename: &str) -> bool {
    !filename.contains('/')
        && !filename.contains('\\')
        && !filename.contains("..")
        && filename.ends_with(".webp.enc")
}
