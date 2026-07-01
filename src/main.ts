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

const appWindow = getCurrentWindow();
let currentPath = "";

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
}

listen<string>("screenshot-captured", async (event) => {
  currentPath = event.payload;
  resetCompose();
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

  try {
    const res = await invoke<string>("ask_claude", {
      question,
      imagePath: currentPath,
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
    close();
  } else if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    ask();
  }
});
