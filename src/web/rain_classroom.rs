use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Mutex as StdMutex,
    time::Duration,
};

use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::{Browser, Page, browser::BrowserConfig, page::ScreenshotParams};
use futures::StreamExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{io::AsyncWriteExt, sync::Mutex, task::JoinHandle};

const HOME_URL: &str = "https://pro.yuketang.cn/v2/web/index";

/// The stable identity selected by the user. Replay URLs are intentionally not
/// accepted from the browser because Rain Classroom signs them for a limited
/// time and the server must establish which entries belong to the lecture.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LectureSelection {
    pub classroom_id: u64,
    pub lesson_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Course {
    pub classroom_id: u64,
    pub course_name: String,
    pub classroom_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Lecture {
    pub lesson_id: String,
    pub title: String,
}

pub(super) struct RainClassroom {
    profile: PathBuf,
    session: Mutex<Option<BrowserSession>>,
    downloads: StdMutex<HashMap<String, DownloadProgress>>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DownloadProgress {
    phase: &'static str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

struct DownloadRegistration<'a> {
    rain_classroom: &'a RainClassroom,
    import_id: &'a str,
}

impl Drop for DownloadRegistration<'_> {
    fn drop(&mut self) {
        self.rain_classroom
            .downloads
            .lock()
            .expect("Rain Classroom download progress lock is not poisoned")
            .remove(self.import_id);
    }
}

struct BrowserSession {
    browser: Browser,
    page: Page,
    handler: JoinHandle<()>,
    authenticated: bool,
}

impl RainClassroom {
    pub fn new(application_root: &Path) -> Self {
        Self {
            profile: application_root.join(".rain-classroom-browser"),
            session: Mutex::new(None),
            downloads: StdMutex::new(HashMap::new()),
        }
    }

    pub fn download_progress(&self, import_id: &str) -> Option<DownloadProgress> {
        self.downloads
            .lock()
            .expect("Rain Classroom download progress lock is not poisoned")
            .get(import_id)
            .cloned()
    }

    /// Opens a dedicated headless browser. Its profile is retained locally so
    /// a user normally needs to scan the QR code only once.
    pub async fn connect(&self) -> Result<(), String> {
        let mut session = self.session.lock().await;
        if let Some(current) = session.as_ref() {
            if current.page.bring_to_front().await.is_ok() {
                return Ok(());
            }
            *session = None;
        }

        tokio::fs::create_dir_all(&self.profile)
            .await
            .map_err(|error| {
                format!("Could not create the Rain Classroom browser profile: {error}")
            })?;
        let config = BrowserConfig::builder()
            .new_headless_mode()
            .user_data_dir(&self.profile)
            .request_timeout(Duration::from_secs(45))
            .launch_timeout(Duration::from_secs(30))
            .build()
            .map_err(provider_error)?;
        let (browser, mut handler) = Browser::launch(config).await.map_err(provider_error)?;
        let handler = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        let page = browser.new_page(HOME_URL).await.map_err(provider_error)?;
        *session = Some(BrowserSession {
            browser,
            page,
            handler,
            authenticated: false,
        });
        Ok(())
    }

    /// Gives Chromium an opportunity to flush its persistent profile before
    /// the local controller exits.
    pub async fn shutdown(&self) {
        let Some(mut session) = self.session.lock().await.take() else {
            return;
        };
        let closed = tokio::time::timeout(Duration::from_secs(5), session.browser.close()).await;
        if closed.is_err() || closed.is_ok_and(|result| result.is_err()) {
            let _ = session.browser.kill().await;
        } else {
            let _ = tokio::time::timeout(Duration::from_secs(5), session.browser.wait()).await;
        }
        session.handler.abort();
    }

    pub async fn login_view(&self) -> Result<Vec<u8>, String> {
        let session = self.session.lock().await;
        let session = session
            .as_ref()
            .ok_or("Start Rain Classroom login before requesting its QR code")?;
        if session.authenticated {
            return Err("Rain Classroom is already authenticated".into());
        }
        session
            .page
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .build(),
            )
            .await
            .map_err(provider_error)
    }

    pub async fn courses(&self) -> Result<Vec<Course>, String> {
        let response: CourseResponse = self
            .evaluate(
                "async function() { const response = await (await fetch('/v/course_meta/learning_list/?front_time=' + Date.now(), {credentials: 'include'})).json(); const valid = Array.isArray(response.data); return {errcode: valid ? Number(response.errcode ?? response.code ?? 0) : -1, errmsg: String(response.errmsg ?? response.msg ?? ''), data: valid ? response.data : []}; }",
            )
            .await?;
        if response.errcode != 0 {
            if let Some(session) = self.session.lock().await.as_mut() {
                session.authenticated = false;
            }
            return Err(login_error(&response.errmsg));
        }
        if let Some(session) = self.session.lock().await.as_mut() {
            session.authenticated = true;
        }
        let mut courses: Vec<_> = response
            .data
            .into_iter()
            .map(|course| {
                let classroom_id = course.classroom_id;
                Course {
                    classroom_id,
                    course_name: course
                        .course_name
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| format!("Course {classroom_id}")),
                    classroom_name: course.classroom_name.unwrap_or_default(),
                }
            })
            .collect();
        courses.sort_by(|left, right| left.course_name.cmp(&right.course_name));
        Ok(courses)
    }

    pub async fn lectures(&self, classroom_id: u64) -> Result<Vec<Lecture>, String> {
        let script = format!(
            "async function() {{ const response = await (await fetch('/v2/api/web/logs/learn/{classroom_id}?actype=-1&page=0&offset=100&sort=-1', {{credentials: 'include'}})).json(); const valid = Array.isArray(response.data?.activities); return {{errcode: valid ? Number(response.errcode ?? response.code ?? 0) : -1, errmsg: String(response.errmsg ?? response.msg ?? ''), data: {{activities: valid ? response.data.activities : []}}}}; }}"
        );
        let response: LectureResponse = self.evaluate(&script).await?;
        if response.errcode != 0 {
            return Err(login_error(&response.errmsg));
        }
        let mut lectures: Vec<_> = response
            .data
            .activities
            .into_iter()
            .filter(|activity| activity.kind == 14 && !activity.courseware_id.is_empty())
            .map(|activity| Lecture {
                lesson_id: activity.courseware_id,
                title: activity
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| "未命名讲次".into()),
            })
            .collect();
        lectures.reverse();
        Ok(lectures)
    }

    /// Materializes all replay entries for one lecture as one recording. The
    /// provider's segmentation never crosses this interface.
    pub async fn acquire_recording(
        &self,
        selection: &LectureSelection,
        destination: &Path,
        import_id: &str,
    ) -> Result<u64, String> {
        let registration = self.register_download(import_id)?;
        if selection.lesson_id.is_empty()
            || !selection
                .lesson_id
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            return Err("Invalid Rain Classroom lecture identifier".into());
        }
        // Checking the classroom prevents a stale or edited form from importing
        // a lesson that was not offered by the selected course.
        if !self
            .lectures(selection.classroom_id)
            .await?
            .iter()
            .any(|lecture| lecture.lesson_id == selection.lesson_id)
        {
            return Err("The selected lecture is not part of this Rain Classroom course".into());
        }
        let script = format!(
            "async function() {{ const response = await (await fetch('/api/v3/classroom-report/replay?lesson_id={}&canFakeLive=1&front_time=' + Date.now(), {{credentials: 'include'}})).json(); return {{code: Number(response.code ?? response.errcode ?? -1), msg: String(response.msg ?? response.errmsg ?? ''), data: response.data ?? null}}; }}",
            selection.lesson_id
        );
        let response: ReplayResponse = self.evaluate(&script).await?;
        if response.code != 0 {
            return Err(login_error(&response.msg));
        }
        let response_data = response
            .data
            .ok_or_else(|| login_error("missing replay data"))?;
        if response_data.show_playback != 1 {
            return Err("Rain Classroom does not make this lecture recording available".into());
        }
        let lesson_duration = response_data.lesson_duration;
        let entries: Vec<_> = response_data
            .live
            .into_iter()
            .filter(|entry| entry.hidden_status == 0 && !entry.url.is_empty())
            .collect();
        if entries.is_empty() {
            return Err("Rain Classroom returned no playable recording for this lecture".into());
        }

        let parts = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".rain-classroom-download");
        if parts.exists() {
            tokio::fs::remove_dir_all(&parts).await.map_err(io_error)?;
        }
        tokio::fs::create_dir_all(&parts).await.map_err(io_error)?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60))
            .build()
            .map_err(provider_error)?;
        let mut responses = Vec::with_capacity(entries.len());
        let mut total_bytes = Some(0_u64);
        for (index, entry) in entries.iter().enumerate() {
            let response = request_download(&client, &entry.url).await?;
            total_bytes = total_bytes
                .zip(response.content_length())
                .and_then(|(total, length)| total.checked_add(length));
            responses.push((index, response));
        }
        self.update_download(import_id, |progress| {
            progress.phase = "downloading";
            progress.total_bytes = total_bytes;
        });
        for (index, response) in responses {
            download(
                response,
                &parts.join(format!("part-{index:04}.mp4")),
                |bytes| {
                    self.update_download(import_id, |progress| {
                        progress.downloaded_bytes = progress.downloaded_bytes.saturating_add(bytes);
                    });
                },
            )
            .await?;
        }

        self.update_download(import_id, |progress| progress.phase = "assembling");

        if entries.len() == 1 {
            tokio::fs::rename(parts.join("part-0000.mp4"), destination)
                .await
                .map_err(io_error)?;
        } else {
            let manifest = (0..entries.len())
                .map(|index| format!("file 'part-{index:04}.mp4'\n"))
                .collect::<String>();
            tokio::fs::write(parts.join("concat.txt"), manifest)
                .await
                .map_err(io_error)?;
            let output = tokio::process::Command::new("ffmpeg")
                .current_dir(&parts)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    "concat.txt",
                    "-c",
                    "copy",
                    "-movflags",
                    "+faststart",
                    "recording.mp4",
                ])
                .output()
                .await
                .map_err(|error| {
                    format!(
                        "Could not start ffmpeg to assemble the Rain Classroom recording: {error}"
                    )
                })?;
            if !output.status.success() {
                return Err(format!(
                    "Could not assemble the Rain Classroom recording: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            tokio::fs::rename(parts.join("recording.mp4"), destination)
                .await
                .map_err(io_error)?;
        }
        tokio::fs::remove_dir_all(&parts).await.map_err(io_error)?;
        drop(registration);
        Ok(lesson_duration)
    }

    fn register_download<'a>(
        &'a self,
        import_id: &'a str,
    ) -> Result<DownloadRegistration<'a>, String> {
        if import_id.is_empty()
            || import_id.len() > 64
            || !import_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("Invalid Rain Classroom import progress identifier".into());
        }
        let mut downloads = self
            .downloads
            .lock()
            .expect("Rain Classroom download progress lock is not poisoned");
        if downloads.contains_key(import_id) {
            return Err("This Rain Classroom import is already in progress".into());
        }
        downloads.insert(
            import_id.into(),
            DownloadProgress {
                phase: "preparing",
                downloaded_bytes: 0,
                total_bytes: None,
            },
        );
        Ok(DownloadRegistration {
            rain_classroom: self,
            import_id,
        })
    }

    fn update_download(&self, import_id: &str, update: impl FnOnce(&mut DownloadProgress)) {
        if let Some(progress) = self
            .downloads
            .lock()
            .expect("Rain Classroom download progress lock is not poisoned")
            .get_mut(import_id)
        {
            update(progress);
        }
    }

    async fn evaluate<T: DeserializeOwned>(&self, script: &str) -> Result<T, String> {
        let session = self.session.lock().await;
        let session = session
            .as_ref()
            .ok_or("Open Rain Classroom and finish QR-code login before loading courses")?;
        let value: serde_json::Value = session
            .page
            .evaluate_function(script)
            .await
            .map_err(provider_error)?
            .into_value()
            .map_err(provider_error)?;
        serde_json::from_value(value.clone()).map_err(|error| {
            let shape = match value {
                serde_json::Value::Object(object) => {
                    format!(
                        "object fields [{}]",
                        object.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                }
                serde_json::Value::Array(_) => "array".into(),
                serde_json::Value::Null => "null".into(),
                serde_json::Value::Bool(_) => "boolean".into(),
                serde_json::Value::Number(_) => "number".into(),
                serde_json::Value::String(_) => "string".into(),
            };
            format!("Rain Classroom returned an unexpected response ({shape}): {error}")
        })
    }
}

async fn request_download(
    client: &reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, String> {
    let url = url::Url::parse(url).map_err(provider_error)?;
    if url.scheme() != "https" {
        return Err("Rain Classroom returned a recording URL that is not HTTPS".into());
    }
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Could not download the Rain Classroom recording: {error}"))
}

async fn download(
    response: reqwest::Response,
    destination: &Path,
    mut report_bytes: impl FnMut(u64),
) -> Result<(), String> {
    let mut output = tokio::fs::File::create(destination)
        .await
        .map_err(io_error)?;
    let mut bytes = response.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(provider_error)?;
        output.write_all(&chunk).await.map_err(io_error)?;
        report_bytes(chunk.len() as u64);
    }
    output.flush().await.map_err(io_error)
}

fn provider_error(error: impl std::fmt::Display) -> String {
    format!("Rain Classroom could not be reached: {error}")
}

fn io_error(error: io::Error) -> String {
    format!("Could not save the Rain Classroom recording: {error}")
}

fn login_error(message: &str) -> String {
    if message.is_empty() {
        "Rain Classroom rejected the request; finish QR-code login and try again".into()
    } else {
        format!(
            "Rain Classroom rejected the request ({message}); finish QR-code login and try again"
        )
    }
}

#[derive(Deserialize)]
struct CourseResponse {
    errcode: i64,
    #[serde(default)]
    errmsg: String,
    #[serde(default)]
    data: Vec<CourseRecord>,
}

#[derive(Deserialize)]
struct CourseRecord {
    classroom_id: u64,
    #[serde(default)]
    course_name: Option<String>,
    #[serde(default)]
    classroom_name: Option<String>,
}

#[derive(Deserialize)]
struct LectureResponse {
    errcode: i64,
    #[serde(default)]
    errmsg: String,
    data: LectureData,
}

#[derive(Default, Deserialize)]
struct LectureData {
    #[serde(default)]
    activities: Vec<ActivityRecord>,
}

#[derive(Deserialize)]
struct ActivityRecord {
    #[serde(rename = "type")]
    kind: u8,
    #[serde(default)]
    courseware_id: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct ReplayResponse {
    code: i64,
    #[serde(default)]
    msg: String,
    data: Option<ReplayData>,
}

#[derive(Deserialize)]
struct ReplayData {
    #[serde(rename = "lessonDuration")]
    lesson_duration: u64,
    #[serde(rename = "showPlayback")]
    show_playback: u8,
    #[serde(default)]
    live: Vec<ReplayEntry>,
}

#[derive(Deserialize)]
struct ReplayEntry {
    url: String,
    #[serde(rename = "hiddenStatus", default)]
    hidden_status: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_progress_exists_only_for_the_registered_import() {
        let root = tempfile::tempdir().unwrap();
        let rain_classroom = RainClassroom::new(root.path());

        let registration = rain_classroom
            .register_download("browser-import-1")
            .unwrap();
        assert_eq!(
            serde_json::to_value(
                rain_classroom
                    .download_progress("browser-import-1")
                    .unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "phase": "preparing",
                "downloaded_bytes": 0,
                "total_bytes": null
            })
        );

        rain_classroom.update_download("browser-import-1", |progress| {
            progress.phase = "downloading";
            progress.downloaded_bytes = 125;
            progress.total_bytes = Some(500);
        });
        let progress = rain_classroom
            .download_progress("browser-import-1")
            .unwrap();
        assert_eq!(progress.phase, "downloading");
        assert_eq!(progress.downloaded_bytes, 125);
        assert_eq!(progress.total_bytes, Some(500));

        drop(registration);
        assert!(
            rain_classroom
                .download_progress("browser-import-1")
                .is_none()
        );
    }

    #[test]
    fn download_progress_rejects_unsafe_or_duplicate_identifiers() {
        let root = tempfile::tempdir().unwrap();
        let rain_classroom = RainClassroom::new(root.path());
        assert!(rain_classroom.register_download("../escape").is_err());

        let _registration = rain_classroom.register_download("same-import").unwrap();
        assert!(rain_classroom.register_download("same-import").is_err());
    }

    #[test]
    fn provider_replay_entries_are_private_implementation_details() {
        let response: ReplayResponse = serde_json::from_value(serde_json::json!({
            "code": 0,
            "msg": "OK",
            "data": {
                "lessonDuration": 8427000,
                "showPlayback": 1,
                "live": [
                    {"url": "https://media.example/first", "hiddenStatus": 0},
                    {"url": "https://media.example/second", "hiddenStatus": 0}
                ]
            }
        }))
        .unwrap();
        let data = response.data.unwrap();
        assert_eq!(data.lesson_duration, 8_427_000);
        assert_eq!(data.live.len(), 2);
        let public = serde_json::to_value(Lecture {
            lesson_id: "lesson".into(),
            title: "并发编程".into(),
        })
        .unwrap();
        assert!(public.get("segments").is_none());
        assert!(public.get("live").is_none());
    }
}
