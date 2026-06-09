/* eslint-disable -- vendored third-party widget (lman80/SuperFeedback); do not lint */
// SuperFeedback web widget (web / Electron renderer / Tauri webview).
// ESM module. Usage:
//   import { SuperFeedback } from "./superfeedback.js";
//   SuperFeedback.init({ backendUrl, repo, app, appKey, position });
// or as a tag:  <script type="module"> import "./superfeedback.js";
//               SuperFeedback.init({ ... }); </script>
//
// It renders a floating button inside a Shadow DOM (so it can't clash with your
// app's CSS), captures a screenshot, and POSTs to your backend (see ../../PROTOCOL.md).
// For Electron/Tauri, pass `captureScreenshot` for a true native window grab
// (see ../../docs/screenshots.md); the default below is a DOM snapshot.

const CAPTURE_CDN = "https://esm.sh/html-to-image@1.11.13";

const SuperFeedback = {
  version: "1.0.0",
  _cfg: null,
  _host: null,

  init(config = {}) {
    if (!config.backendUrl || !config.repo) {
      console.error("[SuperFeedback] init requires { backendUrl, repo }");
      return;
    }
    this.destroy(); // idempotent re-init
    this._cfg = {
      position: "bottom-right",
      label: "Feedback",
      type: "bug",
      attachScreenshot: true,
      ...config,
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", () => this._mount());
    } else {
      this._mount();
    }
    if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
  },

  destroy() {
    if (this._host && this._host.parentNode) this._host.parentNode.removeChild(this._host);
    this._host = null;
  },

  _mount() {
    const host = document.createElement("div");
    host.setAttribute("data-superfeedback", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = TEMPLATE(this._cfg);
    document.body.appendChild(host);
    this._host = host;

    const $ = (sel) => root.querySelector(sel);
    const panel = $(".sf-panel");
    const btn = $(".sf-fab");
    const status = $(".sf-status");

    const open = () => { panel.classList.add("sf-open"); $(".sf-text").focus(); };
    const close = () => { panel.classList.remove("sf-open"); status.textContent = ""; status.className = "sf-status"; };

    btn.addEventListener("click", () => panel.classList.contains("sf-open") ? close() : open());
    $(".sf-cancel").addEventListener("click", close);

    $(".sf-send").addEventListener("click", async () => {
      const message = $(".sf-text").value.trim();
      if (!message) { setStatus(status, "Please describe the issue first.", "err"); return; }
      const type = $(".sf-type").value;
      const wantShot = $(".sf-shot").checked;
      const sendBtn = $(".sf-send");
      sendBtn.disabled = true;
      setStatus(status, wantShot ? "Capturing screenshot…" : "Sending…", "");

      let screenshot = null;
      if (wantShot) {
        host.style.visibility = "hidden";              // keep our UI out of the shot
        try { screenshot = await this._capture(); } catch (_) { /* send without */ }
        host.style.visibility = "visible";
      }

      setStatus(status, "Sending…", "");
      try {
        const res = await this._send({ message, type, screenshot });
        if (res.ok) {
          setStatus(status, "Thanks! Your feedback was sent. ✓", "ok");
          $(".sf-text").value = "";
          if (res.url) {
            const a = $(".sf-link");
            a.href = res.url; a.style.display = "inline"; a.textContent = `View #${res.number}`;
          }
          setTimeout(close, 2600);
        } else {
          setStatus(status, res.error || "Something went wrong.", "err");
        }
      } catch (e) {
        setStatus(status, "Couldn't reach the feedback server.", "err");
      } finally {
        sendBtn.disabled = false;
      }
    });
  },

  async _capture() {
    const cfg = this._cfg;
    if (typeof cfg.captureScreenshot === "function") return await cfg.captureScreenshot();
    // Default: DOM snapshot via html-to-image (loaded on demand). For higher
    // fidelity in Electron/Tauri, pass your own captureScreenshot instead.
    const mod = await import(/* @vite-ignore */ CAPTURE_CDN);
    return await mod.toPng(document.documentElement, {
      cacheBust: true,
      pixelRatio: Math.min(window.devicePixelRatio || 1, 2),
    });
  },

  async _send({ message, type, screenshot }) {
    const cfg = this._cfg;
    const meta = {
      url: location.href,
      platform: navigator.platform,
      userAgent: navigator.userAgent,
      locale: navigator.language,
      viewport: `${window.innerWidth}x${window.innerHeight}`,
      ...(cfg.appVersion ? { appVersion: cfg.appVersion } : {}),
      ...(cfg.meta || {}),
    };
    const res = await fetch(cfg.backendUrl.replace(/\/$/, "") + "/report", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        repo: cfg.repo,
        app: cfg.app || "",
        type,
        message,
        screenshot: screenshot || undefined,
        appKey: cfg.appKey || undefined,
        meta,
      }),
    });
    try { return await res.json(); }
    catch { return { ok: false, error: `HTTP ${res.status}` }; }
  },
};

function setStatus(el, msg, kind) {
  el.textContent = msg;
  el.className = "sf-status" + (kind ? " sf-" + kind : "");
}

function TEMPLATE(cfg) {
  const pos = {
    "bottom-right": "bottom:20px;right:20px;",
    "bottom-left": "bottom:20px;left:20px;",
    "top-right": "top:20px;right:20px;",
    "top-left": "top:20px;left:20px;",
  }[cfg.position] || "bottom:20px;right:20px;";
  const panelSide = cfg.position && cfg.position.includes("left") ? "left:0;" : "right:0;";
  const panelVert = cfg.position && cfg.position.includes("top") ? "top:64px;" : "bottom:64px;";
  return `
  <style>
    :host { all: initial; }
    .sf-wrap { position: fixed; ${pos} z-index: 2147483000;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
    .sf-fab { display: inline-flex; align-items: center; gap: 8px; cursor: pointer;
      border: none; border-radius: 999px; padding: 11px 16px; font-size: 14px; font-weight: 600;
      color: #fff; background: ${cfg.color || "#5b21b6"}; box-shadow: 0 6px 20px rgba(0,0,0,.22); }
    .sf-fab:hover { filter: brightness(1.07); }
    .sf-fab svg { width: 16px; height: 16px; }
    .sf-panel { position: absolute; ${panelVert} ${panelSide} width: 320px; max-width: 84vw;
      background: #fff; color: #111; border-radius: 14px; box-shadow: 0 12px 40px rgba(0,0,0,.28);
      padding: 16px; display: none; }
    .sf-panel.sf-open { display: block; }
    .sf-h { font-size: 15px; font-weight: 700; margin: 0 0 10px; }
    .sf-type { width: 100%; padding: 8px; border: 1px solid #d6d6e0; border-radius: 9px;
      font-size: 13px; margin-bottom: 8px; background: #fafafe; }
    .sf-text { width: 100%; box-sizing: border-box; min-height: 92px; resize: vertical;
      padding: 10px; border: 1px solid #d6d6e0; border-radius: 9px; font-size: 14px;
      font-family: inherit; }
    .sf-row { display: flex; align-items: center; justify-content: space-between; margin: 10px 0 4px; }
    .sf-check { display: flex; align-items: center; gap: 7px; font-size: 13px; color: #444; }
    .sf-actions { display: flex; gap: 8px; margin-top: 12px; }
    .sf-send { flex: 1; background: ${cfg.color || "#5b21b6"}; color: #fff; border: none;
      border-radius: 9px; padding: 10px; font-size: 14px; font-weight: 600; cursor: pointer; }
    .sf-send:disabled { opacity: .6; cursor: default; }
    .sf-cancel { background: #f0f0f4; color: #333; border: none; border-radius: 9px;
      padding: 10px 14px; font-size: 14px; cursor: pointer; }
    .sf-status { font-size: 13px; margin-top: 10px; min-height: 16px; }
    .sf-status.sf-ok { color: #0b8a3b; } .sf-status.sf-err { color: #c01c28; }
    .sf-link { display: none; margin-left: 8px; font-size: 13px; color: #5b21b6; }
  </style>
  <div class="sf-wrap">
    <div class="sf-panel">
      <p class="sf-h">Send feedback</p>
      <select class="sf-type">
        <option value="bug"${cfg.type === "bug" ? " selected" : ""}>🐞 Bug</option>
        <option value="feature"${cfg.type === "feature" ? " selected" : ""}>✨ Feature request</option>
        <option value="other"${cfg.type === "other" ? " selected" : ""}>💬 Other</option>
      </select>
      <textarea class="sf-text" placeholder="What went wrong, or what would you like?"></textarea>
      <div class="sf-row">
        <label class="sf-check"><input type="checkbox" class="sf-shot"${cfg.attachScreenshot ? " checked" : ""}/> Attach screenshot</label>
      </div>
      <div class="sf-actions">
        <button class="sf-cancel" type="button">Cancel</button>
        <button class="sf-send" type="button">Send</button>
      </div>
      <div class="sf-status"></div><a class="sf-link" target="_blank" rel="noopener"></a>
    </div>
    <button class="sf-fab" type="button" aria-label="${cfg.label}">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
      ${cfg.label}
    </button>
  </div>`;
}

export { SuperFeedback };
if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
