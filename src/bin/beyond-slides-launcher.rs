#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{
    env,
    error::Error,
    ffi::OsString,
    fs::{self, OpenOptions},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const DEFAULT_PORT: u16 = 7842;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_ON_STDIN_CLOSE_ENV: &str = "BEYOND_SLIDES_SHUTDOWN_ON_STDIN_CLOSE";

fn main() {
    if let Err(error) = run() {
        show_error(&format!("BeyondSlides could not start.\n\n{error}"));
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let application_directory = env::current_exe()?
        .parent()
        .ok_or("the launcher has no parent directory")?
        .to_owned();
    let backend = application_directory.join(format!("beyond-slides{}", env::consts::EXE_SUFFIX));
    if !backend.is_file() {
        return Err(format!(
            "The package is incomplete: {} is missing.",
            backend.display()
        )
        .into());
    }

    let browser = match env::var_os("BEYOND_SLIDES_BROWSER_EXECUTABLE") {
        Some(configured) => PathBuf::from(configured),
        None => beyond_slides::browser_runtime::packaged_chromium()?
            .ok_or("the packaged Chromium executable is missing")?
            .executable()
            .to_owned(),
    };
    if !browser.is_file() {
        return Err(format!("The browser executable {} is missing.", browser.display()).into());
    }

    let data_directory = match env::var_os("BEYOND_SLIDES_DATA_DIR") {
        Some(configured) => PathBuf::from(configured),
        None => dirs::data_local_dir()
            .ok_or("Windows did not provide a local application-data directory")?
            .join("BeyondSlides"),
    };
    fs::create_dir_all(&data_directory)?;
    let port = configured_or_available_port()?;
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let log_path = data_directory.join("controller.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let mut server_command = Command::new(&backend);
    server_command
        .arg("serve")
        .arg(&data_directory)
        .arg(port.to_string())
        .env(
            beyond_slides::runtime_tools::RUNTIME_TOOLS_DIRECTORY_ENV,
            application_directory.join("runtime-tools"),
        )
        .env("BEYOND_SLIDES_BIND_ADDRESS", "127.0.0.1")
        .env(SHUTDOWN_ON_STDIN_CLOSE_ENV, "1")
        .env_remove("BEYOND_SLIDES_TRUSTED_ORIGINS")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    hide_console(&mut server_command);
    let server = server_command.spawn().map_err(|error| {
        format!(
            "Could not launch {}: {error}. Diagnostics: {}",
            backend.display(),
            log_path.display()
        )
    })?;
    let mut server = ChildGuard::new(server);
    wait_until_ready(&mut server, address, &log_path)?;

    let url = format!("http://127.0.0.1:{port}");
    let mut browser_command = Command::new(&browser);
    browser_command
        .arg(format!("--app={url}"))
        .arg(format!(
            "--user-data-dir={}",
            data_directory.join("application-browser").display()
        ))
        .arg("--no-first-run")
        .arg("--disable-default-apps")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(extra) = env::var_os("BEYOND_SLIDES_BROWSER_EXTRA_ARGS") {
        browser_command.args(split_extra_arguments(extra));
    }
    hide_console(&mut browser_command);
    let mut browser_process = browser_command.spawn().map_err(|error| {
        format!(
            "Could not launch the packaged browser {}: {error}",
            browser.display()
        )
    })?;
    let status = browser_process.wait()?;
    server.stop();
    if !status.success() {
        return Err(format!("The application window exited with status {status}.").into());
    }
    Ok(())
}

fn configured_or_available_port() -> Result<u16, Box<dyn Error>> {
    if let Some(configured) = env::var_os("BEYOND_SLIDES_PORT") {
        return Ok(configured
            .to_str()
            .ok_or("BEYOND_SLIDES_PORT is not valid Unicode")?
            .parse()?);
    }
    let preferred = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT);
    if TcpListener::bind(preferred).is_ok() {
        return Ok(DEFAULT_PORT);
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn wait_until_ready(
    server: &mut ChildGuard,
    address: SocketAddr,
    log_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        if let Some(status) = server.child.try_wait()? {
            return Err(format!(
                "The controller exited with status {status}. Diagnostics: {}",
                log_path.display()
            )
            .into());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "The controller did not become ready within {} seconds. Diagnostics: {}",
                STARTUP_TIMEOUT.as_secs(),
                log_path.display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn split_extra_arguments(arguments: OsString) -> Vec<OsString> {
    arguments
        .to_string_lossy()
        .split_whitespace()
        .map(OsString::from)
        .collect()
}

struct ChildGuard {
    child: Child,
    stopped: bool,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self {
            child,
            stopped: false,
        }
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }

        // The packaged server watches this private pipe. Closing it lets Axum
        // drain and, importantly, gives Rain Classroom's Chromium session a
        // chance to close before the controller process exits.
        drop(self.child.stdin.take());
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.stopped = true;
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }

        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.stopped = true;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(windows)]
fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let title = "BeyondSlides\0".encode_utf16().collect::<Vec<_>>();
    let text = format!("{message}\0").encode_utf16().collect::<Vec<_>>();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that live
    // for the duration of the synchronous MessageBoxW call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_error(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_browser_arguments_are_split_for_packaging_smoke_tests() {
        assert_eq!(
            split_extra_arguments("--headless=new --dump-dom".into()),
            ["--headless=new", "--dump-dom"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }
}
