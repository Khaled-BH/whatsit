import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  getCurrentWindow,
  LogicalSize,
  PhysicalPosition,
  currentMonitor,
} from "@tauri-apps/api/window";
import { marked } from "marked";
import DOMPurify from "dompurify";

marked.setOptions({ breaks: true, gfm: true });

/** Render Claude's markdown answer to safe HTML. */
function renderMarkdown(md: string): string {
  return DOMPurify.sanitize(marked.parse(md, { async: false }) as string);
}

const DEFAULT_QUESTION = "What is this? Explain briefly.";
const BODY_PAD = 0; // keep in sync with body padding in styles.css

const card = document.getElementById("card") as HTMLElement;
const shot = document.getElementById("shot") as HTMLImageElement;
const dims = document.getElementById("dims") as HTMLElement;
const asked = document.getElementById("asked") as HTMLElement;
const q = document.getElementById("q") as HTMLInputElement;
const sendBtn = document.getElementById("send") as HTMLButtonElement;
const closeBtn = document.getElementById("close") as HTMLButtonElement;
const answer = document.getElementById("answer") as HTMLElement;
const answerBody = document.getElementById("answerBody") as HTMLElement;
const sessionPicker = document.getElementById("sessionPicker") as HTMLElement;
const sessionTrigger = document.getElementById("sessionTrigger") as HTMLButtonElement;
const sessionLabel = document.getElementById("sessionLabel") as HTMLElement;
const sessionPanel = document.getElementById("sessionPanel") as HTMLElement;
const sessionSearch = document.getElementById("sessionSearch") as HTMLInputElement;
const sessionList = document.getElementById("sessionList") as HTMLElement;

const appWindow = getCurrentWindow();
let currentPath = "";

interface Session {
  id: string;
  title: string;
  project: string;
  project_name: string;
  modified: number;
}

interface Entry {
  id: string;
  cwd: string;
  title: string;
  sub: string;
}

let sessions: Session[] = [];
let selected: { id: string; cwd: string } | null = null; // null = New chat
let activeIndex = 0;

function relTime(unixSec: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - unixSec);
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d ago`;
  return `${Math.floor(d / 7)}w ago`;
}

/** Rows currently shown, honoring the search filter. Index 0 is always New chat. */
function currentEntries(): Entry[] {
  const f = sessionSearch.value.trim().toLowerCase();
  const items = sessions
    .filter(
      (s) =>
        !f ||
        s.title.toLowerCase().includes(f) ||
        s.project_name.toLowerCase().includes(f),
    )
    .map((s) => ({
      id: s.id,
      cwd: s.project,
      title: s.title,
      sub: `${s.project_name || "?"} · ${relTime(s.modified)}`,
    }));
  return [{ id: "", cwd: "", title: "New chat", sub: "no session context" }, ...items];
}

function renderList() {
  const entries = currentEntries();
  activeIndex = Math.max(0, Math.min(activeIndex, entries.length - 1));
  sessionList.textContent = "";
  entries.forEach((e, i) => {
    const row = document.createElement("div");
    row.className = "session-item";
    if (i === activeIndex) row.classList.add("active");
    const isSel =
      (selected === null && e.id === "") ||
      (selected !== null && selected.id === e.id);
    if (isSel) row.classList.add("selected");

    const title = document.createElement("div");
    title.className = "session-item-title";
    title.textContent = e.title;
    const sub = document.createElement("div");
    sub.className = "session-item-sub";
    sub.textContent = e.sub;
    row.append(title, sub);

    row.addEventListener("mousedown", (ev) => {
      ev.preventDefault(); // keep focus, select immediately
      choose(e);
    });
    row.addEventListener("mouseenter", () => {
      activeIndex = i;
      highlightActive();
    });
    sessionList.appendChild(row);
  });
}

function highlightActive() {
  const n = sessionList.children.length;
  if (n === 0) return;
  activeIndex = Math.max(0, Math.min(activeIndex, n - 1));
  [...sessionList.children].forEach((c, i) =>
    c.classList.toggle("active", i === activeIndex),
  );
  (sessionList.children[activeIndex] as HTMLElement).scrollIntoView({
    block: "nearest",
  });
}

function choose(e: Entry) {
  selected = e.id ? { id: e.id, cwd: e.cwd } : null;
  sessionLabel.textContent = e.id ? e.title : "New chat";
  closePicker();
  q.focus();
}

function openPicker() {
  sessionPanel.hidden = false;
  sessionTrigger.classList.add("open");
  sessionSearch.value = "";
  activeIndex = 0;
  renderList();
  sessionSearch.focus();
  void fitWindow();
}

function closePicker() {
  if (sessionPanel.hidden) return;
  sessionPanel.hidden = true;
  sessionTrigger.classList.remove("open");
  void fitWindow();
}

/** Fetch recent sessions; refresh the list if the panel is open. */
async function loadSessions() {
  try {
    sessions = await invoke<Session[]>("list_sessions");
  } catch {
    return;
  }
  if (!sessionPanel.hidden) renderList();
}

sessionTrigger.addEventListener("click", (e) => {
  e.stopPropagation();
  if (sessionPanel.hidden) openPicker();
  else closePicker();
});

sessionSearch.addEventListener("input", () => {
  activeIndex = 0;
  renderList();
  void fitWindow();
});

sessionSearch.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    activeIndex++;
    highlightActive();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    activeIndex--;
    highlightActive();
  } else if (e.key === "Enter") {
    e.preventDefault();
    e.stopPropagation();
    const entry = currentEntries()[activeIndex];
    if (entry) choose(entry);
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    closePicker();
    q.focus();
  }
});

// Close the picker when clicking outside it.
document.addEventListener("mousedown", (e) => {
  if (!sessionPanel.hidden && !sessionPicker.contains(e.target as Node)) {
    closePicker();
  }
});

/** Resize the OS window to hug the card, then keep it fully on-screen. */
let fitScheduled = false;
async function fitWindow() {
  if (fitScheduled) return; // coalesce bursts (e.g. ResizeObserver) to one per frame
  fitScheduled = true;
  await new Promise((r) => requestAnimationFrame(r));
  fitScheduled = false;
  const w = Math.ceil(card.offsetWidth) + BODY_PAD * 2;
  const h = Math.ceil(card.offsetHeight) + BODY_PAD * 2 + 1; // +1 guards against subpixel clipping
  await appWindow.setSize(new LogicalSize(w, h));
  await clampOnScreen();
}

/** Nudge the window so it never hangs off the edge of the screen. */
async function clampOnScreen() {
  try {
    const mon = await currentMonitor();
    if (!mon) return;
    const pos = await appWindow.outerPosition();
    const size = await appWindow.outerSize();
    const margin = 8;
    const maxX = mon.position.x + mon.size.width - size.width - margin;
    const maxY = mon.position.y + mon.size.height - size.height - margin;
    const x = Math.max(mon.position.x + margin, Math.min(pos.x, maxX));
    const y = Math.max(mon.position.y + margin, Math.min(pos.y, maxY));
    if (x !== pos.x || y !== pos.y) {
      await appWindow.setPosition(new PhysicalPosition(x, y));
    }
  } catch {
    /* positioning is best-effort */
  }
}

/** Reset to the compose state for a fresh snip. */
function resetCompose() {
  card.classList.remove("asking");
  answer.classList.remove("done");
  answer.hidden = true;
  answerBody.textContent = "";
  asked.textContent = "";
  q.value = "";
  sendBtn.disabled = false;
  closePicker();
}

listen<string>("screenshot-captured", async (event) => {
  currentPath = event.payload;
  resetCompose();
  void loadSessions(); // refresh recency in the background
  shot.src = convertFileSrc(currentPath);
  // Wait for the image to actually decode so the card has its final height
  // before we size the window — otherwise it can come out too short.
  try {
    await shot.decode();
  } catch {
    /* fall back to the load event + ResizeObserver */
  }
  await fitWindow();
  q.focus();
});

shot.addEventListener("load", () => {
  if (shot.naturalWidth) {
    dims.textContent = `${shot.naturalWidth}×${shot.naturalHeight}`;
  }
  void fitWindow();
});

// Re-fit whenever the card changes size (image load, answer view, answer growing).
new ResizeObserver(() => {
  void fitWindow();
}).observe(card);

// Warm the session list at startup so the picker is populated before the first snip.
void loadSessions();

async function ask() {
  if (!currentPath || card.classList.contains("asking")) return;
  const question = q.value.trim() || DEFAULT_QUESTION;

  asked.textContent = question;
  card.classList.add("asking");
  answer.hidden = false;
  answer.classList.remove("done");
  answerBody.textContent = "";
  sendBtn.disabled = true;
  await fitWindow();

  const sessionId = selected?.id || null;
  const sessionCwd = selected?.cwd || null;

  try {
    const res = await invoke<string>("ask_claude", {
      question,
      imagePath: currentPath,
      sessionId,
      sessionCwd,
    });
    answerBody.innerHTML = renderMarkdown(res);
  } catch (err) {
    answerBody.textContent = String(err);
  } finally {
    answer.classList.add("done");
    await fitWindow();
  }
}

function close() {
  invoke("hide_main");
}

sendBtn.addEventListener("click", ask);
closeBtn.addEventListener("click", close);

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    if (!sessionPanel.hidden) {
      closePicker();
      q.focus();
      return;
    }
    close();
  } else if (e.key === "Enter" && !e.shiftKey) {
    if (!sessionPanel.hidden) return; // the search field handles Enter
    e.preventDefault();
    ask();
  }
});
