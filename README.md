# Whatsit

**Snip → ask Claude.** A tiny Windows tray app: press a hotkey, drag a box
around anything on screen, and ask Claude about it — answer appears in a small
popup right at your cursor.

> Built with Tauri v2 + Rust. ~9 MB installer, ~28 MB RAM. No Electron.

![Whatsit demo](docs/demo.gif)

---

## What it does

1. Lives in your **system tray** (right-click → Capture / Quit).
2. Press **`Ctrl+Shift+C`** → the native Windows snip overlay appears.
3. Drag a box → a compact popup opens **near your cursor** showing the crop.
4. Type a question (or just press **Enter** for *“What is this?”*).
5. Claude's answer streams into the popup, rendered as markdown.

The snip is the hero of the UI, with a translucent **caption bar** (logo +
dimensions) overlaid on it — ultra-compact, designed to feel like it belongs
next to your cursor, not a big window in the middle of the screen.

## Requirements

- **Windows 10/11** (uses the built-in `ms-screenclip:` snip overlay).
- The **[Claude CLI](https://code.claude.com/docs/en/overview)** installed and
  logged in (`claude` on your PATH, or at `%USERPROFILE%\.local\bin\claude.exe`).

> This is a community project. It is **not** affiliated with or endorsed by
> Anthropic. It simply shells out to the official `claude` CLI on your machine.

## Install

- **Installer:** grab `Whatsit_0.1.0_x64-setup.exe` from
  [Releases](../../releases) and run it.
  (It's unsigned, so Windows SmartScreen may warn — click *More info → Run anyway*.)
- **Portable:** download `whatsit.exe` and run it directly.

To launch on startup, drop a shortcut to the exe in your Startup folder
(`Win+R` → `shell:startup`).

## Develop

```sh
npm install
npm run tauri dev      # hot-reload dev build
npm run tauri build    # release exe + installer
```

- **Frontend:** `index.html`, `src/main.ts`, `src/styles.css` (Vite + TypeScript)
- **Backend:** `src-tauri/src/lib.rs` (Tauri v2, Rust)

Prerequisites: Node.js, the Rust toolchain, and the MSVC C++ build tools +
Windows SDK (Tauri's standard Windows requirements).

## Configure

- **Hotkey** — `HOTKEY` in `src-tauri/src/lib.rs`
- **Default question** — `DEFAULT_QUESTION` in `src/main.ts`
- **Claude flags / prompt wording** — `run_claude` in `src-tauri/src/lib.rs`

## How it works

Rather than building a custom region-selection overlay, Whatsit reuses Windows'
own excellent snipper (`ms-screenclip:`) and reads the crop off the clipboard
(`arboard`), saves it to a temp PNG, then runs:

```
<your question> | claude -p --allowedTools Read   # image path passed in the prompt
```

The CLI call runs on a background thread so the UI never blocks, and the
answer is rendered with `marked` + sanitized with `DOMPurify`.

## License

[MIT](LICENSE) © 2026 Khaled BH
