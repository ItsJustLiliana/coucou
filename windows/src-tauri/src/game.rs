//! Local game detection. Retains the detected process across focus changes; no process scanning or network calls.
use tauri::{AppHandle, Manager};
use std::sync::atomic::{AtomicBool, Ordering};

static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[cfg(windows)]
static TRACKED: std::sync::Mutex<Option<TrackedGame>> = std::sync::Mutex::new(None);

#[cfg(windows)]
struct TrackedGame {
    handle: std::os::windows::io::OwnedHandle,
    session: GameSession,
}

#[cfg(windows)]
#[derive(Clone, PartialEq)]
struct GameSession {
    name: String,
    started_at_ms: u64,
}

#[cfg(windows)]
fn process_started_at_ms(handle: windows::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetProcessTimes;
    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user).ok()?; }
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    ticks.checked_sub(116_444_736_000_000_000).map(|ticks| ticks / 10_000)
}

static GAME_FOCUSED: AtomicBool = AtomicBool::new(false);

static PERFORMANCE: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn performance_active() -> bool { PERFORMANCE.load(Ordering::Relaxed) }

/// Hidden game cursors still have desktop coordinates: never hit-test them.
#[tauri::command]
pub fn pointer_blocked() -> bool {
    performance_active() || (GAME_FOCUSED.load(Ordering::Relaxed) && cursor_hidden())
}

fn cursor_hidden() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING};
        let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        return GetCursorInfo(&mut info).is_ok() && (info.flags.0 & CURSOR_SHOWING.0) == 0;
    }
    #[cfg(not(windows))]
    false
}

fn apply_pointer_protection(app: &AppHandle) {
    use tauri::Emitter;
    static LAST: AtomicBool = AtomicBool::new(false);
    let blocked = pointer_blocked();
    if LAST.swap(blocked, Ordering::Relaxed) == blocked { return; }
    if let Some(shared) = app.try_state::<crate::Shared>() {
        if blocked { crate::island::set_ignore_cursor(app, true); }
        else { crate::island::refresh_click_through(app, &shared.gate); }
    }
    crate::desktop::pointer_protection_changed(app, blocked);
    let _ = app.emit("pointer-blocked", blocked);
}

#[cfg(windows)]
fn watch_input_changes() {
    // Out-of-context accessibility events need their own sleeping message loop.
    // They observe focus/cursor visibility; they do not hook game input.
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn changed(_: HWINEVENTHOOK, event: u32, _: HWND, object: i32, _: i32, _: u32, _: u32) {
        if event == EVENT_SYSTEM_FOREGROUND || object == OBJID_CURSOR.0 { WAKE.notify_one(); }
    }
    let _ = std::thread::Builder::new().name("game-focus".into()).spawn(|| unsafe {
        let _focus = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(changed), 0, 0, WINEVENT_OUTOFCONTEXT);
        let _cursor = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE, None, Some(changed), 0, 0, WINEVENT_OUTOFCONTEXT);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
}

pub fn set_performance(app: &AppHandle, on: bool) {
    // Serialize window changes on the UI thread, without holding a mutex while
    // waiting on that thread (settings commands can originate there too).
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || apply_performance(&handle, on));
}

fn apply_performance(app: &AppHandle, on: bool) {
    use tauri::Emitter;
    let changed = PERFORMANCE.swap(on, Ordering::Relaxed) != on;
    apply_pointer_protection(app);
    if !changed { return; }
    if let Some(shared) = app.try_state::<crate::Shared>() {
        shared.gate.set_suspended(on);
        let pref = shared.settings.lock().unwrap().screen.clone();
        let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
        crate::island::apply_geometry(app, &pref, collapsed);
    }
    crate::desktop::performance_changed(app, on);
    #[cfg(windows)]
    if on { crate::beat::performance_suspend(app); }
    let _ = app.emit("game-performance", on);
}

pub fn start(app: AppHandle) {
    #[cfg(windows)]
    watch_input_changes();
    #[cfg(windows)]
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(3));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { _ = timer.tick() => {}, _ = WAKE.notified() => {} }
            sync(&app, false);
        }
    });
    #[cfg(not(windows))]
    let _ = app;
}

fn detection_enabled(settings: &crate::settings::Settings) -> bool {
    settings.game_optimization || settings.active_integrations.iter().any(|id| id == ID)
}

fn optimization_wanted(settings: &crate::settings::Settings, game_open: bool, focused: bool) -> bool {
    settings.game_optimization && (if settings.game_optimization_auto { game_open } else { focused })
}

pub fn sync(app: &AppHandle, force: bool) {
    let enabled = app.try_state::<crate::Shared>().is_some_and(|shared|
        detection_enabled(&shared.settings.lock().unwrap()));
    if enabled {
        poll(app, force);
    } else {
        GAME_FOCUSED.store(false, Ordering::Relaxed);
        set_performance(app, false);
    }
}

pub const ID: &str = "integration_game";

// Manually registered executable names are stored locally, not sent to a server.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredGame {
    pub executable: String,
    pub name: String,
}

fn registry_path() -> std::path::PathBuf {
    crate::platform::config_dir().join("registered-games.json")
}

fn registered_games() -> Vec<RegisteredGame> {
    std::fs::read_to_string(registry_path()).ok()
        .and_then(|s| serde_json::from_str::<Vec<RegisteredGame>>(&s).ok())
        .unwrap_or_default()
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningApplication {
    pub executable: String,
    pub title: String,
}

#[tauri::command]
pub fn game_list_running() -> Vec<RunningApplication> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible
        };
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION
        };
        use windows::core::{BOOL, PWSTR};
        unsafe extern "system" fn collect(hwnd: HWND, param: LPARAM) -> BOOL {
            let entries = &mut *(param.0 as *mut Vec<RunningApplication>);
            if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) <= 0 {
                return BOOL(1);
            }
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == std::process::id() { return BOOL(1); }
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return BOOL(1);
            };
            let mut name = vec![0u16; 32768];
            let mut len = name.len() as u32;
            let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(name.as_mut_ptr()), &mut len);
            let _ = windows::Win32::Foundation::CloseHandle(process);
            if result.is_err() { return BOOL(1); }
            let full_path = String::from_utf16_lossy(&name[..len as usize]);
            let executable = full_path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
            if executable.is_empty() || ["explorer.exe", "applicationframehost.exe", "shellexperiencehost.exe"]
                .iter().any(|p| executable.eq_ignore_ascii_case(p)) { return BOOL(1); }
            let mut title = vec![0u16; (GetWindowTextLengthW(hwnd) as usize + 1).min(1024)];
            let size = GetWindowTextW(hwnd, &mut title);
            if size <= 0 { return BOOL(1); }
            let title = String::from_utf16_lossy(&title[..size as usize]);
            if !entries.iter().any(|a| a.executable.eq_ignore_ascii_case(&executable)) {
                entries.push(RunningApplication { executable, title });
            }
            BOOL(1)
        }
        let mut entries: Vec<RunningApplication> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM((&mut entries as *mut Vec<RunningApplication>) as isize));
        }
        entries.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        entries
    }
    #[cfg(not(windows))]
    { Vec::new() }
}

#[tauri::command]
pub fn game_list_registered() -> Vec<RegisteredGame> { registered_games() }

#[tauri::command]
pub fn game_register(executable: String, name: String) -> Result<Vec<RegisteredGame>, String> {
    let executable = executable.trim().rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    let name = name.trim();
    if !executable.ends_with(".exe") || executable.len() > 128 || executable.len() < 5
        || !executable.chars().all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c))
        || name.is_empty() || name.len() > 80
    {
        return Err("Enter a valid Windows .exe filename and game name.".into());
    }
    let mut games = registered_games();
    games.retain(|g| !g.executable.eq_ignore_ascii_case(&executable));
    games.push(RegisteredGame { executable, name: name.to_string() });
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let path = registry_path();
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    std::fs::write(path, serde_json::to_vec_pretty(&games).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(games)
}

#[tauri::command]
pub fn game_unregister(executable: String) -> Result<Vec<RegisteredGame>, String> {
    let mut games = registered_games();
    games.retain(|g| !g.executable.eq_ignore_ascii_case(executable.trim()));
    std::fs::write(registry_path(), serde_json::to_vec_pretty(&games).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(games)
}

// Fullscreen alone is deliberately not evidence of a game.
fn game_name(path: &str) -> Option<String> {
    let normalized = path.replace('/', "\\");
    let lower = normalized.to_lowercase();
    let exe = lower.rsplit('\\').next()?;
    if ["launcher", "crash", "setup", "unins", "helper", "report", "redist"]
        .iter().any(|word| exe.contains(word)) { return None; }
    let known = match exe {
        "valorant-win64-shipping.exe" => Some("VALORANT"),
        "fortniteclient-win64-shipping.exe" => Some("Fortnite"),
        "league of legends.exe" => Some("League of Legends"),
        "overwatch.exe" => Some("Overwatch"),
        "minecraft.windows.exe" => Some("Minecraft"),
        "rocketleague.exe" => Some("Rocket League"),
        "eldenring.exe" => Some("Elden Ring"),
        "cs2.exe" => Some("Counter-Strike 2"),
        "gta5.exe" => Some("Grand Theft Auto V"),
        "genshinimpact.exe" => Some("Genshin Impact"),
        "starrail.exe" => Some("Honkai: Star Rail"),
        "ffxiv_dx11.exe" => Some("Final Fantasy XIV"),
        "wow.exe" => Some("World of Warcraft"),
        "minecraftlauncher.exe" => None,
        "r5apex.exe" => Some("Apex Legends"),
        _ => None,
    };
    if let Some(name) = known { return Some(name.into()); }
    let parts: Vec<_> = normalized.split('\\').collect();
    let index = parts.windows(2).position(|pair|
        pair[0].eq_ignore_ascii_case("steamapps") && pair[1].eq_ignore_ascii_case("common"))?;
    let folder = *parts.get(index + 2)?;
    if folder.is_empty() || folder.eq_ignore_ascii_case("Steamworks Shared") { return None; }
    Some(folder.into())
}

#[cfg(windows)]
fn foreground_game() -> Option<TrackedGame> {
    use std::os::windows::io::FromRawHandle;
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_NAME_WIN32, PROCESS_SYNCHRONIZE};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() { return None; }
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        if pid == 0 { return None; }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid).ok()?;
        let mut buffer = vec![0u16; 32768];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()), &mut len);
        let owned = std::os::windows::io::OwnedHandle::from_raw_handle(process.0);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        let exe = path.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
        let name = registered_games().into_iter()
            .find(|game| game.executable.eq_ignore_ascii_case(&exe))
            .map(|game| game.name)
            .or_else(|| game_name(&path))?;
        // Retain this exact process handle across alt-tab, avoiding PID reuse.
        let started_at_ms = process_started_at_ms(process).unwrap_or_else(||
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64);
        Some(TrackedGame { handle: owned, session: GameSession { name, started_at_ms } })
    }
}

#[cfg(windows)]
fn open_game() -> (Option<GameSession>, bool) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{HANDLE, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::WaitForSingleObject;
    let mut tracked = TRACKED.lock().unwrap();
    let foreground = foreground_game();
    let focused = foreground.is_some();
    if let Some(mut game) = foreground {
        // Preserve the same session even if the creation-time query briefly fails.
        if let Some(previous) = tracked.as_ref() {
            use windows::Win32::System::Threading::GetProcessId;
            let pid = unsafe { GetProcessId(HANDLE(game.handle.as_raw_handle())) };
            if pid != 0 && unsafe { GetProcessId(HANDLE(previous.handle.as_raw_handle())) } == pid {
                game.session.started_at_ms = previous.session.started_at_ms;
            }
        }
        *tracked = Some(game);
    }
    let running = tracked.as_ref().is_some_and(|game| unsafe {
        WaitForSingleObject(HANDLE(game.handle.as_raw_handle()), 0) == WAIT_TIMEOUT
    });
    if !running { *tracked = None; }
    (tracked.as_ref().map(|game| game.session.clone()), focused)
}

/// Focus only the exact live process detected earlier; never start a new game.
#[tauri::command]
pub async fn game_focus() -> bool {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(focus_tracked_game).await.unwrap_or(false);
    #[cfg(not(windows))]
    false
}

#[cfg(windows)]
fn focus_tracked_game() -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{HANDLE, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{GetProcessId, WaitForSingleObject};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let Some((handle, name)) = TRACKED.lock().unwrap().as_ref()
        .and_then(|game| game.handle.try_clone().ok().map(|handle| (handle, game.session.name.clone())))
        else { return false; };
    let process = HANDLE(handle.as_raw_handle());
    unsafe {
        if WaitForSingleObject(process, 0) != WAIT_TIMEOUT { return false; }
        let pid = GetProcessId(process);
        if pid == 0 { return false; }
        let _ = crate::platform::focus_process_window(pid, &name);
        // Windows can decline focus or complete it asynchronously. Check the
        // actual foreground owner instead of treating an attempted call as success.
        for _ in 0..20 {
            if WaitForSingleObject(process, 0) != WAIT_TIMEOUT { return false; }
            let mut foreground = 0;
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut foreground));
            if foreground == pid { return true; }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }
}

pub fn poll(app: &AppHandle, force: bool) {
    #[cfg(windows)]
    {
        use tauri::Emitter;
        let (game, focused) = open_game();
        GAME_FOCUSED.store(focused, Ordering::Relaxed);
        // Cursor visibility can change in a pause menu without the game
        // process or optimization mode changing.
        apply_pointer_protection(app);
        let optimize = app.try_state::<crate::Shared>().is_some_and(|shared|
            optimization_wanted(&shared.settings.lock().unwrap(), game.is_some(), focused));
        set_performance(app, optimize);
        static LAST: std::sync::Mutex<Option<Option<GameSession>>> = std::sync::Mutex::new(None);
        let mut last = LAST.lock().unwrap();
        if !force && last.as_ref() == Some(&game) { return; }
        *last = Some(game.clone());
        let _ = app.emit_to(crate::island::WINDOW_LABEL, "integration",
            crate::integrations::IntegrationUpdate {
                id: ID, data: serde_json::json!({"playing": game.is_some(), "name": game.as_ref().map(|game| &game.name), "startedAtMs": game.as_ref().map(|game| game.started_at_ms)}),
                error: None, event: None,
            });
    }
    #[cfg(not(windows))]
    let _ = (app, force);
}

#[cfg(test)]
mod tests {
    use super::game_name;
    #[cfg(windows)]
    #[test]
    fn session_time_uses_the_actual_process_creation_time() {
        let started = super::process_started_at_ms(unsafe { windows::Win32::System::Threading::GetCurrentProcess() }).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        assert!(started > 1_000_000_000_000 && started <= now);
    }
    #[test]
    fn background_automatic_mode_stays_active_until_the_game_closes() {
        let mut settings = crate::settings::Settings::default();
        settings.game_optimization = true;
        assert!(super::optimization_wanted(&settings, true, true));
        assert!(!super::optimization_wanted(&settings, true, false));
        settings.game_optimization_auto = true;
        assert!(super::optimization_wanted(&settings, true, false), "alt-tab must not stop automatic optimization");
        assert!(!super::optimization_wanted(&settings, false, false), "closing the game restores normal operation");
        settings.game_optimization = false;
        assert!(!super::optimization_wanted(&settings, true, true), "the main switch overrides automatic mode");
    }
    #[test]
    fn optimization_can_detect_games_without_a_visual_pill() {
        let mut settings = crate::settings::Settings::default();
        settings.active_integrations.clear();
        assert!(!super::detection_enabled(&settings));
        settings.game_optimization = true;
        assert!(super::detection_enabled(&settings));
    }
    #[test]
    fn visual_pill_does_not_enable_optimization() {
        let mut settings = crate::settings::Settings::default();
        settings.active_integrations = vec![super::ID.into()];
        assert!(super::detection_enabled(&settings));
        assert!(!settings.game_optimization);
        let restored: crate::settings::Settings = serde_json::from_str("{\"activeIntegrations\":[\"integration_game\"]}").unwrap();
        assert!(!restored.game_optimization, "older settings must not implicitly enable optimization");
    }
    #[cfg(windows)]
    #[test]
    fn focusing_without_a_tracked_game_fails_cleanly() {
        assert!(!super::focus_tracked_game());
    }
    #[test]
    fn recognizes_games_without_treating_every_app_as_a_game() {
        assert_eq!(game_name(r"D:\SteamLibrary\steamapps\common\Elden Ring\Game\eldenring.exe"), Some("Elden Ring".into()));
        assert_eq!(game_name(r"C:\Riot Games\VALORANT\VALORANT-Win64-Shipping.exe"), Some("VALORANT".into()));
        assert_eq!(game_name(r"C:\Battle.net\Overwatch\Overwatch.exe"), Some("Overwatch".into()));
        assert_eq!(game_name(r"D:\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe"), Some("Fortnite".into()));
        assert_eq!(game_name(r"C:\Games\Genshin Impact\GenshinImpact.exe"), Some("Genshin Impact".into()));
        assert_eq!(game_name(r"C:\Windows\explorer.exe"), None);
        assert_eq!(game_name(r"D:\steamapps\common\Game\launcher.exe"), None);
        assert_eq!(game_name(r"D:\steamapps\common\Steamworks Shared\tool.exe"), None);
    }
}
