use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

use futures::StreamExt;
use reqwest::{Client, header::RANGE};
use sha2::{Digest, Sha256};

/// Downloads one pinned model asset, resuming partial transfers when possible.
pub(super) async fn download_verified(
    client: &Client,
    url: &str,
    expected_bytes: u64,
    expected_sha256: &str,
    destination: &Path,
    mut report: impl FnMut(u64) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if destination.is_file() && file_matches(destination, expected_sha256)? {
        report(expected_bytes)?;
        return Ok(());
    }
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    let partial = destination.with_extension("part");
    for attempt in 1..=3 {
        let mut downloaded = partial
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if downloaded == expected_bytes && file_matches(&partial, expected_sha256)? {
            fs::rename(partial, destination)?;
            report(expected_bytes)?;
            return Ok(());
        }
        if downloaded > expected_bytes {
            fs::remove_file(&partial)?;
            downloaded = 0;
        }
        report(downloaded)?;
        let mut request = client.get(url);
        if downloaded > 0 {
            request = request.header(RANGE, format!("bytes={downloaded}-"));
        }
        let response = match request
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
        {
            Ok(response) => response,
            Err(error) if attempt < 3 => {
                eprintln!("Model download attempt {attempt} failed; resuming: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(attempt)).await;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let append = downloaded > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if !append {
            downloaded = 0;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&partial)?;
        let mut stream = response.bytes_stream();
        let mut next_report = downloaded.saturating_add(1024 * 1024);
        let mut transfer_failed = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    file.write_all(&chunk)?;
                    downloaded = downloaded.saturating_add(chunk.len() as u64);
                    if downloaded >= next_report || downloaded >= expected_bytes {
                        report(downloaded.min(expected_bytes))?;
                        next_report = downloaded.saturating_add(1024 * 1024);
                    }
                }
                Err(error) if attempt < 3 => {
                    eprintln!(
                        "Model download attempt {attempt} was interrupted; resuming: {error}"
                    );
                    transfer_failed = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        file.flush()?;
        if transfer_failed {
            tokio::time::sleep(std::time::Duration::from_secs(attempt)).await;
            continue;
        }
        if downloaded == expected_bytes && file_matches(&partial, expected_sha256)? {
            fs::rename(partial, destination)?;
            return Ok(());
        }
        if attempt == 3 {
            return Err(format!(
                "downloaded model asset from {url} failed size or SHA-256 validation"
            )
            .into());
        }
        fs::remove_file(&partial)?;
    }
    unreachable!("the bounded download loop always returns")
}

pub(super) fn file_matches(path: &Path, expected_sha256: &str) -> Result<bool, io::Error> {
    Ok(path.is_file() && file_hash(path)? == expected_sha256)
}

pub(super) fn file_hash(path: &Path) -> Result<String, io::Error> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
