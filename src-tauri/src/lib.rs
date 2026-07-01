use std::hash::{Hash, Hasher};
use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_updater::UpdaterExt;

const HOTKEY: &str = "CommandOrControl+Shift+C";

/// Windows flag so spawned CLI processes don't flash a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run the `claude` CLI in non-interactive mode against the captured image
/// and return its textual answer. Invoked from the frontend.
///
/// Async + spawn_blocking so the (potentially many-second) CLI call runs off
/// the main thread — otherwise the window freezes ("App not responding").
#[tauri::command]
async fn ask_claude(
    question: String,
    image_path: String,
    session_id: Option<String>,
    session_cwd: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_claude(question, image_path, session_id, session_cwd)
    })
    .await
    .map_err(|e| format!("Internal task error: {e}"))?
}

fn run_claude(
    question: String,
    image_path: String,
    session_id: Option<String>,
    session_cwd: Option<String>,
) -> Result<String, String> {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};

    let prompt = format!(
        "{q}\n\nA screenshot relevant to this question is saved at this path:\n{p}\n\nRead/view that image file, then answer the question about it.",
        q = question.trim(),
        p = image_path
    );

    let claude = resolve_claude();

    // Optionally resume a chosen session (in its own project directory), then feed
    // the prompt via stdin — `--allowedTools` is variadic and would otherwise
    // swallow a prompt arg. CREATE_NO_WINDOW hides the console flash.
    let mut command = Command::new(&claude);
    if let Some(id) = session_id.as_deref() {
        if !id.is_empty() {
            command.args(["--resume", id]);
        }
    }
    command.args(["-p", "--allowedTools", "Read"]);
    if let Some(cwd) = session_cwd.as_deref() {
        if !cwd.is_empty() && Path::new(cwd).is_dir() {
            command.current_dir(cwd);
        }
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Couldn't launch claude ({claude}): {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or("Couldn't open claude's stdin")?;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("Failed to send prompt to claude: {e}"))?;
        // stdin drops here, signalling end-of-input to claude.
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed while waiting for claude: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "claude exited with an error.\n\n{}\n{}",
            stderr.trim(),
            stdout.trim()
        ));
    }

    let answer = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if answer.is_empty() {
        Err("claude returned an empty response.".into())
    } else {
        Ok(answer)
    }
}

/// A past Claude conversation the user can drop a snip into.
#[derive(serde::Serialize)]
struct SessionInfo {
    id: String,
    title: String,
    project: String,      // absolute cwd of the session's project
    project_name: String, // last path segment, for display
    modified: u64,        // unix seconds, for "x ago" labels
}

/// List the most recent Claude sessions found on disk so the UI can offer them.
#[tauri::command]
async fn list_sessions() -> Result<Vec<SessionInfo>, String> {
    tauri::async_runtime::spawn_blocking(scan_sessions)
        .await
        .map_err(|e| format!("Internal task error: {e}"))?
}

fn scan_sessions() -> Result<Vec<SessionInfo>, String> {
    use std::time::UNIX_EPOCH;

    let home = std::env::var("USERPROFILE").map_err(|_| "USERPROFILE not set")?;
    let projects = std::path::Path::new(&home).join(".claude").join("projects");

    // Collect every transcript with its last-modified time.
    let mut files: Vec<(std::path::PathBuf, u64)> = Vec::new();
    let dirs = std::fs::read_dir(&projects).map_err(|e| e.to_string())?;
    for dir in dirs.flatten() {
        if !dir.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(dir.path()) {
            for f in entries.flatten() {
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let mtime = f
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                files.push((p, mtime));
            }
        }
    }

    // Newest first, and only bother parsing titles for the most recent handful.
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(40);

    let mut out = Vec::new();
    for (path, mtime) in files {
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let (title, cwd) = session_meta(&path);
        let project = cwd.unwrap_or_default();
        let project_name = std::path::Path::new(&project)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push(SessionInfo {
            id,
            title: title.unwrap_or_else(|| "(untitled session)".into()),
            project,
            project_name,
            modified: mtime,
        });
    }
    Ok(out)
}

/// Pull a display title (first real user prompt) and the project cwd from a
/// transcript, reading only the first several lines.
fn session_meta(path: &std::path::Path) -> (Option<String>, Option<String>) {
    use std::io::{BufRead, BufReader};

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None),
    };
    let reader = BufReader::new(file);

    let mut title: Option<String> = None;
    let mut cwd: Option<String> = None;

    for line in reader.lines().take(80).map_while(Result::ok) {
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                if !c.is_empty() {
                    cwd = Some(c.to_string());
                }
            }
        }

        if title.is_none() && v.get("type").and_then(|t| t.as_str()) == Some("user") {
            let content = v.get("message").and_then(|m| m.get("content"));
            let text = match content {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Array(arr)) => arr
                    .iter()
                    .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .and_then(|b| b.get("text").and_then(|t| t.as_str()))
                    .unwrap_or("")
                    .to_string(),
                _ => String::new(),
            };
            let text = text.trim();
            // Skip system reminders, command wrappers, and caveat preambles.
            if !text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:") {
                let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let short: String = clean.chars().take(70).collect();
                title = Some(if clean.chars().count() > 70 {
                    format!("{short}…")
                } else {
                    short
                });
            }
        }

        if title.is_some() && cwd.is_some() {
            break;
        }
    }

    (title, cwd)
}

/// Check GitHub releases for a newer signed build, if any.
async fn get_update(app: &tauri::AppHandle) -> Option<tauri_plugin_updater::Update> {
    match app.updater() {
        Ok(updater) => updater.check().await.ok().flatten(),
        Err(_) => None,
    }
}

/// Download and install the latest update, then relaunch.
async fn install_update(app: tauri::AppHandle) {
    if let Some(update) = get_update(&app).await {
        if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
            app.restart();
        }
    }
}

/// Hide the main window (called from the frontend Close button / Escape key).
#[tauri::command]
fn hide_main(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

/// Locate the claude executable: prefer PATH, fall back to the standard
/// per-user install location.
fn resolve_claude() -> String {
    if let Ok(home) = std::env::var("USERPROFILE") {
        let candidate = format!("{home}\\.local\\bin\\claude.exe");
        if std::path::Path::new(&candidate).exists() {
            return candidate;
        }
    }
    "claude".to_string()
}

/// A cheap signature of whatever image currently sits on the clipboard.
/// Returns (width, height, rgba_bytes, hash) or None if there is no image.
fn clipboard_image() -> Option<(usize, usize, Vec<u8>, u64)> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    let bytes = img.bytes.into_owned();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    img.width.hash(&mut hasher);
    img.height.hash(&mut hasher);
    bytes.hash(&mut hasher);

    Some((img.width, img.height, bytes, hasher.finish()))
}

/// Save RGBA bytes to a temp PNG and return its absolute path.
fn save_png(width: usize, height: usize, bytes: &[u8]) -> Result<String, String> {
    let img = image::RgbaImage::from_raw(width as u32, height as u32, bytes.to_vec())
        .ok_or("clipboard image had an unexpected buffer size")?;

    let dir = std::env::temp_dir().join("whatsit");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("shot-{stamp}.png"));

    img.save(&path).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// Move the popup just below-right of the mouse cursor, clamped on-screen.
fn position_near_cursor(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    let cursor = match app.cursor_position() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut x = cursor.x + 12.0;
    let mut y = cursor.y + 12.0;

    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten());

    if let (Ok(size), Some(monitor)) = (win.outer_size(), monitor) {
        let mpos = monitor.position();
        let msize = monitor.size();
        let min_x = mpos.x as f64;
        let min_y = mpos.y as f64;
        let max_x = min_x + msize.width as f64 - size.width as f64 - 8.0;
        let max_y = min_y + msize.height as f64 - size.height as f64 - 8.0;
        x = x.min(max_x).max(min_x);
        y = y.min(max_y).max(min_y);
    }

    let _ = win.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
}

/// Trigger Windows' native screen-clip overlay, then wait for the cropped
/// image to appear on the clipboard. Runs on its own thread.
fn run_capture(app: tauri::AppHandle) {
    // Remember what was on the clipboard before, so we only react to a NEW crop.
    let before = clipboard_image().map(|(_, _, _, h)| h);

    // Launch the same overlay that Win+Shift+S uses.
    if let Err(e) = std::process::Command::new("explorer.exe")
        .arg("ms-screenclip:")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        eprintln!("failed to launch screen clip: {e}");
        return;
    }

    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if Instant::now() > deadline {
            return; // user cancelled or took too long
        }
        std::thread::sleep(Duration::from_millis(400));

        if let Some((w, h, bytes, hash)) = clipboard_image() {
            if before != Some(hash) {
                match save_png(w, h, &bytes) {
                    Ok(path) => {
                        let _ = app.emit("screenshot-captured", path);
                        if let Some(win) = app.get_webview_window("main") {
                            position_near_cursor(&app, &win);
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                    }
                    Err(e) => eprintln!("failed to save screenshot: {e}"),
                }
                return;
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let global_shortcut = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let app = app.clone();
                std::thread::spawn(move || run_capture(app));
            }
        })
        .build();

    tauri::Builder::default()
        .plugin(global_shortcut)
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![ask_claude, hide_main, list_sessions])
        .setup(|app| {
            // Register the global hotkey.
            app.global_shortcut().register(HOTKEY)?;

            // System tray with a small menu.
            let capture_item =
                MenuItem::with_id(app, "capture", "Capture (Ctrl+Shift+C)", true, None::<&str>)?;
            let update_item =
                MenuItem::with_id(app, "update", "Check for updates", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Whatsit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&capture_item, &update_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Whatsit — Ctrl+Shift+C to snip & ask Claude")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "capture" => {
                        let app = app.clone();
                        std::thread::spawn(move || run_capture(app));
                    }
                    "update" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(install_update(app));
                    }
                    _ => {}
                })
                .build(app)?;

            // Check for an update in the background; surface it in the tray if found.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Some(update) = get_update(&app_handle).await {
                    let _ = update_item.set_text(format!("⬆  Install update v{}", update.version));
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window just hides it; the app keeps living in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Whatsit");
}
