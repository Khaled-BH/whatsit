import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { marked } from "marked";
import DOMPurify from "dompurify";

marked.setOptions({ breaks: true, gfm: true });

/** Render Claude's markdown answer to safe HTML. */
function renderMarkdown(md: string): string {
  return DOMPurify.sanitize(marked.parse(md, { async: false }) as string);
}

const DEFAULT_QUESTION = "What is this? Explain briefly.";
const BODY_PAD = 16; // keep in sync with body padding in styles.css

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

/** Resize the OS window to hug the card (the "ultra-compact" feel). */
async function fitWindow() {
  await new Promise((r) => requestAnimationFrame(r));
  const w = card.offsetWidth + BODY_PAD * 2;
  const h = card.offsetHeight + BODY_PAD * 2;
  await appWindow.setSize(new LogicalSize(w, h));
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
  await fitWindow();
  q.focus();
});

shot.addEventListener("load", () => {
  if (shot.naturalWidth) {
    dims.textContent = `${shot.naturalWidth}×${shot.naturalHeight}`;
  }
  fitWindow();
});

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
