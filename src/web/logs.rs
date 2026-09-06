//! Browser-visible logs are sanitized at capture time, never raw legacy logs.
use std::{
    io,
    path::Path,
    process::{ExitStatus, Stdio},
    sync::Arc,
};

use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};

const TAIL_BYTES: u64 = 128 * 1024;

struct Sink {
    file: tokio::fs::File,
    // A second pass prevents two individually harmless stream fragments from
    // combining into a credential in the merged log.
    redactor: Redactor,
}

impl Sink {
    async fn write(&mut self, bytes: &[u8], eof: bool) -> io::Result<()> {
        let safe = self.redactor.feed(bytes, eof);
        self.file.write_all(&safe).await?;
        self.file.flush().await
    }
}

/// Retain enough bytes across reads to redact secrets split across pipe chunks.
struct Redactor {
    secrets: Vec<Vec<u8>>,
    pending: Vec<u8>,
    lookbehind: usize,
}

impl Redactor {
    fn new(key: &str) -> Self {
        let mut variants = vec![key.to_owned()];
        if let Ok(json) = serde_json::to_string(key) {
            variants.push(json[1..json.len() - 1].to_owned());
        }
        variants.push(url::form_urlencoded::byte_serialize(key.as_bytes()).collect());
        let mut secrets: Vec<Vec<u8>> = variants
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(String::into_bytes)
            .collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        let lookbehind = secrets.iter().map(Vec::len).max().unwrap_or(1) - 1;
        Self {
            secrets,
            pending: Vec::new(),
            lookbehind,
        }
    }

    fn feed(&mut self, bytes: &[u8], eof: bool) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let safe_end = if eof {
            self.pending.len()
        } else {
            self.pending.len().saturating_sub(self.lookbehind)
        };
        let mut result = Vec::new();
        let mut position = 0;
        while position < safe_end {
            if let Some(secret) = self
                .secrets
                .iter()
                .find(|s| self.pending[position..].starts_with(s))
            {
                result.extend_from_slice(b"<REDACTED>");
                position += secret.len();
            } else {
                result.push(self.pending[position]);
                position += 1;
            }
        }
        self.pending.drain(..position);
        result
    }
}

/// Drain both output streams even if one sink fails. This process owns the
/// pipes, so a browser/controller restart cannot block the underlying work.
pub(super) async fn capture(
    command: &mut tokio::process::Command,
    path: &Path,
    key: &str,
) -> io::Result<ExitStatus> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(format!("\n--- attempt {} ---\n", super::jobs::now_ms()).as_bytes())
        .await?;
    let file = Arc::new(Mutex::new(Sink {
        file,
        redactor: Redactor::new(key),
    }));
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout configured as a pipe");
    let stderr = child.stderr.take().expect("stderr configured as a pipe");
    let (out, err, status) = tokio::join!(
        pump(stdout, file.clone(), Redactor::new(key)),
        pump(stderr, file.clone(), Redactor::new(key)),
        child.wait(),
    );
    let flushed = file.lock().await.write(&[], true).await;
    out?;
    err?;
    flushed?;
    status
}

async fn pump(
    mut input: impl AsyncRead + Unpin,
    file: Arc<Mutex<Sink>>,
    mut redactor: Redactor,
) -> io::Result<()> {
    let mut buffer = [0; 8192];
    let mut error = None;
    loop {
        let count = input.read(&mut buffer).await?;
        let safe = redactor.feed(&buffer[..count], count == 0);
        if error.is_none() {
            let mut output = file.lock().await;
            if let Err(e) = output.write(&safe, false).await {
                error = Some(e);
            }
        }
        if count == 0 {
            break;
        }
    }
    error.map_or(Ok(()), Err)
}

pub(super) fn path(job: &Path, run: &Path, kind: &str) -> io::Result<std::path::PathBuf> {
    let path = match kind {
        "worker" => run.join("worker-debug.log"),
        "transcription" => job.join("transcription/asr-debug.log"),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unknown log kind",
            ));
        }
    };
    if path.exists()
        && (std::fs::symlink_metadata(&path)?.file_type().is_symlink()
            || !path.canonicalize()?.starts_with(job.canonicalize()?))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Log must remain inside its lecture directory",
        ));
    }
    Ok(path)
}

#[derive(Serialize)]
pub(super) struct Tail {
    pub text: String,
    pub truncated: bool,
    pub available: bool,
}

pub(super) fn tail(path: &Path) -> io::Result<Tail> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Tail {
                text: String::new(),
                truncated: false,
                available: false,
            });
        }
        Err(e) => return Err(e),
    };
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes)?;
    for byte in &mut bytes {
        if *byte == b'\r' {
            *byte = b'\n';
        }
    }
    let plain = strip_ansi_escapes::strip(bytes);
    let text = String::from_utf8_lossy(&plain)
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    Ok(Tail {
        text,
        truncated: size > TAIL_BYTES,
        available: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn capture_redacts_both_streams_and_keeps_exit_status() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("safe.log");
        let mut command = tokio::process::Command::new("sh");
        command.args([
            "-c",
            "printf 'stdout secret:123'; printf 'stderr secret:123' >&2; exit 7",
        ]);
        assert_eq!(
            capture(&mut command, &path, "secret:123").await?.code(),
            Some(7)
        );
        let log = std::fs::read_to_string(path)?;
        assert!(log.contains("stdout"));
        assert!(log.contains("stderr"));
        assert_eq!(log.matches("<REDACTED>").count(), 2);
        assert!(!log.contains("secret:123"));
        Ok(())
    }
    #[test]
    fn secrets_are_removed_at_every_possible_chunk_boundary() {
        let input = b"prefix secret:123 suffix secret:123";
        for boundary in 0..=input.len() {
            let mut redactor = Redactor::new("secret:123");
            let mut output = redactor.feed(&input[..boundary], false);
            output.extend(redactor.feed(&input[boundary..], false));
            output.extend(redactor.feed(&[], true));
            assert_eq!(output, b"prefix <REDACTED> suffix <REDACTED>");
        }
    }
    #[test]
    fn encoded_credentials_and_final_partial_lines_are_redacted() {
        let mut redactor = Redactor::new("key\"中文");
        assert_eq!(
            redactor.feed(
                "key\\\"中文 key%22%E4%B8%AD%E6%96%87 key\"中文".as_bytes(),
                true
            ),
            b"<REDACTED> <REDACTED> <REDACTED>"
        );
    }
    #[test]
    fn tail_is_bounded_and_plain_text() -> io::Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let mut bytes = vec![b'x'; TAIL_BYTES as usize + 100];
        bytes.extend_from_slice(b"\x1b[31mred\x1b[0m\rnext");
        std::fs::write(file.path(), bytes)?;
        let tail = tail(file.path())?;
        assert!(tail.truncated);
        assert!(tail.text.ends_with("red\nnext"));
        assert!(tail.text.len() <= TAIL_BYTES as usize);
        Ok(())
    }

    #[test]
    fn legacy_logs_are_not_used_and_unknown_paths_are_rejected() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let run = directory.path().join("run-0001");
        std::fs::create_dir(&run)?;
        std::fs::write(run.join("worker.log"), "unfiltered legacy log")?;
        assert!(!tail(&path(directory.path(), &run, "worker")?)?.available);
        assert!(path(directory.path(), &run, "../../worker.log").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn merged_stream_fragments_cannot_reassemble_a_secret() -> io::Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let mut sink = Sink {
            file: tokio::fs::File::create(file.path()).await?,
            redactor: Redactor::new("secret"),
        };
        sink.write(b"sec", false).await?;
        sink.write(b"ret", true).await?;
        assert_eq!(std::fs::read(file.path())?, b"<REDACTED>");
        Ok(())
    }
}
