use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{ChatCompletionsConfig, ChatCompletionsConfigError, ModelExchangeTrace};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const API_BASE_URL_ENV: &str = "BEYOND_SLIDES_API_BASE_URL";
const API_KEY_ENV: &str = "BEYOND_SLIDES_API_KEY";
const MODEL_ENV: &str = "BEYOND_SLIDES_MODEL";
const CHAT_EXTRA_BODY_ENV: &str = "BEYOND_SLIDES_CHAT_EXTRA_BODY";
const MANIFEST_FILE: &str = "manifest.json";
const MODEL_TRACE_FILE: &str = "model-trace.jsonl";

pub(crate) struct ProviderSettings {
    base_url: String,
    api_key: String,
    model: String,
    extra_body: Option<Value>,
}

impl ProviderSettings {
    pub(crate) fn from_environment() -> Result<Self, io::Error> {
        Ok(Self {
            base_url: required_environment_variable(API_BASE_URL_ENV)?,
            api_key: required_environment_variable(API_KEY_ENV)?,
            model: required_environment_variable(MODEL_ENV)?,
            extra_body: optional_json_object(CHAT_EXTRA_BODY_ENV)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            extra_body: None,
        }
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn extra_body(&self) -> Option<&Value> {
        self.extra_body.as_ref()
    }

    pub(crate) fn chat_config(&self) -> Result<ChatCompletionsConfig, ChatCompletionsConfigError> {
        let config = ChatCompletionsConfig::new(
            self.base_url.clone(),
            self.api_key.clone(),
            self.model.clone(),
        )?;
        match &self.extra_body {
            Some(extra_body) => config.with_extra_body(extra_body.clone()),
            None => Ok(config),
        }
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
        if actual_manifest != *expected_manifest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{run_kind} run manifest {} does not match the current inputs or configuration; use a new run directory",
                    manifest_path.display()
                ),
            ));
        }
    } else {
        write_json_atomically(&manifest_path, expected_manifest, "run manifest")?;
    }
    Ok(())
}

pub(crate) fn checkpoint_path(run_directory: &Path, window_number: usize) -> PathBuf {
    run_directory.join(format!("window-{window_number:04}.json"))
}

pub(crate) fn open_run_model_trace(run_directory: &Path) -> Result<ModelExchangeTrace, io::Error> {
    ModelExchangeTrace::open(run_directory.join(MODEL_TRACE_FILE))
}

pub(crate) fn open_output_model_trace(output_path: &Path) -> Result<ModelExchangeTrace, io::Error> {
    ModelExchangeTrace::open(output_path.with_extension("model-trace.jsonl"))
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

pub(crate) fn display_token_count(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "unknown".into(), |tokens| tokens.to_string())
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
    fn atomic_json_writer_replaces_an_existing_checkpoint() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("window-0001.json");

        write_json_atomically(&path, &1, "test checkpoint")?;
        write_json_atomically(&path, &2, "test checkpoint")?;

        assert_eq!(read_json::<u8>(&path, "test checkpoint")?, 2);
        Ok(())
    }
}
