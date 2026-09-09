use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use beyond_slides::runtime_tools;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, PrintToPdfParams};
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

/// One Rain Classroom presentation selected as the written source.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PresentationSelection {
    pub classroom_id: u64,
    pub lesson_id: String,
    pub presentation_id: String,
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
    pub has_recording: bool,
    pub presentation_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Presentation {
    pub presentation_id: String,
    pub title: String,
    pub page_count: usize,
}

pub(super) struct RainClassroom {
    application_root: PathBuf,
    profile: PathBuf,
    session: Mutex<Option<BrowserSession>>,
    downloads: Arc<StdMutex<HashMap<String, DownloadProgress>>>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DownloadProgress {
    resource: &'static str,
    phase: &'static str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    completed_items: Option<usize>,
    total_items: Option<usize>,
}

pub(super) struct AcquiredSources {
    pub slide_ocr: Option<Vec<String>>,
}

struct DownloadRegistration {
    downloads: Arc<StdMutex<HashMap<String, DownloadProgress>>>,
    import_id: String,
}

impl Drop for DownloadRegistration {
    fn drop(&mut self) {
        self.downloads
            .lock()
            .expect("Rain Classroom download progress lock is not poisoned")
            .remove(&self.import_id);
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
            application_root: application_root.to_owned(),
            profile: application_root.join(".rain-classroom-browser"),
            session: Mutex::new(None),
            downloads: Arc::default(),
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
        let mut config = BrowserConfig::builder()
            .new_headless_mode()
            .user_data_dir(&self.profile)
            .request_timeout(Duration::from_secs(45))
            .launch_timeout(Duration::from_secs(30));
        if let Some(executable) =
            beyond_slides::browser_runtime::packaged_chromium_path().map_err(provider_error)?
        {
            config = config
                .chrome_executable(executable)
                .env("APPIMAGE_EXTRACT_AND_RUN", "1");
        }
        let config = config.build().map_err(provider_error)?;
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
        self.close_browser().await;
    }

    /// Forgets the dedicated local login without affecting the user's normal
    /// browser profile or their remote Rain Classroom account.
    pub async fn logout(&self) -> Result<(), String> {
        if !self
            .downloads
            .lock()
            .expect("Rain Classroom download progress lock is not poisoned")
            .is_empty()
        {
            return Err("Wait for the active Rain Classroom import before logging out".into());
        }
        self.close_browser().await;
        match tokio::fs::remove_dir_all(&self.profile).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Could not forget the local Rain Classroom login: {error}"
            )),
        }
    }

    async fn close_browser(&self) {
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
        let mut lectures = self.lecture_catalog(classroom_id).await?;
        let lesson_ids: Vec<_> = lectures
            .iter()
            .map(|lecture| lecture.lesson_id.clone())
            .collect();
        let availability = self.lecture_availability(&lesson_ids).await?;
        for lecture in &mut lectures {
            let Some(details) = availability.get(&lecture.lesson_id) else {
                return Err("Rain Classroom omitted lecture availability information".into());
            };
            lecture.has_recording = details.has_recording;
            lecture.presentation_count = details.presentation_count;
        }
        Ok(lectures)
    }

    async fn lecture_catalog(&self, classroom_id: u64) -> Result<Vec<Lecture>, String> {
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
                has_recording: false,
                presentation_count: 0,
            })
            .collect();
        lectures.reverse();
        Ok(lectures)
    }

    async fn lecture_availability(
        &self,
        lesson_ids: &[String],
    ) -> Result<HashMap<String, LectureAvailability>, String> {
        if lesson_ids.is_empty() {
            return Ok(HashMap::new());
        }
        for lesson_id in lesson_ids {
            validate_identifier(lesson_id, "lecture")?;
        }
        let lesson_ids = serde_json::to_string(lesson_ids).map_err(provider_error)?;
        let script = [
            "async function() { const lessonIds = ",
            &lesson_ids,
            "; return await Promise.all(lessonIds.map(async lesson_id => { const response = await (await fetch('/api/v3/classroom-report/replay?lesson_id=' + encodeURIComponent(lesson_id) + '&canFakeLive=1&front_time=' + Date.now(), {credentials: 'include'})).json(); const data = response.data ?? null; const live = Array.isArray(data?.live) ? data.live : []; const presentations = Array.isArray(data?.presentations) ? data.presentations : []; return {lesson_id, code: Number(response.code ?? response.errcode ?? -1), message: String(response.msg ?? response.errmsg ?? ''), has_recording: Number(data?.showPlayback ?? 0) === 1 && live.some(entry => Number(entry.hiddenStatus ?? 0) === 0 && String(entry.url ?? '') !== ''), presentation_count: presentations.length}; })); }",
        ]
        .concat();
        let records: Vec<LectureAvailabilityRecord> = self.evaluate(&script).await?;
        let mut availability = HashMap::with_capacity(records.len());
        for record in records {
            if record.code != 0 {
                return Err(login_error(&record.message));
            }
            availability.insert(
                record.lesson_id,
                LectureAvailability {
                    has_recording: record.has_recording,
                    presentation_count: record.presentation_count,
                },
            );
        }
        Ok(availability)
    }

    pub async fn presentations(
        &self,
        selection: &LectureSelection,
    ) -> Result<Vec<Presentation>, String> {
        self.validate_lecture(selection).await?;
        let replay = self.replay(&selection.lesson_id).await?;
        let mut presentations = Vec::with_capacity(replay.presentations.len());
        for presentation_id in replay.presentations {
            let detail = self
                .presentation_detail(&selection.lesson_id, &presentation_id)
                .await?;
            presentations.push(Presentation {
                presentation_id,
                title: detail.presentation.title,
                page_count: detail.slides.len(),
            });
        }
        Ok(presentations)
    }

    /// Materializes the selected provider sources behind the canonical files
    /// consumed by the rest of BeyondSlides.
    pub async fn acquire_sources(
        &self,
        slides: Option<&PresentationSelection>,
        recording: Option<&LectureSelection>,
        destination: &Path,
        import_id: &str,
    ) -> Result<AcquiredSources, String> {
        let _registration = self.register_download(import_id)?;
        let slide_ocr = match slides {
            Some(selection) => Some(
                self.acquire_slides(selection, &destination.join("slides.pdf"), import_id)
                    .await?,
            ),
            None => None,
        };
        if let Some(selection) = recording {
            self.acquire_recording(selection, &destination.join("recording.mp4"), import_id)
                .await?;
        }
        Ok(AcquiredSources { slide_ocr })
    }

    /// Materializes all replay entries for one lecture as one recording. The
    /// provider's segmentation never crosses this interface.
    async fn acquire_recording(
        &self,
        selection: &LectureSelection,
        destination: &Path,
        import_id: &str,
    ) -> Result<u64, String> {
        self.update_download(import_id, |progress| {
            progress.resource = "recording";
            progress.phase = "preparing";
            progress.downloaded_bytes = 0;
            progress.total_bytes = None;
            progress.completed_items = None;
            progress.total_items = None;
        });
        self.validate_lecture(selection).await?;
        let response_data = self.replay(&selection.lesson_id).await?;
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
            let response = request_download(&client, &entry.url, "recording").await?;
            total_bytes = total_bytes
                .zip(response.content_length())
                .and_then(|(total, length)| total.checked_add(length));
            responses.push((index, response));
        }
        self.update_download(import_id, |progress| {
            progress.resource = "recording";
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
            let ffmpeg = runtime_tools::ffmpeg_path()
                .map_err(|error| format!("Could not prepare FFmpeg: {error}"))?;
            let output = tokio::process::Command::new(ffmpeg)
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
        Ok(lesson_duration)
    }

    async fn acquire_slides(
        &self,
        selection: &PresentationSelection,
        destination: &Path,
        import_id: &str,
    ) -> Result<Vec<String>, String> {
        self.update_download(import_id, |progress| {
            progress.resource = "slides";
            progress.phase = "preparing";
            progress.downloaded_bytes = 0;
            progress.total_bytes = None;
            progress.completed_items = None;
            progress.total_items = None;
        });
        let lecture = LectureSelection {
            classroom_id: selection.classroom_id,
            lesson_id: selection.lesson_id.clone(),
        };
        self.validate_lecture(&lecture).await?;
        let replay = self.replay(&selection.lesson_id).await?;
        if !replay
            .presentations
            .iter()
            .any(|id| id == &selection.presentation_id)
        {
            return Err(
                "The selected courseware is not part of this Rain Classroom lecture".into(),
            );
        }
        let detail = self
            .presentation_detail(&selection.lesson_id, &selection.presentation_id)
            .await?;
        if detail.slides.is_empty() {
            return Err("Rain Classroom returned an empty courseware presentation".into());
        }
        self.update_download(import_id, |progress| {
            progress.phase = "downloading";
            progress.completed_items = Some(0);
            progress.total_items = Some(detail.slides.len());
        });

        let parts = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".rain-classroom-slides");
        if parts.exists() {
            tokio::fs::remove_dir_all(&parts).await.map_err(io_error)?;
        }
        tokio::fs::create_dir_all(&parts).await.map_err(io_error)?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(5 * 60))
            .build()
            .map_err(provider_error)?;
        let mut slides = detail.slides;
        slides.sort_by_key(|slide| slide.index);
        let mut page_paths = Vec::with_capacity(slides.len());
        for (position, slide) in slides.into_iter().enumerate() {
            let response = request_download(&client, &slide.cover, "courseware page").await?;
            let page_path = parts.join(format!("slide-{position:04}.jpg"));
            download(response, &page_path, |bytes| {
                self.update_download(import_id, |progress| {
                    progress.downloaded_bytes = progress.downloaded_bytes.saturating_add(bytes);
                });
            })
            .await?;
            self.update_download(import_id, |progress| {
                progress.completed_items = Some(position + 1);
            });
            page_paths.push(page_path);
        }
        self.update_download(import_id, |progress| {
            progress.phase = "assembling";
        });
        self.print_slide_pdf(
            &page_paths,
            detail.presentation.width,
            detail.presentation.height,
            destination,
        )
        .await?;
        self.update_download(import_id, |progress| {
            progress.phase = "downloading_ocr_models";
            progress.downloaded_bytes = 0;
            progress.total_bytes = None;
            progress.completed_items = None;
            progress.total_items = None;
        });
        let models =
            super::slide_ocr::ensure_models(&self.application_root, |downloaded, total| {
                self.update_download(import_id, |progress| {
                    progress.downloaded_bytes = downloaded;
                    progress.total_bytes = Some(total);
                });
            })
            .await?;
        self.update_download(import_id, |progress| {
            progress.phase = "recognizing_text";
            progress.downloaded_bytes = 0;
            progress.total_bytes = None;
            progress.completed_items = Some(0);
            progress.total_items = Some(page_paths.len());
        });
        let downloads = Arc::clone(&self.downloads);
        let progress_id = import_id.to_owned();
        let slide_ocr = tokio::task::spawn_blocking(move || {
            super::slide_ocr::recognize_pages(&page_paths, models, |completed, total| {
                if let Some(progress) = downloads
                    .lock()
                    .expect("Rain Classroom download progress lock is not poisoned")
                    .get_mut(&progress_id)
                {
                    progress.completed_items = Some(completed);
                    progress.total_items = Some(total);
                }
            })
        })
        .await
        .map_err(|error| format!("native slide OCR task failed: {error}"))??;
        tokio::fs::remove_dir_all(&parts).await.map_err(io_error)?;
        Ok(slide_ocr)
    }

    async fn print_slide_pdf(
        &self,
        pages: &[PathBuf],
        width: u32,
        height: u32,
        destination: &Path,
    ) -> Result<(), String> {
        let image_tags = pages
            .iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("Could not address a downloaded Rain Classroom slide")?;
                Ok(format!("<img src=\"{name}\">"))
            })
            .collect::<Result<String, String>>()?;
        let html = format!(
            "<!doctype html><style>@page{{size:{width}px {height}px;margin:0}}*{{box-sizing:border-box}}html,body{{margin:0}}img{{display:block;width:{width}px;height:{height}px;object-fit:contain;break-after:page}}img:last-child{{break-after:auto}}</style>{image_tags}"
        );
        let html_path = pages
            .first()
            .and_then(|page| page.parent())
            .ok_or("Rain Classroom returned no slide pages")?
            .join("slides.html");
        tokio::fs::write(&html_path, html).await.map_err(io_error)?;
        let html_url = url::Url::from_file_path(&html_path)
            .map_err(|_| "Could not address the local Rain Classroom slide document")?;
        let session = self.session.lock().await;
        let session = session
            .as_ref()
            .ok_or("Open Rain Classroom and finish QR-code login before importing courseware")?;
        let page = session
            .browser
            .new_page(html_url.as_str())
            .await
            .map_err(provider_error)?;
        page.evaluate_function(
            "async function() { await Promise.all(Array.from(document.images, image => image.complete ? (image.naturalWidth > 0 ? Promise.resolve() : Promise.reject(new Error('slide image failed to load'))) : new Promise((resolve, reject) => { image.addEventListener('load', resolve, {once: true}); image.addEventListener('error', reject, {once: true}); }))); return document.images.length; }",
        )
        .await
        .map_err(provider_error)?;
        let options = PrintToPdfParams::builder()
            .print_background(true)
            .prefer_css_page_size(true)
            .margin_top(0.0)
            .margin_bottom(0.0)
            .margin_left(0.0)
            .margin_right(0.0)
            .build();
        page.save_pdf(options, destination)
            .await
            .map_err(provider_error)?;
        page.close().await.map_err(provider_error)?;
        Ok(())
    }

    async fn validate_lecture(&self, selection: &LectureSelection) -> Result<(), String> {
        validate_identifier(&selection.lesson_id, "lecture")?;
        if !self
            .lecture_catalog(selection.classroom_id)
            .await?
            .iter()
            .any(|lecture| lecture.lesson_id == selection.lesson_id)
        {
            return Err("The selected lecture is not part of this Rain Classroom course".into());
        }
        Ok(())
    }

    async fn replay(&self, lesson_id: &str) -> Result<ReplayData, String> {
        validate_identifier(lesson_id, "lecture")?;
        let script = format!(
            "async function() {{ const response = await (await fetch('/api/v3/classroom-report/replay?lesson_id={lesson_id}&canFakeLive=1&front_time=' + Date.now(), {{credentials: 'include'}})).json(); return {{code: Number(response.code ?? response.errcode ?? -1), msg: String(response.msg ?? response.errmsg ?? ''), data: response.data ?? null}}; }}"
        );
        let response: ReplayResponse = self.evaluate(&script).await?;
        if response.code != 0 {
            return Err(login_error(&response.msg));
        }
        response
            .data
            .ok_or_else(|| login_error("missing replay data"))
    }

    async fn presentation_detail(
        &self,
        lesson_id: &str,
        presentation_id: &str,
    ) -> Result<PresentationData, String> {
        validate_identifier(lesson_id, "lecture")?;
        validate_identifier(presentation_id, "presentation")?;
        let script = format!(
            "async function() {{ const response = await (await fetch('/api/v3/lesson-summary/student/presentation?lesson_id={lesson_id}&presentation_id={presentation_id}', {{credentials: 'include'}})).json(); return {{code: Number(response.code ?? response.errcode ?? -1), msg: String(response.msg ?? response.errmsg ?? ''), data: response.data ?? null}}; }}"
        );
        let response: PresentationResponse = self.evaluate(&script).await?;
        if response.code != 0 {
            return Err(login_error(&response.msg));
        }
        response
            .data
            .ok_or_else(|| login_error("missing courseware data"))
    }

    fn register_download(&self, import_id: &str) -> Result<DownloadRegistration, String> {
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
                resource: "preparing",
                phase: "preparing",
                downloaded_bytes: 0,
                total_bytes: None,
                completed_items: None,
                total_items: None,
            },
        );
        Ok(DownloadRegistration {
            downloads: Arc::clone(&self.downloads),
            import_id: import_id.into(),
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
    resource: &str,
) -> Result<reqwest::Response, String> {
    let url = url::Url::parse(url).map_err(provider_error)?;
    if url.scheme() != "https" {
        return Err(format!(
            "Rain Classroom returned a {resource} URL that is not HTTPS"
        ));
    }
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            format!(
                "Could not download the Rain Classroom {resource}: {}",
                error.without_url()
            )
        })
}

fn validate_identifier(identifier: &str, kind: &str) -> Result<(), String> {
    if identifier.is_empty() || !identifier.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("Invalid Rain Classroom {kind} identifier"));
    }
    Ok(())
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
        let chunk = chunk.map_err(|error| provider_error(error.without_url()))?;
        output.write_all(&chunk).await.map_err(io_error)?;
        report_bytes(chunk.len() as u64);
    }
    output.flush().await.map_err(io_error)
}

fn provider_error(error: impl std::fmt::Display) -> String {
    format!("Rain Classroom could not be reached: {error}")
}

fn io_error(error: io::Error) -> String {
    format!("Could not save Rain Classroom content: {error}")
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
struct LectureAvailabilityRecord {
    lesson_id: String,
    code: i64,
    message: String,
    has_recording: bool,
    presentation_count: usize,
}

struct LectureAvailability {
    has_recording: bool,
    presentation_count: usize,
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
    #[serde(default)]
    presentations: Vec<String>,
}

#[derive(Deserialize)]
struct ReplayEntry {
    url: String,
    #[serde(rename = "hiddenStatus", default)]
    hidden_status: u8,
}

#[derive(Deserialize)]
struct PresentationResponse {
    code: i64,
    #[serde(default)]
    msg: String,
    data: Option<PresentationData>,
}

#[derive(Deserialize)]
struct PresentationData {
    presentation: PresentationRecord,
    #[serde(default)]
    slides: Vec<SlideRecord>,
}

#[derive(Deserialize)]
struct PresentationRecord {
    #[serde(default = "default_slide_width")]
    width: u32,
    #[serde(default = "default_slide_height")]
    height: u32,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
struct SlideRecord {
    index: u32,
    cover: String,
}

fn default_slide_width() -> u32 {
    1280
}

fn default_slide_height() -> u32 {
    720
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn logout_removes_only_the_dedicated_profile() {
        let root = tempfile::tempdir().unwrap();
        let rain_classroom = RainClassroom::new(root.path());
        tokio::fs::create_dir_all(&rain_classroom.profile)
            .await
            .unwrap();
        tokio::fs::write(rain_classroom.profile.join("Cookies"), b"session")
            .await
            .unwrap();

        rain_classroom.logout().await.unwrap();

        assert!(!rain_classroom.profile.exists());
        assert!(root.path().exists());
    }

    #[tokio::test]
    async fn logout_does_not_interrupt_an_active_import() {
        let root = tempfile::tempdir().unwrap();
        let rain_classroom = RainClassroom::new(root.path());
        tokio::fs::create_dir_all(&rain_classroom.profile)
            .await
            .unwrap();
        let _registration = rain_classroom.register_download("active-import").unwrap();

        let error = rain_classroom.logout().await.unwrap_err();

        assert!(error.contains("active Rain Classroom import"));
        assert!(rain_classroom.profile.exists());
    }

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
                "resource": "preparing",
                "phase": "preparing",
                "downloaded_bytes": 0,
                "total_bytes": null,
                "completed_items": null,
                "total_items": null
            })
        );

        rain_classroom.update_download("browser-import-1", |progress| {
            progress.resource = "recording";
            progress.phase = "downloading";
            progress.downloaded_bytes = 125;
            progress.total_bytes = Some(500);
        });
        let progress = rain_classroom
            .download_progress("browser-import-1")
            .unwrap();
        assert_eq!(progress.phase, "downloading");
        assert_eq!(progress.resource, "recording");
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
                "presentations": ["1765600451302821888"],
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
        assert_eq!(data.presentations, ["1765600451302821888"]);
        let public = serde_json::to_value(Lecture {
            lesson_id: "lesson".into(),
            title: "并发编程".into(),
            has_recording: true,
            presentation_count: 1,
        })
        .unwrap();
        assert!(public.get("segments").is_none());
        assert!(public.get("live").is_none());
    }

    #[test]
    fn presentation_details_keep_signed_page_urls_private() {
        let response: PresentationResponse = serde_json::from_value(serde_json::json!({
            "code": 0,
            "msg": "OK",
            "data": {
                "presentation": {"title": "04-adv-types", "width": 841, "height": 473},
                "slides": [
                    {"index": 2, "cover": "https://media.example/second.jpg?signature=secret"},
                    {"index": 1, "cover": "https://media.example/first.jpg?signature=secret"}
                ]
            }
        }))
        .unwrap();
        let data = response.data.unwrap();
        assert_eq!(data.presentation.width, 841);
        assert_eq!(data.presentation.height, 473);
        assert_eq!(data.slides.len(), 2);

        let public = serde_json::to_value(Presentation {
            presentation_id: "1765600451302821888".into(),
            title: data.presentation.title,
            page_count: data.slides.len(),
        })
        .unwrap();
        assert!(public.get("slides").is_none());
        assert!(public.to_string().find("signature").is_none());
    }
}
