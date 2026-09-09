mod export;
mod jobs;
mod logs;
mod model_assets;
mod rain_classroom;
mod recognition_eta;
mod slide_ocr;
mod slide_review;
mod speech_recognition;
mod transcription;
mod usage;
pub(crate) mod worker;

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::OsStr,
    fs, io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Component, Path as FsPath, PathBuf},
    process::Stdio,
    str::FromStr,
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{io::AsyncWriteExt, sync::Mutex};
use tower_http::services::ServeFile;

use crate::{
    run_support::{read_json, write_json_atomically},
    worker_control::WorkerProgress,
};
use jobs::{
    Job, Outcome, OutcomeStatus, Run, Settings, is_worker_running, job_path, now_ms, read_job,
    save_job,
};
use usage::{TraceCursor, Usage};

const EXAMPLE_REPORT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/demo/report.html"
));
const BIND_ADDRESS_ENV: &str = "BEYOND_SLIDES_BIND_ADDRESS";
const TRUSTED_ORIGINS_ENV: &str = "BEYOND_SLIDES_TRUSTED_ORIGINS";

#[derive(Clone)]
struct App {
    root: PathBuf,
    request_policy: Arc<RequestPolicy>,
    launching: Arc<Mutex<HashSet<String>>>,
    cursors: Arc<Mutex<HashMap<PathBuf, TraceCursor>>>,
    preview_renders: Arc<tokio::sync::Semaphore>,
    recognition_estimators: Arc<Mutex<HashMap<String, recognition_eta::RecognitionEstimator>>>,
    rain_classroom: Arc<rain_classroom::RainClassroom>,
}

pub(crate) async fn serve(root: &OsStr, port: u16) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root)?;
    let root = fs::canonicalize(root)?;
    // Prevent two controllers from concurrently updating the same job metadata.
    let lock = jobs::worker_lock(&root)?;
    lock.try_lock()
        .map_err(|e| format!("another application is using {}: {e}", root.display()))?;
    let bind_address = bind_address_from_environment()?;
    let request_policy = RequestPolicy::from_environment()?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(bind_address, port)).await?;
    let port = listener.local_addr()?.port();
    let rain_classroom = Arc::new(rain_classroom::RainClassroom::new(&root));
    let app = App {
        root,
        request_policy: Arc::new(request_policy),
        launching: Arc::default(),
        cursors: Arc::default(),
        preview_renders: Arc::new(tokio::sync::Semaphore::new(2)),
        recognition_estimators: Arc::default(),
        rain_classroom,
    };
    let rain_classroom = app.rain_classroom.clone();
    let router = Router::new()
        .route(
            "/",
            get(|| async {
                Html(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/templates/application.html"
                )))
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/templates/application.js"
                    )),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/templates/application.css"
                    )),
                )
            }),
        )
        .route("/api/jobs", get(list_jobs).post(upload))
        .route("/api/codex/status", get(codex_status))
        .route("/api/rain-classroom/connect", post(connect_rain_classroom))
        .route("/api/rain-classroom/logout", post(logout_rain_classroom))
        .route(
            "/api/rain-classroom/login-view",
            get(rain_classroom_login_view),
        )
        .route("/api/rain-classroom/courses", get(rain_classroom_courses))
        .route(
            "/api/rain-classroom/courses/{classroom_id}/lectures",
            get(rain_classroom_lectures),
        )
        .route(
            "/api/rain-classroom/courses/{classroom_id}/lectures/{lesson_id}/presentations",
            get(rain_classroom_presentations),
        )
        .route(
            "/api/rain-classroom/imports/{import_id}",
            get(rain_classroom_download_progress),
        )
        .route(
            "/example/report.html",
            get(|| async { Html(EXAMPLE_REPORT) }),
        )
        .route("/api/jobs/{id}", get(job_status).delete(delete_job))
        .route("/api/jobs/{id}/slide-review", get(review_pages))
        .route("/api/jobs/{id}/slide-review/{page}", get(review_image))
        .route(
            "/api/jobs/{id}/start",
            post(start).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route("/api/jobs/{id}/stop", post(stop))
        .route("/api/jobs/{id}/export", get(export_report))
        .route("/api/jobs/{id}/logs/{kind}", get(log_tail))
        .route("/api/jobs/{id}/logs/{kind}/download", get(log_download))
        .route("/reports/{id}/{*file}", get(report_file))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024 * 1024_usize))
        .layer(middleware::from_fn_with_state(app.clone(), local_only))
        .with_state(app);
    println!("BeyondSlides application: http://{bind_address}:{port}");
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    rain_classroom.shutdown().await;
    result?;
    Ok(())
}

fn bind_address_from_environment() -> Result<IpAddr, String> {
    let Some(configured) = std::env::var_os(BIND_ADDRESS_ENV) else {
        return Ok(Ipv4Addr::LOCALHOST.into());
    };
    configured
        .to_str()
        .ok_or_else(|| format!("{BIND_ADDRESS_ENV} must be valid UTF-8"))?
        .parse()
        .map_err(|error| format!("invalid {BIND_ADDRESS_ENV}: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestPolicy {
    trusted_origins: HashSet<TrustedOrigin>,
}

impl RequestPolicy {
    fn from_environment() -> Result<Self, String> {
        let configured = match std::env::var(TRUSTED_ORIGINS_ENV) {
            Ok(configured) => configured,
            Err(std::env::VarError::NotPresent) => String::new(),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(format!("{TRUSTED_ORIGINS_ENV} must be valid UTF-8"));
            }
        };
        Self::parse(&configured)
    }

    fn parse(configured: &str) -> Result<Self, String> {
        let trusted_origins = configured
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(TrustedOrigin::parse)
            .collect::<Result<_, _>>()?;
        Ok(Self { trusted_origins })
    }

    fn allows(&self, host: &str, origin: Option<&str>) -> bool {
        let Some(authority) = RequestAuthority::parse(host) else {
            return false;
        };
        let host_is_trusted = authority.is_loopback()
            || self
                .trusted_origins
                .iter()
                .any(|trusted| trusted.matches_authority(&authority));
        if !host_is_trusted {
            return false;
        }
        let Some(origin) = origin else {
            return true;
        };
        let Ok(origin) = TrustedOrigin::parse(origin) else {
            return false;
        };
        if !origin.matches_authority(&authority) {
            return false;
        }
        origin.is_loopback() || self.trusted_origins.contains(&origin)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TrustedOrigin {
    serialized: String,
    host: String,
    port: u16,
}

impl TrustedOrigin {
    fn parse(value: &str) -> Result<Self, String> {
        let url = url::Url::parse(value)
            .map_err(|error| format!("invalid trusted origin {value:?}: {error}"))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(format!(
                "trusted origin {value:?} must contain only an http(s) scheme, host, and optional port"
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| format!("trusted origin {value:?} has no host"))?
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| format!("trusted origin {value:?} has no usable port"))?;
        Ok(Self {
            serialized: url.origin().ascii_serialization(),
            host,
            port,
        })
    }

    fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }

    fn matches_authority(&self, authority: &RequestAuthority) -> bool {
        self.host == authority.host && authority.port.is_none_or(|port| port == self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestAuthority {
    host: String,
    port: Option<u16>,
}

impl RequestAuthority {
    fn parse(value: &str) -> Option<Self> {
        let authority = axum::http::uri::Authority::from_str(value).ok()?;
        Some(Self {
            host: authority
                .host()
                .trim_matches(['[', ']'])
                .to_ascii_lowercase(),
            port: authority.port_u16(),
        })
    }

    fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

async fn local_only(State(app): State<App>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok());
    if !app.request_policy.allows(host, origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.method() != axum::http::Method::GET
        && request.method() != axum::http::Method::HEAD
        && request
            .headers()
            .get("x-beyondslides")
            .and_then(|h| h.to_str().ok())
            != Some("local-ui")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, "SAMEORIGIN".parse().unwrap());
    response
}

#[derive(Debug)]
struct AppError(StatusCode, String);
impl AppError {
    fn bad(error: impl std::fmt::Display) -> Self {
        Self(StatusCode::BAD_REQUEST, error.to_string())
    }
    fn conflict(error: impl std::fmt::Display) -> Self {
        Self(StatusCode::CONFLICT, error.to_string())
    }
}
impl From<io::Error> for AppError {
    fn from(error: io::Error) -> Self {
        let status = match error.kind() {
            io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(root: &FsPath) -> App {
        App {
            root: root.into(),
            request_policy: Arc::new(RequestPolicy::parse("").unwrap()),
            launching: Arc::default(),
            cursors: Arc::default(),
            preview_renders: Arc::new(tokio::sync::Semaphore::new(2)),
            recognition_estimators: Arc::default(),
            rain_classroom: Arc::new(rain_classroom::RainClassroom::new(root)),
        }
    }

    #[test]
    fn loopback_requests_accept_arbitrary_external_ports_but_not_cross_origin_ports() {
        let policy = RequestPolicy::parse("").unwrap();

        assert!(policy.allows("127.0.0.1:80", None));
        assert!(policy.allows("127.0.0.1:8080", Some("http://127.0.0.1:8080")));
        assert!(policy.allows("[::1]:49152", Some("http://[::1]:49152")));
        assert!(!policy.allows("127.0.0.1:7842", Some("http://127.0.0.1:80")));
        assert!(!policy.allows("example.com:7842", None));
    }

    #[test]
    fn configured_origins_authorize_their_host_and_exact_browser_origin() {
        let policy =
            RequestPolicy::parse("https://reader.example:8443, http://lecture.internal:8080/")
                .unwrap();

        assert!(policy.allows("reader.example:8443", Some("https://reader.example:8443")));
        assert!(policy.allows("lecture.internal:8080", None));
        assert!(!policy.allows("reader.example:8443", Some("https://reader.example:9443")));
        assert!(!policy.allows("reader.example:80", None));
    }

    #[test]
    fn trusted_origin_configuration_rejects_paths_credentials_and_non_http_schemes() {
        for invalid in [
            "https://reader.example/path",
            "https://user@reader.example",
            "file:///tmp/report",
        ] {
            assert!(RequestPolicy::parse(invalid).is_err(), "accepted {invalid}");
        }
    }

    fn save_test_job(root: &FsPath, id: &str) -> PathBuf {
        let directory = root.join(id);
        fs::create_dir_all(&directory).unwrap();
        let job: Job = serde_json::from_value(json!({
            "id": id, "name": "Deletion regression", "created_ms": 1,
            "preview": {"slide_count": 1, "segment_count": 0, "duration_ms": null,
                "transcript_sample": "", "slide_sample": "", "warnings": []},
            "recording": null, "transcribe_recording": false,
            "runs": []
        }))
        .unwrap();
        save_job(&directory, &job).unwrap();
        directory
    }

    #[tokio::test]
    async fn live_asr_progress_produces_recognition_eta_without_changing_worker_files() {
        let root = tempfile::tempdir().unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let directory = root.path().join(id);
        let control = directory.join("run-0001/control");
        fs::create_dir_all(&control).unwrap();
        let job: Job = serde_json::from_value(json!({
            "id": id, "name": "ETA regression", "created_ms": 1,
            "preview": {"slide_count": 1, "segment_count": 0, "duration_ms": null,
                "transcript_sample": "", "slide_sample": "", "warnings": []},
            "recording": "recording.mp4", "transcribe_recording": true,
            "runs": [{"number": 1, "started_ms": 10, "elapsed_before_ms": 0,
                "settings": {"backend": "openai_compatible", "base_url": "http://localhost/v1", "model": "test", "extra_body": null,
                    "max_concurrency": 2, "request_interval_ms": 0, "adaptive": false, "boundary_passages": true}}]
        })).unwrap();
        save_job(&directory, &job).unwrap();
        let lock = jobs::worker_lock(&directory).unwrap();
        lock.try_lock().unwrap();
        let app = test_app(root.path());
        let mut observed = json!({"phase": "recognizing", "attempt_started_ms": 10,
            "completed_regions": 840, "total_regions": 1330,
            "completed_speech_ms": 1804000, "total_speech_ms": 6277000});
        let path = control.join("asr-progress.json");
        write_json_atomically(&path, &observed, "test progress").unwrap();
        let first = job_status(State(app.clone()), Path(id.into()))
            .await
            .unwrap()
            .0;
        assert_eq!(first.state, "running");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        observed["completed_speech_ms"] = json!(1805000);
        write_json_atomically(&path, &observed, "test progress").unwrap();
        let before = fs::read(&path).unwrap();
        let status = job_status(State(app.clone()), Path(id.into()))
            .await
            .unwrap()
            .0;
        let value = serde_json::to_value(status).unwrap();
        assert!(
            value["transcription"]["recognition_eta_ms"]
                .as_u64()
                .is_some_and(|eta| eta > 0),
            "live speech progress must reach indicatif: {value}"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        observed["phase"] = json!("finalizing");
        write_json_atomically(&path, &observed, "test progress").unwrap();
        let status = job_status(State(app), Path(id.into())).await.unwrap().0;
        assert!(
            serde_json::to_value(status).unwrap()["transcription"]["recognition_eta_ms"].is_null()
        );
    }

    #[tokio::test]
    async fn deleting_a_lecture_removes_only_its_job_directory() {
        let root = tempfile::tempdir().unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let directory = save_test_job(root.path(), id);
        let unrelated = root.path().join("keep-me");
        fs::create_dir(&unrelated).unwrap();
        fs::write(directory.join("saved-result.json"), b"{}\n").unwrap();

        let _ = delete_job(State(test_app(root.path())), Path(id.into()))
            .await
            .unwrap();

        assert!(!directory.exists());
        assert!(unrelated.exists());
    }

    #[tokio::test]
    async fn deleting_a_launching_lecture_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let directory = save_test_job(root.path(), id);
        let app = test_app(root.path());
        app.launching.lock().await.insert(id.into());

        let error = delete_job(State(app), Path(id.into())).await.unwrap_err();

        assert_eq!(error.0, StatusCode::CONFLICT);
        assert!(directory.exists());
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

async fn list_jobs(State(app): State<App>) -> Result<Json<Vec<Job>>, AppError> {
    let mut jobs = Vec::new();
    for entry in fs::read_dir(&app.root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && job_path(&app.root, &entry.file_name().to_string_lossy()).is_ok()
            && entry.path().join("job.json").is_file()
        {
            jobs.push(read_job(&entry.path())?);
        }
    }
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_ms));
    Ok(Json(jobs))
}

async fn delete_job(
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Holding the launch guard across deletion prevents a concurrent Start
    // request from validating the job immediately before its files disappear.
    let launching = app.launching.lock().await;
    let directory = job_path(&app.root, &id)?;
    read_job(&directory)?;
    if launching.contains(&id) || is_worker_running(&directory)? {
        return Err(AppError::conflict(
            "Stop processing before deleting this lecture",
        ));
    }
    fs::remove_dir_all(&directory)?;
    app.recognition_estimators.lock().await.remove(&id);
    app.cursors
        .lock()
        .await
        .retain(|path, _| !path.starts_with(&directory));
    Ok(Json(json!({"deleted": true})))
}

async fn connect_rain_classroom(
    State(app): State<App>,
) -> Result<Json<serde_json::Value>, AppError> {
    app.rain_classroom.connect().await.map_err(AppError::bad)?;
    Ok(Json(json!({"opened": true})))
}

async fn logout_rain_classroom(
    State(app): State<App>,
) -> Result<Json<serde_json::Value>, AppError> {
    app.rain_classroom.logout().await.map_err(AppError::bad)?;
    Ok(Json(json!({"logged_out": true})))
}

async fn rain_classroom_courses(
    State(app): State<App>,
) -> Result<Json<Vec<rain_classroom::Course>>, AppError> {
    Ok(Json(
        app.rain_classroom.courses().await.map_err(AppError::bad)?,
    ))
}

async fn rain_classroom_login_view(State(app): State<App>) -> Result<Response, AppError> {
    let image = app
        .rain_classroom
        .login_view()
        .await
        .map_err(AppError::bad)?;
    Ok(([(header::CONTENT_TYPE, "image/png")], image).into_response())
}

async fn rain_classroom_lectures(
    State(app): State<App>,
    Path(classroom_id): Path<u64>,
) -> Result<Json<Vec<rain_classroom::Lecture>>, AppError> {
    Ok(Json(
        app.rain_classroom
            .lectures(classroom_id)
            .await
            .map_err(AppError::bad)?,
    ))
}

async fn rain_classroom_presentations(
    State(app): State<App>,
    Path((classroom_id, lesson_id)): Path<(u64, String)>,
) -> Result<Json<Vec<rain_classroom::Presentation>>, AppError> {
    Ok(Json(
        app.rain_classroom
            .presentations(&rain_classroom::LectureSelection {
                classroom_id,
                lesson_id,
            })
            .await
            .map_err(AppError::bad)?,
    ))
}

async fn rain_classroom_download_progress(
    State(app): State<App>,
    Path(import_id): Path<String>,
) -> Result<Json<rain_classroom::DownloadProgress>, AppError> {
    app.rain_classroom
        .download_progress(&import_id)
        .map(Json)
        .ok_or_else(|| {
            AppError(
                StatusCode::NOT_FOUND,
                "Rain Classroom import not found".into(),
            )
        })
}

fn log_path(app: &App, id: &str, kind: &str) -> Result<PathBuf, AppError> {
    let directory = job_path(&app.root, id)?;
    let job = read_job(&directory)?;
    let run = job
        .runs
        .last()
        .ok_or_else(|| AppError::bad("No processing run yet"))?;
    Ok(logs::path(&directory, &run.directory(&directory), kind)?)
}

async fn log_tail(
    State(app): State<App>,
    Path((id, kind)): Path<(String, String)>,
) -> Result<Json<logs::Tail>, AppError> {
    let path = log_path(&app, &id, &kind)?;
    let tail = tokio::task::spawn_blocking(move || logs::tail(&path))
        .await
        .map_err(AppError::bad)??;
    Ok(Json(tail))
}

async fn log_download(
    State(app): State<App>,
    Path((id, kind)): Path<(String, String)>,
    request: Request,
) -> Result<Response, AppError> {
    let path = log_path(&app, &id, &kind)?;
    let mut response = ServeFile::new(path)
        .try_call(request)
        .await
        .map_err(AppError::bad)?
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "text/plain; charset=utf-8".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{kind}-debug.log\"")
            .parse()
            .unwrap(),
    );
    Ok(response)
}

async fn upload(State(app): State<App>, mut multipart: Multipart) -> Result<Json<Job>, AppError> {
    let directory = tempfile::tempdir_in(&app.root)?;
    let mut fields = HashSet::new();
    let mut title = "Untitled lecture".to_owned();
    let mut extension = String::new();
    let mut recording = None;
    let mut rain_slides = None;
    let mut rain_recording = None;
    let mut rain_import_id = None;
    while let Some(mut field) = multipart.next_field().await.map_err(AppError::bad)? {
        let name = field.name().unwrap_or("").to_owned();
        if !fields.insert(name.clone()) {
            return Err(AppError::bad("Duplicate upload field"));
        }
        if name == "name" {
            let mut bytes = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(AppError::bad)? {
                if bytes.len() + chunk.len() > 256 {
                    return Err(AppError::bad("Lecture name is too long"));
                }
                bytes.extend_from_slice(&chunk);
            }
            title = String::from_utf8(bytes)
                .map_err(AppError::bad)?
                .trim()
                .to_owned();
            if title.is_empty() {
                title = "Untitled lecture".into();
            }
            continue;
        }
        if matches!(name.as_str(), "rain_slides" | "rain_recording") {
            let mut bytes = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(AppError::bad)? {
                if bytes.len() + chunk.len() > 512 {
                    return Err(AppError::bad("Invalid Rain Classroom selection"));
                }
                bytes.extend_from_slice(&chunk);
            }
            if name == "rain_slides" {
                rain_slides = Some(
                    serde_json::from_slice::<rain_classroom::PresentationSelection>(&bytes)
                        .map_err(AppError::bad)?,
                );
            } else {
                rain_recording = Some(
                    serde_json::from_slice::<rain_classroom::LectureSelection>(&bytes)
                        .map_err(AppError::bad)?,
                );
            }
            continue;
        }
        if name == "rain_import_id" {
            let bytes = field.bytes().await.map_err(AppError::bad)?;
            if bytes.len() > 64 {
                return Err(AppError::bad(
                    "Invalid Rain Classroom import progress identifier",
                ));
            }
            rain_import_id = Some(String::from_utf8(bytes.to_vec()).map_err(AppError::bad)?);
            continue;
        }
        let ext = FsPath::new(field.file_name().unwrap_or(""))
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (filename, limit) = match name.as_str() {
            "slides" if ext == "pdf" => ("slides.pdf".into(), 100 * 1024 * 1024),
            "transcript" if matches!(ext.as_str(), "json" | "srt" | "vtt" | "txt") => {
                extension = ext;
                ("transcript-upload".into(), 16 * 1024 * 1024)
            }
            "recording"
                if ["mp4", "webm", "mp3", "m4a", "wav", "flac", "ogg"].contains(&ext.as_str()) =>
            {
                let file = format!("recording.{ext}");
                recording = Some(file.clone());
                (file, 4 * 1024 * 1024 * 1024_usize)
            }
            _ => {
                return Err(AppError::bad(
                    "Expected slides.pdf, transcript.json/.srt/.vtt/.txt, and an optional audio/video recording",
                ));
            }
        };
        let mut output = tokio::fs::File::create(directory.path().join(filename)).await?;
        let mut size = 0;
        while let Some(chunk) = field.chunk().await.map_err(AppError::bad)? {
            size += chunk.len();
            if size > limit {
                return Err(AppError::bad(format!(
                    "{name} exceeds its upload size limit"
                )));
            }
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
    }
    if rain_recording.is_some() && (fields.contains("transcript") || recording.is_some()) {
        return Err(AppError::bad(
            "Choose either a Rain Classroom lecture or local transcript/recording files",
        ));
    }
    let uses_rain = rain_slides.is_some() || rain_recording.is_some();
    let mut rain_slide_ocr = None;
    if uses_rain != rain_import_id.is_some() {
        return Err(AppError::bad(
            "Rain Classroom imports require a progress identifier",
        ));
    }
    if fields.contains("slides") == rain_slides.is_some() {
        return Err(AppError::bad(
            "Choose exactly one slide source: a PDF upload or Rain Classroom courseware",
        ));
    }
    if !fields.contains("transcript") && recording.is_none() && rain_recording.is_none() {
        return Err(AppError::bad(
            "Upload slides and provide a transcript, a recording, or a Rain Classroom lecture",
        ));
    }
    if let Some(import_id) = rain_import_id {
        let acquired = app
            .rain_classroom
            .acquire_sources(
                rain_slides.as_ref(),
                rain_recording.as_ref(),
                directory.path(),
                &import_id,
            )
            .await
            .map_err(AppError::bad)?;
        rain_slide_ocr = acquired.slide_ocr;
        if rain_recording.is_some() {
            recording = Some("recording.mp4".into());
        }
    }
    let root = app.root.clone();
    let job = tokio::task::spawn_blocking(move || {
        let id = format!("{:032x}", fastrand::u128(..));
        let job = jobs::import_job(
            directory.path(),
            id.clone(),
            title,
            &extension,
            recording,
            rain_slide_ocr.as_deref(),
        )?;
        fs::rename(directory.path(), root.join(id)).map_err(|e| e.to_string())?;
        Ok::<_, String>(job)
    })
    .await
    .map_err(AppError::bad)?
    .map_err(AppError::bad)?;
    Ok(Json(job))
}

#[derive(Serialize)]
struct Status {
    job: Job,
    state: String,
    progress: WorkerProgress,
    usage: Usage,
    usage_error: Option<String>,
    error: Option<String>,
    elapsed_ms: u64,
    report_url: Option<String>,
    transcription: Option<transcription::Progress>,
}

async fn job_status(
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<Status>, AppError> {
    let directory = job_path(&app.root, &id)?;
    let job = read_job(&directory)?;
    let mut status = Status {
        job: job.clone(),
        state: "ready".into(),
        progress: WorkerProgress::default(),
        usage: Usage::default(),
        usage_error: None,
        error: None,
        elapsed_ms: 0,
        report_url: None,
        transcription: None,
    };
    if let Some(run) = job.runs.last() {
        let path = run.directory(&directory);
        let running = app.launching.lock().await.contains(&id) || is_worker_running(&directory)?;
        {
            // Serialize observation reads too: simultaneous browser polls must
            // not feed an older snapshot after a newer one.
            let mut estimators = app.recognition_estimators.lock().await;
            status.transcription = transcription::read_progress(&path, run.started_ms)?;
            let eta = estimators
                .entry(id.clone())
                .or_default()
                .observe(status.transcription.as_ref(), running);
            if let Some(progress) = &mut status.transcription {
                progress.recognition_eta_ms = eta;
            }
        }
        let outcome: Option<Outcome> = if path.join("outcome.json").exists() {
            Some(read_json(&path.join("outcome.json"), "worker outcome")?)
        } else {
            None
        };
        status.state = if running {
            if path.join("control/stop-requested").exists() {
                "stopping"
            } else {
                "running"
            }
        } else {
            match outcome.as_ref().map(|o| &o.status) {
                Some(OutcomeStatus::Complete) => "complete",
                Some(OutcomeStatus::Paused) => "paused",
                Some(OutcomeStatus::Failed) => "failed",
                None => "interrupted",
            }
        }
        .into();
        status.elapsed_ms = outcome.as_ref().map_or_else(
            || {
                if running {
                    Ok(run.elapsed_before_ms + now_ms().saturating_sub(run.started_ms))
                } else {
                    run.checkpointed_elapsed(&directory)
                }
            },
            |o| Ok(o.elapsed_ms),
        )?;
        status.error = outcome.and_then(|o| o.error);
        if status.state == "interrupted" {
            status.error = Some(
                "Worker exited without a final outcome. Validated checkpoints are preserved."
                    .into(),
            );
        }
        if path.join("control/progress.json").exists() {
            status.progress = read_json(&path.join("control/progress.json"), "worker progress")?;
        }
        let mut cursors = app.cursors.lock().await;
        for trace in [
            path.join("analysis/model-trace.jsonl"),
            path.join("analysis/restoration/model-trace.jsonl"),
        ] {
            match cursors
                .entry(trace.clone())
                .or_default()
                .poll(&trace, run.started_ms, running)
            {
                Ok(usage) => status.usage.add(usage),
                Err(error) => {
                    status.usage_error = Some(format!("Token telemetry is incomplete: {error}"))
                }
            }
        }
        if status.state == "complete" && path.join("analysis/report.html").is_file() {
            status.report_url = Some(format!("/reports/{id}/report.html"));
        }
    }
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    settings: Settings,
    api_key: String,
    #[serde(default)]
    confirm_reprocessing: bool,
}

async fn codex_status() -> Result<Json<serde_json::Value>, AppError> {
    let version = tokio::process::Command::new("codex")
        .arg("--version")
        .output()
        .await
        .map_err(|error| AppError::bad(format!("could not run codex: {error}")))?;
    if !version.status.success() {
        return Err(AppError::bad("codex --version failed"));
    }
    let login = tokio::process::Command::new("codex")
        .args(["login", "status"])
        .output()
        .await
        .map_err(|error| AppError::bad(format!("could not inspect Codex login: {error}")))?;
    let logged_in = login.status.success();
    let (models, model_error) = if logged_in {
        match beyond_slides::discover_codex_models("codex").await {
            Ok(models) => (models, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        }
    } else {
        (Vec::new(), None)
    };
    Ok(Json(json!({
        "version": String::from_utf8_lossy(&version.stdout).trim(),
        "logged_in": logged_in,
        "models": models,
        "model_error": model_error,
    })))
}

async fn start(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(request): Json<StartRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    request
        .settings
        .validate(&request.api_key)
        .map_err(AppError::bad)?;
    let mut launching = app.launching.lock().await;
    let directory = job_path(&app.root, &id)?;
    let mut job = read_job(&directory)?;
    if launching.contains(&id) || is_worker_running(&directory)? {
        return Err(AppError::conflict("This lecture is already processing"));
    }
    let previous = job.runs.last().cloned();
    let new_revision = previous
        .as_ref()
        .is_some_and(|r| !r.settings.same_analysis(&request.settings));
    if new_revision && !request.confirm_reprocessing {
        let reuse_restoration = previous
            .as_ref()
            .is_some_and(|r| r.settings.same_restoration(&request.settings));
        return Err(AppError::conflict(if reuse_restoration {
            "Passage preparation changed: keep restoration; rerun passage preparation, comparisons and rendering. Previous results will be preserved. Confirm to continue."
        } else {
            "Provider, model or request options changed: rerun restoration and all downstream stages in a new revision. Previous results will be preserved. Confirm to continue."
        }));
    }
    let mut run = if new_revision || previous.is_none() {
        Run {
            number: job.runs.len() + 1,
            settings: request.settings,
            started_ms: now_ms(),
            elapsed_before_ms: 0,
        }
    } else {
        let mut run = previous.clone().unwrap();
        let outcome_path = run.directory(&directory).join("outcome.json");
        if outcome_path.exists() {
            let outcome: Outcome = read_json(&outcome_path, "worker outcome")?;
            if matches!(outcome.status, OutcomeStatus::Complete) {
                return Err(AppError::conflict("This run is complete; open its report"));
            }
            run.elapsed_before_ms = outcome.elapsed_ms;
            fs::rename(
                &outcome_path,
                run.directory(&directory)
                    .join(format!("outcome-{}.json", run.started_ms)),
            )?;
        } else {
            run.elapsed_before_ms = run.checkpointed_elapsed(&directory)?;
        }
        run.settings = request.settings;
        run
    };
    run.started_ms = now_ms().max(run.started_ms + 1);
    let path = run.directory(&directory);
    fs::create_dir_all(path.join("control"))?;
    if path.join("control/stop-requested").exists() {
        fs::remove_file(path.join("control/stop-requested"))?;
    }
    if new_revision
        && let Some(previous) = &previous
        && previous.settings.same_restoration(&run.settings)
    {
        jobs::copy_restoration(
            &previous.directory(&directory).join("analysis/restoration"),
            &path.join("analysis/restoration"),
        )?;
    }
    if new_revision || job.runs.is_empty() {
        job.runs.push(run.clone());
    } else {
        *job.runs.last_mut().unwrap() = run.clone();
    }
    save_job(&directory, &job)?;
    // A resumed attempt must not retain active-exchange state from a dead worker.
    app.cursors
        .lock()
        .await
        .retain(|p, _| !p.starts_with(&path));
    let log = fs::File::options()
        .create(true)
        .append(true)
        .open(path.join("worker.log"))?;
    let runtime_tools_directory =
        std::env::var_os(beyond_slides::runtime_tools::RUNTIME_TOOLS_DIRECTORY_ENV);
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("application-worker")
        .arg(&directory)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    for (name, _) in
        std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("BEYOND_SLIDES_"))
    {
        command.env_remove(name);
    }
    command
        .env(
            "BEYOND_SLIDES_MODEL_BACKEND",
            match run.settings.backend {
                crate::run_support::ModelBackendKind::OpenAiCompatible => "openai_compatible",
                crate::run_support::ModelBackendKind::Codex => "codex",
            },
        )
        .env("BEYOND_SLIDES_MODEL", &run.settings.model)
        .env(
            "BEYOND_SLIDES_MAX_CONCURRENCY",
            run.settings.max_concurrency.to_string(),
        )
        .env(
            "BEYOND_SLIDES_REQUEST_INTERVAL_MS",
            run.settings.request_interval_ms.to_string(),
        )
        .env(
            "BEYOND_SLIDES_SCHEDULING",
            if run.settings.adaptive {
                "adaptive"
            } else {
                "fixed"
            },
        )
        .env("BEYOND_SLIDES_WORKER_CONTROL", path.join("control"));
    if let Some(runtime_tools_directory) = runtime_tools_directory {
        command.env(
            beyond_slides::runtime_tools::RUNTIME_TOOLS_DIRECTORY_ENV,
            runtime_tools_directory,
        );
    }
    if let Some(initial_concurrency) = run.settings.initial_concurrency {
        command.env(
            "BEYOND_SLIDES_INITIAL_CONCURRENCY",
            initial_concurrency.to_string(),
        );
    }
    if run.settings.backend == crate::run_support::ModelBackendKind::OpenAiCompatible {
        command
            .env("BEYOND_SLIDES_API_BASE_URL", &run.settings.base_url)
            .env("BEYOND_SLIDES_API_KEY", request.api_key)
            .env(
                "BEYOND_SLIDES_CHAT_EXTRA_BODY",
                run.settings
                    .extra_body
                    .as_ref()
                    .unwrap_or(&json!({}))
                    .to_string(),
            );
    } else {
        if let Some(reasoning_effort) = &run.settings.codex_reasoning_effort {
            command.env("BEYOND_SLIDES_CODEX_REASONING_EFFORT", reasoning_effort);
        }
        if let Some(service_tier) = &run.settings.codex_service_tier {
            command.env("BEYOND_SLIDES_CODEX_SERVICE_TIER", service_tier);
        }
    }
    // A terminal Ctrl+C should stop the controller, not the independently owned
    // worker and its log supervisor. They retain graceful stop-file control.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    launching.insert(id.clone());
    let app_clone = app.clone();
    tokio::spawn(async move {
        let result = child.wait().await;
        if !path.join("outcome.json").exists() {
            let _ = write_json_atomically(
                &path.join("outcome.json"),
                &Outcome {
                    status: OutcomeStatus::Failed,
                    error: Some(format!(
                        "Worker exited before recording a result: {result:?}. See the local worker log."
                    )),
                    finished_ms: now_ms(),
                    elapsed_ms: run.elapsed_before_ms + now_ms().saturating_sub(run.started_ms),
                },
                "worker outcome",
            );
        }
        app_clone.launching.lock().await.remove(&id);
    });
    Ok(Json(json!({"started": true})))
}

async fn stop(
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let launching = app.launching.lock().await;
    let directory = job_path(&app.root, &id)?;
    let job = read_job(&directory)?;
    if !launching.contains(&id) && !is_worker_running(&directory)? {
        return Err(AppError::conflict("This lecture is not running"));
    }
    let run = job
        .runs
        .last()
        .ok_or_else(|| AppError::bad("No run to stop"))?;
    fs::write(run.directory(&directory).join("control/stop-requested"), [])?;
    Ok(Json(json!({"stopping": true})))
}

async fn review_pages(
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<Vec<slide_review::ReviewPage>>, AppError> {
    let directory = job_path(&app.root, &id)?;
    read_job(&directory)?;
    Ok(Json(slide_review::pages(&directory)?))
}

async fn review_image(
    State(app): State<App>,
    Path((id, page)): Path<(String, u32)>,
) -> Result<Response, AppError> {
    let directory = job_path(&app.root, &id)?;
    read_job(&directory)?;
    let image = slide_review::image(&directory, page, &app.preview_renders).await?;
    Ok(([(header::CONTENT_TYPE, "image/png")], image).into_response())
}

async fn report_file(
    State(app): State<App>,
    Path((id, file)): Path<(String, String)>,
    request: Request,
) -> Result<Response, AppError> {
    if !(file == "report.html" || file.starts_with("report.assets/"))
        || !FsPath::new(&file)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(AppError(
            StatusCode::NOT_FOUND,
            "Unknown report asset".into(),
        ));
    }
    let directory = job_path(&app.root, &id)?;
    let job = read_job(&directory)?;
    let run = job
        .runs
        .last()
        .ok_or_else(|| AppError::bad("No report yet"))?;
    let root = run.directory(&directory).join("analysis").canonicalize()?;
    let path = root.join(file).canonicalize()?;
    if !path.starts_with(&root) {
        return Err(AppError::bad("Invalid asset path"));
    }
    Ok(ServeFile::new(path)
        .try_call(request)
        .await?
        .into_response())
}

async fn export_report(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    let directory = job_path(&app.root, &id)?;
    let job = read_job(&directory)?;
    let run = job
        .runs
        .last()
        .ok_or_else(|| AppError::bad("No completed report to export"))?;
    let path = run.directory(&directory);
    let outcome: Outcome = read_json(&path.join("outcome.json"), "worker outcome")?;
    if !matches!(outcome.status, OutcomeStatus::Complete) {
        return Err(AppError::conflict(
            "Wait for a complete report before exporting",
        ));
    }
    let destination = path.join("reader.zip");
    if !destination.is_file() {
        let target = destination.clone();
        tokio::task::spawn_blocking(move || export::build(&path.join("analysis"), &target))
            .await
            .map_err(AppError::bad)??;
    }
    let mut response = ServeFile::new(destination)
        .try_call(request)
        .await?
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        "attachment; filename=BeyondSlides-lecture.zip"
            .parse()
            .unwrap(),
    );
    Ok(response)
}
