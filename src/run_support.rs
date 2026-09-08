use std::{
    env, fs,
    io::{self, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{
    ChatCompletionsConfig, ChatCompletionsConfigError, CodexAppServerConfig, LectureModelBackend,
    ModelExchangeTrace, RequestScheduler,
};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const API_BASE_URL_ENV: &str = "BEYOND_SLIDES_API_BASE_URL";
const API_KEY_ENV: &str = "BEYOND_SLIDES_API_KEY";
const MODEL_ENV: &str = "BEYOND_SLIDES_MODEL";
const MODEL_BACKEND_ENV: &str = "BEYOND_SLIDES_MODEL_BACKEND";
const CODEX_REASONING_EFFORT_ENV: &str = "BEYOND_SLIDES_CODEX_REASONING_EFFORT";
const CODEX_SERVICE_TIER_ENV: &str = "BEYOND_SLIDES_CODEX_SERVICE_TIER";
const CHAT_EXTRA_BODY_ENV: &str = "BEYOND_SLIDES_CHAT_EXTRA_BODY";
const ANNOTATION_CHAT_EXTRA_BODY_ENV: &str = "BEYOND_SLIDES_ANNOTATION_CHAT_EXTRA_BODY";
const RESTORATION_CHAT_EXTRA_BODY_ENV: &str = "BEYOND_SLIDES_RESTORATION_CHAT_EXTRA_BODY";
const MANIFEST_FILE: &str = "manifest.json";
const MODEL_TRACE_FILE: &str = "model-trace.jsonl";
const RUN_LOCK_FILE: &str = ".run.lock";

/// Hold the returned file for the entire mutating run. Never unlink a lock
/// file: another process may already be waiting on that same inode.
pub(crate) fn lock_run_directory(directory: &Path) -> Result<fs::File, io::Error> {
    fs::create_dir_all(directory)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(RUN_LOCK_FILE))?;
    lock.try_lock().map_err(|error| {
        io::Error::other(format!(
            "run directory {} is already being written or cannot be locked: {error}",
            directory.display()
        ))
    })?;
    Ok(lock)
}

pub(crate) struct ProviderSettings {
    pub(crate) worker: crate::worker_control::WorkerControl,
    backend: ModelBackendKind,
    base_url: Option<String>,
    api_key: Option<String>,
    model: String,
    codex_reasoning_effort: Option<String>,
    codex_service_tier: Option<String>,
    extra_body: Option<Value>,
    execution: ExecutionSettings,
    scheduler: RequestScheduler,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelBackendKind {
    #[default]
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    Codex,
}

/// Operational settings do not change the identity of a validated checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ExecutionSettings {
    mode: SchedulingMode,
    initial_concurrency: NonZeroUsize,
    max_concurrency: NonZeroUsize,
    request_interval_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SchedulingMode {
    Fixed,
    Adaptive,
}

impl ExecutionSettings {
    fn parse(
        mode: Option<&str>,
        initial_concurrency: Option<&str>,
        concurrency: Option<&str>,
        interval_ms: Option<&str>,
    ) -> Result<Self, io::Error> {
        let mode = match mode.unwrap_or("adaptive") {
            "adaptive" => SchedulingMode::Adaptive,
            "fixed" => SchedulingMode::Fixed,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid BEYOND_SLIDES_SCHEDULING: expected adaptive or fixed",
                ));
            }
        };
        let invalid = |name| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "invalid {name}: expected an unsigned integer{}",
                    if matches!(
                        name,
                        "BEYOND_SLIDES_INITIAL_CONCURRENCY" | "BEYOND_SLIDES_MAX_CONCURRENCY"
                    ) {
                        " greater than zero"
                    } else {
                        ""
                    }
                ),
            )
        };
        let max_concurrency = concurrency
            .unwrap_or(if mode == SchedulingMode::Adaptive {
                "8"
            } else {
                "2"
            })
            .parse()
            .map_err(|_| invalid("BEYOND_SLIDES_MAX_CONCURRENCY"))?;
        let initial_concurrency = initial_concurrency
            .unwrap_or("2")
            .parse::<NonZeroUsize>()
            .map_err(|_| invalid("BEYOND_SLIDES_INITIAL_CONCURRENCY"))?;
        if initial_concurrency > max_concurrency {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "BEYOND_SLIDES_INITIAL_CONCURRENCY cannot exceed BEYOND_SLIDES_MAX_CONCURRENCY",
            ));
        }
        Ok(Self {
            mode,
            initial_concurrency,
            max_concurrency,
            request_interval_ms: interval_ms
                .unwrap_or("0")
                .parse()
                .map_err(|_| invalid("BEYOND_SLIDES_REQUEST_INTERVAL_MS"))?,
        })
    }

    fn scheduler(self) -> RequestScheduler {
        let interval = Duration::from_millis(self.request_interval_ms);
        match self.mode {
            SchedulingMode::Fixed => RequestScheduler::fixed(self.max_concurrency, interval),
            SchedulingMode::Adaptive => RequestScheduler::adaptive_starting_at(
                self.initial_concurrency,
                self.max_concurrency,
                interval,
            ),
        }
    }
}

impl ProviderSettings {
    pub(crate) fn from_annotation_environment() -> Result<Self, io::Error> {
        Self::from_environment(ANNOTATION_CHAT_EXTRA_BODY_ENV)
    }

    pub(crate) fn from_restoration_environment() -> Result<Self, io::Error> {
        Self::from_environment(RESTORATION_CHAT_EXTRA_BODY_ENV)
    }

    fn from_environment(stage_extra_body_env: &str) -> Result<Self, io::Error> {
        let backend = match optional_environment_variable(MODEL_BACKEND_ENV)?.as_deref() {
            None | Some("openai_compatible") => ModelBackendKind::OpenAiCompatible,
            Some("codex") => ModelBackendKind::Codex,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid BEYOND_SLIDES_MODEL_BACKEND: expected openai_compatible or codex",
                ));
            }
        };
        let extra_body = match backend {
            ModelBackendKind::Codex => None,
            ModelBackendKind::OpenAiCompatible => {
                match optional_json_object(stage_extra_body_env)? {
                    Some(extra_body) => Some(extra_body),
                    None => optional_json_object(CHAT_EXTRA_BODY_ENV)?,
                }
            }
        };
        let execution = ExecutionSettings::parse(
            optional_environment_variable("BEYOND_SLIDES_SCHEDULING")?.as_deref(),
            optional_environment_variable("BEYOND_SLIDES_INITIAL_CONCURRENCY")?.as_deref(),
            optional_environment_variable("BEYOND_SLIDES_MAX_CONCURRENCY")?.as_deref(),
            optional_environment_variable("BEYOND_SLIDES_REQUEST_INTERVAL_MS")?.as_deref(),
        )?;
        Ok(Self {
            worker: crate::worker_control::WorkerControl::from_environment()?,
            backend,
            base_url: if backend == ModelBackendKind::OpenAiCompatible {
                Some(required_environment_variable(API_BASE_URL_ENV)?)
            } else {
                None
            },
            api_key: if backend == ModelBackendKind::OpenAiCompatible {
                Some(required_environment_variable(API_KEY_ENV)?)
            } else {
                None
            },
            model: required_environment_variable(MODEL_ENV)?,
            codex_reasoning_effort: if backend == ModelBackendKind::Codex {
                optional_environment_variable(CODEX_REASONING_EFFORT_ENV)?
            } else {
                None
            },
            codex_service_tier: if backend == ModelBackendKind::Codex {
                optional_environment_variable(CODEX_SERVICE_TIER_ENV)?
            } else {
                None
            },
            extra_body,
            execution,
            scheduler: execution.scheduler(),
        })
    }

    #[cfg(test)]
    pub(crate) fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let execution = ExecutionSettings::parse(None, None, None, None).expect("valid defaults");
        Self {
            worker: crate::worker_control::WorkerControl::new(None)
                .expect("disabled worker control"),
            backend: ModelBackendKind::OpenAiCompatible,
            base_url: Some(base_url.into()),
            api_key: Some(api_key.into()),
            model: model.into(),
            codex_reasoning_effort: None,
            codex_service_tier: None,
            extra_body: None,
            execution,
            scheduler: execution.scheduler(),
        }
    }

    pub(crate) fn base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or("codex-app-server://stdio")
    }

    pub(crate) const fn backend_name(&self) -> &'static str {
        match self.backend {
            ModelBackendKind::OpenAiCompatible => "openai_compatible",
            ModelBackendKind::Codex => "codex",
        }
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn extra_body(&self) -> Option<&Value> {
        self.extra_body.as_ref()
    }

    pub(crate) fn max_concurrency(&self) -> usize {
        self.execution.max_concurrency.get()
    }

    pub(crate) fn request_interval(&self) -> Duration {
        Duration::from_millis(self.execution.request_interval_ms)
    }

    pub(crate) fn scheduler(&self) -> RequestScheduler {
        self.scheduler.clone()
    }

    pub(crate) fn with_scheduler(mut self, scheduler: RequestScheduler) -> Self {
        self.scheduler = scheduler;
        self
    }

    pub(crate) fn record_scheduling(&self, directory: &Path) -> Result<(), io::Error> {
        write_json_atomically(
            &directory.join("request-scheduling.json"),
            &self.scheduler.snapshot(),
            "request scheduling telemetry",
        )
    }

    pub(crate) fn progress_message(&self, task: &str) -> String {
        let snapshot = self.scheduler.snapshot();
        format!(
            "{task} · model {}/{} · {} ms spacing",
            snapshot.effective_concurrency, snapshot.max_concurrency, snapshot.request_interval_ms
        )
    }

    pub(crate) fn record_execution_settings(&self, directory: &Path) -> Result<(), io::Error> {
        eprintln!(
            "Model scheduling: {:?}, initial concurrency {}, concurrency ceiling {}, request spacing floor {} ms",
            self.execution.mode,
            self.execution.initial_concurrency,
            self.max_concurrency(),
            self.execution.request_interval_ms
        );
        write_json_atomically(
            &directory.join("execution-settings.json"),
            &self.execution,
            "execution settings",
        )?;
        self.record_scheduling(directory)
    }

    pub(crate) fn chat_config(&self) -> Result<ChatCompletionsConfig, ChatCompletionsConfigError> {
        let config = ChatCompletionsConfig::new(
            self.base_url.clone().unwrap_or_default(),
            self.api_key.clone().unwrap_or_default(),
            self.model.clone(),
        )?
        .with_request_scheduler(self.scheduler.clone());
        match &self.extra_body {
            Some(extra_body) => config.with_extra_body(extra_body.clone()),
            None => Ok(config),
        }
    }

    pub(crate) fn model_client(
        &self,
        trace: ModelExchangeTrace,
        configure_chat: impl FnOnce(
            ChatCompletionsConfig,
        )
            -> Result<ChatCompletionsConfig, ChatCompletionsConfigError>,
    ) -> Result<Box<dyn LectureModelBackend>, Box<dyn std::error::Error>> {
        match self.backend {
            ModelBackendKind::OpenAiCompatible => {
                Ok(Box::new(beyond_slides::ChatCompletionsClient::new(
                    configure_chat(self.chat_config()?.with_model_trace(trace))?,
                )))
            }
            ModelBackendKind::Codex => {
                let mut config = CodexAppServerConfig::new(self.model.clone())?
                    .with_request_scheduler(self.scheduler.clone())
                    .with_model_trace(trace);
                if let Some(reasoning_effort) = &self.codex_reasoning_effort {
                    config = config.with_reasoning_effort(reasoning_effort.clone())?;
                }
                if let Some(service_tier) = &self.codex_service_tier {
                    config = config.with_service_tier(service_tier.clone())?;
                }
                Ok(Box::new(beyond_slides::CodexAppServerClient::new(config)))
            }
        }
    }
}

pub(crate) fn default_model_backend() -> String {
    "openai_compatible".into()
}

fn optional_environment_variable(name: &str) -> Result<Option<String>, io::Error> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("environment variable {name} is not valid Unicode"),
        )),
    }
}

pub(crate) fn initialize_run_directory<T>(
    run_directory: &Path,
    expected_manifest: &T,
    run_kind: &str,
) -> Result<(), io::Error>
where
    T: DeserializeOwned + PartialEq + Serialize,
{
    initialize_run_directory_with(run_directory, expected_manifest, run_kind, PartialEq::eq)
}

pub(crate) fn initialize_run_directory_with<T>(
    run_directory: &Path,
    expected_manifest: &T,
    run_kind: &str,
    compatible: impl FnOnce(&T, &T) -> bool,
) -> Result<(), io::Error>
where
    T: DeserializeOwned + Serialize,
{
    fs::create_dir_all(run_directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not create {run_kind} run directory {}: {error}",
                run_directory.display()
            ),
        )
    })?;
    let manifest_path = run_directory.join(MANIFEST_FILE);
    if manifest_path.exists() {
        let actual_manifest: T = read_json(&manifest_path, "run manifest")?;
        if !compatible(&actual_manifest, expected_manifest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{run_kind} run manifest {} does not match the current inputs or configuration; use a new run directory",
                    manifest_path.display()
                ),
            ));
        }
    } else {
        if fs::read_dir(run_directory)?.any(|entry| {
            if entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name() == RUN_LOCK_FILE)
            {
                return false;
            }
            entry
                .and_then(|entry| entry.file_type())
                .map_or(true, |kind| !kind.is_dir())
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{run_kind} directory {} contains files but no manifest; cannot establish checkpoint provenance",
                    run_directory.display()
                ),
            ));
        }
        write_json_atomically(&manifest_path, expected_manifest, "run manifest")?;
    }
    Ok(())
}

pub(crate) fn checkpoint_path(run_directory: &Path, window_number: usize) -> PathBuf {
    run_directory.join(format!("window-{window_number:04}.json"))
}

pub(crate) fn open_run_model_trace(run_directory: &Path) -> Result<ModelExchangeTrace, io::Error> {
    ModelExchangeTrace::open(model_trace_path(run_directory))
}

pub(crate) fn model_trace_path(run_directory: &Path) -> PathBuf {
    run_directory.join(MODEL_TRACE_FILE)
}

pub(crate) fn window_progress_bar(
    total_windows: usize,
    completed_windows: usize,
) -> Result<ProgressBar, indicatif::style::TemplateError> {
    let progress = ProgressBar::new(total_windows as u64);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
             {pos}/{len} windows {msg}",
        )?
        .progress_chars("=>-"),
    );
    progress.set_position(completed_windows as u64);
    progress.enable_steady_tick(Duration::from_millis(100));
    Ok(progress)
}

pub(crate) fn ranking_progress_bar(
    total_batches: usize,
    completed_batches: usize,
) -> Result<ProgressBar, indicatif::style::TemplateError> {
    let progress = ProgressBar::new(total_batches as u64);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.magenta/blue}] \
             {pos}/{len} comparison batches {msg}",
        )?
        .progress_chars("=>-"),
    );
    progress.set_position(completed_batches as u64);
    progress.enable_steady_tick(Duration::from_millis(100));
    Ok(progress)
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path, kind: &str) -> Result<T, io::Error> {
    let bytes = fs::read(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {kind} {}: {error}", path.display()),
        )
    })?;
    parse_json(&bytes, path, kind)
}

pub(crate) fn read_json_with_hash<T: DeserializeOwned>(
    path: &Path,
    kind: &str,
) -> Result<(T, String), io::Error> {
    let bytes = fs::read(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {kind} {}: {error}", path.display()),
        )
    })?;
    let value = parse_json(&bytes, path, kind)?;
    Ok((value, sha256(&bytes)))
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8], path: &Path, kind: &str) -> Result<T, io::Error> {
    serde_json::from_slice(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {kind} JSON in {}: {error}", path.display()),
        )
    })
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn write_json_atomically<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    kind: &str,
) -> Result<(), io::Error> {
    let mut temporary = temporary_file(path, kind)?;
    serde_json::to_writer_pretty(&mut temporary, value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not serialize {kind}: {error}"),
        )
    })?;
    temporary.write_all(b"\n")?;
    persist_temporary(temporary, path, kind)
}

pub(crate) fn write_text_atomically(path: &Path, text: &str, kind: &str) -> Result<(), io::Error> {
    let mut temporary = temporary_file(path, kind)?;
    temporary.write_all(text.as_bytes())?;
    persist_temporary(temporary, path, kind)
}

fn temporary_file(path: &Path, kind: &str) -> Result<NamedTempFile, io::Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    NamedTempFile::new_in(parent).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not create temporary {kind} in {}: {error}",
                parent.display()
            ),
        )
    })
}

fn persist_temporary(temporary: NamedTempFile, path: &Path, kind: &str) -> Result<(), io::Error> {
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| {
        io::Error::new(
            error.error.kind(),
            format!(
                "could not persist {kind} {}: {}",
                path.display(),
                error.error
            ),
        )
    })?;
    Ok(())
}

fn required_environment_variable(name: &str) -> Result<String, io::Error> {
    env::var(name).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("could not read required environment variable {name}: {error}"),
        )
    })
}

fn optional_json_object(name: &str) -> Result<Option<Value>, io::Error> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(error) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("could not read optional environment variable {name}: {error}"),
            ));
        }
    };
    let value: Value = serde_json::from_str(&value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("environment variable {name} is not valid JSON: {error}"),
        )
    })?;
    if !value.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("environment variable {name} must contain a JSON object"),
        ));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn run_lock_excludes_other_writers_and_releases_on_drop() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let first = lock_run_directory(directory.path())?;
        assert!(lock_run_directory(directory.path()).is_err());
        initialize_run_directory(directory.path(), &serde_json::json!({"version": 1}), "test")?;
        drop(first);
        let _next = lock_run_directory(directory.path())?;
        Ok(())
    }

    #[test]
    fn execution_defaults_and_overrides_are_validated() {
        let defaults = ExecutionSettings::parse(None, None, None, None).unwrap();
        assert_eq!(defaults.mode, SchedulingMode::Adaptive);
        assert_eq!(defaults.initial_concurrency.get(), 2);
        assert_eq!(defaults.max_concurrency.get(), 8);
        assert_eq!(defaults.scheduler().snapshot().effective_concurrency, 2);
        assert_eq!(defaults.request_interval_ms, 0);
        let custom = ExecutionSettings::parse(None, Some("5"), Some("7"), Some("250")).unwrap();
        assert_eq!(custom.initial_concurrency.get(), 5);
        assert_eq!(custom.max_concurrency.get(), 7);
        assert_eq!(custom.request_interval_ms, 250);
        for value in ["0", "-1", "", "2.5", "many"] {
            assert!(ExecutionSettings::parse(None, None, Some(value), None).is_err());
        }
        assert!(ExecutionSettings::parse(None, Some("8"), Some("7"), None).is_err());
        for value in ["-1", "", "0.5", "NaN"] {
            assert!(ExecutionSettings::parse(None, None, None, Some(value)).is_err());
        }
        let fixed = ExecutionSettings::parse(Some("fixed"), None, None, None).unwrap();
        assert_eq!(fixed.max_concurrency.get(), 2);
        assert!(!fixed.scheduler().snapshot().adaptive);
        assert!(ExecutionSettings::parse(Some("maybe"), None, None, None).is_err());
    }

    #[test]
    fn unmanifested_files_cannot_be_adopted_as_checkpoints() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("window-0001.json"), "{}").unwrap();
        let error =
            initialize_run_directory(directory.path(), &serde_json::json!({"version":1}), "test")
                .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!directory.path().join("manifest.json").exists());
    }

    #[test]
    fn atomic_json_writer_replaces_an_existing_checkpoint() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("window-0001.json");

        write_json_atomically(&path, &1, "test checkpoint")?;
        write_json_atomically(&path, &2, "test checkpoint")?;

        assert_eq!(read_json::<u8>(&path, "test checkpoint")?, 2);
        Ok(())
    }
}
