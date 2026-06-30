use std::hash::{Hash, Hasher};
use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const HOTKEY: &str = "CommandOrControl+Shift+C";

/// Windows flag so spawned CLI processes don't flash a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run the `claude` CLI in non-interactive mode against the captured image
/// and return its textual answer. Invoked from the frontend.
///
/// Async + spawn_blocking so the (potentially many-second) CLI call runs off
/// the main thread — otherwise the window freezes ("App not responding").
#[tauri::command]
async fn ask_claude(question: String, image_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || run_claude(question, image_path))
        .await
        .map_err(|e| format!("Internal task error: {e}"))?
}

fn run_claude(question: String, image_path: String) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let prompt = format!(
        "{q}\n\nA screenshot relevant to this question is saved at this path:\n{p}\n\nRead/view that image file, then answer the question about it.",
        q = question.trim(),
        p = image_path
    );

    let claude = resolve_claude();

    // Pass the prompt via stdin (not as an arg): `--allowedTools` is variadic and
    // would otherwise swallow the prompt. CREATE_NO_WINDOW hides the console flash.
    let mut child = Command::new(&claude)
        .args(["-p", "--allowedTools", "Read"])
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
        .invoke_handler(tauri::generate_handler![ask_claude, hide_main])
        .setup(|app| {
            // Register the global hotkey.
            app.global_shortcut().register(HOTKEY)?;

            // System tray with a small menu.
            let capture_item =
                MenuItem::with_id(app, "capture", "Capture (Ctrl+Shift+C)", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Whatsit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&capture_item, &quit_item])?;

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
                    _ => {}
                })
                .build(app)?;

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
