mod export;
mod jobs;
mod transcription;
mod usage;
pub(crate) mod worker;

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::OsStr,
    fs, io,
    path::{Component, Path as FsPath, PathBuf},
    process::Stdio,
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

#[derive(Clone)]
struct App {
    root: PathBuf,
    port: u16,
    launching: Arc<Mutex<HashSet<String>>>,
    cursors: Arc<Mutex<HashMap<PathBuf, TraceCursor>>>,
}

pub(crate) async fn serve(root: &OsStr, port: u16) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root)?;
    let root = fs::canonicalize(root)?;
    // Prevent two controllers from concurrently updating the same job metadata.
    let lock = jobs::worker_lock(&root)?;
    lock.try_lock()
        .map_err(|e| format!("another application is using {}: {e}", root.display()))?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    let app = App {
        root,
        port,
        launching: Arc::default(),
        cursors: Arc::default(),
    };
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
        .route("/api/jobs/{id}", get(job_status))
        .route(
            "/api/jobs/{id}/start",
            post(start).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route("/api/jobs/{id}/stop", post(stop))
        .route("/api/jobs/{id}/export", get(export_report))
        .route("/reports/{id}/{*file}", get(report_file))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024 * 1024_usize))
        .layer(middleware::from_fn_with_state(app.clone(), local_only))
        .with_state(app);
    println!("BeyondSlides application: http://127.0.0.1:{port}");
    axum::serve(listener, router).await?;
    Ok(())
}

async fn local_only(State(app): State<App>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let allowed = [
        format!("127.0.0.1:{}", app.port),
        format!("localhost:{}", app.port),
    ];
    if !allowed.iter().any(|h| h == host) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
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

async fn upload(State(app): State<App>, mut multipart: Multipart) -> Result<Json<Job>, AppError> {
    let directory = tempfile::tempdir_in(&app.root)?;
    let mut fields = HashSet::new();
    let mut title = "Untitled lecture".to_owned();
    let mut extension = String::new();
    let mut recording = None;
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
        let ext = FsPath::new(field.file_name().unwrap_or(""))
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (filename, limit) = match name.as_str() {
            "slides" if ext == "pdf" => ("slides.pdf".into(), 100 * 1024 * 1024),
            "transcript" if matches!(ext.as_str(), "json" | "tsv") => {
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
                    "Expected slides.pdf, transcript.json/.tsv, and an optional audio/video recording",
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
    if !fields.contains("slides") || (!fields.contains("transcript") && recording.is_none()) {
        return Err(AppError::bad(
            "Upload slides and either a transcript or a recording",
        ));
    }
    let root = app.root.clone();
    let job = tokio::task::spawn_blocking(move || {
        let id = format!("{:032x}", fastrand::u128(..));
        let job = jobs::import_job(directory.path(), id.clone(), title, &extension, recording)?;
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
    };
    if let Some(run) = job.runs.last() {
        let path = run.directory(&directory);
        let running = app.launching.lock().await.contains(&id) || is_worker_running(&directory)?;
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
            || run.elapsed_before_ms + now_ms().saturating_sub(run.started_ms),
            |o| o.elapsed_ms,
        );
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
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    let asr_python = std::env::var_os("BEYOND_SLIDES_ASR_PYTHON");
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
    if let Some(python) = asr_python {
        command.env("BEYOND_SLIDES_ASR_PYTHON", python);
    }
    command
        .env("BEYOND_SLIDES_API_BASE_URL", &run.settings.base_url)
        .env("BEYOND_SLIDES_MODEL", &run.settings.model)
        .env("BEYOND_SLIDES_API_KEY", request.api_key)
        .env(
            "BEYOND_SLIDES_CHAT_EXTRA_BODY",
            run.settings
                .extra_body
                .as_ref()
                .unwrap_or(&json!({}))
                .to_string(),
        )
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
        .env(
            "BEYOND_SLIDES_PASSAGE_PREPARATION",
            if run.settings.boundary_passages {
                "boundaries"
            } else {
                "windows"
            },
        )
        .env("BEYOND_SLIDES_WORKER_CONTROL", path.join("control"));
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
