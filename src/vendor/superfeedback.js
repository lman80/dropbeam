/* eslint-disable -- vendored third-party widget (lman80/SuperFeedback); do not lint */
// SuperFeedback web widget (web / Electron renderer / Tauri webview). v1.1.0
//
// Trigger styles — pick the least-intrusive one for the app:
//   floating (default)  a corner button.  Options: position, compact (icon-only), color, label.
//   mounted             init({ mount: '#some-toolbar' }) places the button INSIDE that element.
//   none                init({ trigger: 'none' }) renders NO button — you open it yourself:
//                         call SuperFeedback.open() from a menu item, sidebar link, right-click,
//                         or keyboard shortcut.
// The feedback panel is a centered popup, so it works wherever the trigger lives.
//
//   import { SuperFeedback } from "./superfeedback.js";
//   SuperFeedback.init({ backendUrl, repo, app, appKey, /* + any of the above */ });
//
// For Electron/Tauri, pass `captureScreenshot` for a true native window grab
// (see ../../docs/screenshots.md); the default below is a DOM snapshot.

const CAPTURE_CDN = "https://esm.sh/html-to-image@1.11.13";
const ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>`;

const SuperFeedback = {
  version: "1.1.0",
  _cfg: null,
  _panelHost: null,
  _triggerHost: null,

  init(config = {}) {
    if (!config.backendUrl || !config.repo) {
      console.error("[SuperFeedback] init requires { backendUrl, repo }");
      return;
    }
    this.destroy(); // idempotent re-init
    this._cfg = {
      position: "bottom-right",
      label: "Feedback",
      color: "#5b21b6",
      type: "bug",
      attachScreenshot: true,
      trigger: "floating", // "floating" | "mounted" | "none"
      mount: null,         // CSS selector or element; implies trigger "mounted"
      compact: false,      // icon-only button
      ...config,
    };
    if (this._cfg.mount) this._cfg.trigger = "mounted";
    const start = () => this._mount();
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
    else start();
    if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
  },

  // Open/close the feedback panel from anywhere (use with trigger:'none' or custom UI).
  open() { this._panelHost && this._panelHost.__open(); },
  close() { this._panelHost && this._panelHost.__close(); },
  toggle() { this._panelHost && this._panelHost.__toggle(); },

  destroy() {
    for (const h of [this._panelHost, this._triggerHost]) {
      if (h && h.parentNode) h.parentNode.removeChild(h);
    }
    this._panelHost = this._triggerHost = null;
  },

  _mount() {
    this._mountPanel();
    const t = this._cfg.trigger;
    if (t === "none") return;
    if (t === "mounted") this._mountTriggerInto(this._cfg.mount);
    else this._mountFloating();
  },

  _mountPanel() {
    const host = document.createElement("div");
    host.setAttribute("data-superfeedback-panel", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = PANEL_TEMPLATE(this._cfg);
    document.body.appendChild(host);
    this._panelHost = host;

    const $ = (s) => root.querySelector(s);
    const modal = $(".sf-modal"), backdrop = $(".sf-backdrop"), status = $(".sf-status");
    host.__open = () => { backdrop.classList.add("sf-show"); modal.classList.add("sf-show"); $(".sf-text").focus(); };
    host.__close = () => {
      backdrop.classList.remove("sf-show"); modal.classList.remove("sf-show");
      status.textContent = ""; status.className = "sf-status";
    };
    host.__toggle = () => (modal.classList.contains("sf-show") ? host.__close() : host.__open());

    backdrop.addEventListener("click", host.__close);
    $(".sf-cancel").addEventListener("click", host.__close);
    $(".sf-send").addEventListener("click", () => this._submit(root));
  },

  _mountFloating() {
    const host = document.createElement("div");
    host.setAttribute("data-superfeedback-trigger", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = FLOATING_TEMPLATE(this._cfg);
    document.body.appendChild(host);
    this._triggerHost = host;
    root.querySelector(".sf-fab").addEventListener("click", () => this.toggle());
  },

  _mountTriggerInto(target) {
    const el = typeof target === "string" ? document.querySelector(target) : target;
    if (!el) {
      console.warn("[SuperFeedback] mount target not found:", target, "— falling back to floating button");
      this._mountFloating();
      return;
    }
    const host = document.createElement("span");
    host.setAttribute("data-superfeedback-trigger", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = INLINE_TEMPLATE(this._cfg);
    el.appendChild(host);
    this._triggerHost = host;
    root.querySelector(".sf-inline").addEventListener("click", () => this.open());
  },

  async _submit(root) {
    const $ = (s) => root.querySelector(s);
    const status = $(".sf-status");
    const message = $(".sf-text").value.trim();
    if (!message) { setStatus(status, "Please describe the issue first.", "err"); return; }
    const type = $(".sf-type").value;
    const wantShot = $(".sf-shot").checked;
    const sendBtn = $(".sf-send");
    sendBtn.disabled = true;
    setStatus(status, wantShot ? "Capturing screenshot…" : "Sending…", "");

    let screenshot = null;
    if (wantShot) {
      this._setHostsVisible(false);           // keep our UI out of the shot
      try { screenshot = await this._capture(); } catch (_) { /* send without */ }
      this._setHostsVisible(true);
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
        setTimeout(() => this.close(), 2600);
      } else {
        setStatus(status, res.error || "Something went wrong.", "err");
      }
    } catch (e) {
      setStatus(status, "Couldn't reach the feedback server.", "err");
    } finally {
      sendBtn.disabled = false;
    }
  },

  _setHostsVisible(v) {
    for (const h of [this._panelHost, this._triggerHost]) {
      if (h) h.style.visibility = v ? "visible" : "hidden";
    }
  },

  async _capture() {
    const cfg = this._cfg;
    if (typeof cfg.captureScreenshot === "function") return await cfg.captureScreenshot();
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

const FIELD_STYLES = `
    .sf-h { font-size: 15px; font-weight: 700; margin: 0 0 10px; }
    .sf-type { width: 100%; padding: 8px; border: 1px solid #d6d6e0; border-radius: 9px;
      font-size: 13px; margin-bottom: 8px; background: #fafafe; }
    .sf-text { width: 100%; box-sizing: border-box; min-height: 92px; resize: vertical;
      padding: 10px; border: 1px solid #d6d6e0; border-radius: 9px; font-size: 14px; font-family: inherit; }
    .sf-row { display: flex; align-items: center; justify-content: space-between; margin: 10px 0 4px; }
    .sf-check { display: flex; align-items: center; gap: 7px; font-size: 13px; color: #444; }
    .sf-actions { display: flex; gap: 8px; margin-top: 12px; }
    .sf-send { flex: 1; color: #fff; border: none; border-radius: 9px; padding: 10px;
      font-size: 14px; font-weight: 600; cursor: pointer; }
    .sf-send:disabled { opacity: .6; cursor: default; }
    .sf-cancel { background: #f0f0f4; color: #333; border: none; border-radius: 9px;
      padding: 10px 14px; font-size: 14px; cursor: pointer; }
    .sf-status { font-size: 13px; margin-top: 10px; min-height: 16px; }
    .sf-status.sf-ok { color: #0b8a3b; } .sf-status.sf-err { color: #c01c28; }
    .sf-link { display: none; margin-left: 8px; font-size: 13px; }`;

function PANEL_TEMPLATE(cfg) {
  const accent = cfg.color || "#5b21b6";
  return `
  <style>
    :host { all: initial; }
    .sf-backdrop { position: fixed; inset: 0; background: rgba(20,16,40,.28); z-index: 2147483000; display: none; }
    .sf-backdrop.sf-show { display: block; }
    .sf-modal { position: fixed; left: 50%; top: 50%; transform: translate(-50%,-50%);
      z-index: 2147483001; width: 340px; max-width: 90vw; background: #fff; color: #111;
      border-radius: 14px; box-shadow: 0 16px 50px rgba(0,0,0,.32); padding: 18px; display: none;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
    .sf-modal.sf-show { display: block; }
    .sf-send { background: ${accent}; } .sf-link { color: ${accent}; }
    ${FIELD_STYLES}
  </style>
  <div class="sf-backdrop"></div>
  <div class="sf-modal" role="dialog" aria-modal="true" aria-label="Send feedback">
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
  </div>`;
}

function FLOATING_TEMPLATE(cfg) {
  const pos = {
    "bottom-right": "bottom:20px;right:20px;",
    "bottom-left": "bottom:20px;left:20px;",
    "top-right": "top:20px;right:20px;",
    "top-left": "top:20px;left:20px;",
  }[cfg.position] || "bottom:20px;right:20px;";
  const compact = cfg.compact || !cfg.label;
  const label = cfg.label || "Feedback";
  return `
  <style>
    :host { all: initial; }
    .sf-fab { position: fixed; ${pos} z-index: 2147482999; display: inline-flex; align-items: center;
      gap: 8px; cursor: pointer; border: none; border-radius: 999px; color: #fff;
      background: ${cfg.color || "#5b21b6"}; box-shadow: 0 6px 20px rgba(0,0,0,.22);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      font-size: 14px; font-weight: 600;
      ${compact ? "padding: 0; width: 48px; height: 48px; justify-content: center;" : "padding: 11px 16px;"} }
    .sf-fab:hover { filter: brightness(1.07); }
    .sf-fab svg { width: 18px; height: 18px; }
  </style>
  <button class="sf-fab" type="button" aria-label="${label}">${ICON}${compact ? "" : label}</button>`;
}

function INLINE_TEMPLATE(cfg) {
  const label = cfg.label || "Feedback";
  return `
  <style>
    :host { all: initial; display: inline-block; }
    .sf-inline { display: inline-flex; align-items: center; gap: 7px; cursor: pointer; border: none;
      background: transparent; padding: 8px 10px; border-radius: 8px; color: ${cfg.color || "#5b21b6"};
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; font-size: 14px; }
    .sf-inline:hover { background: rgba(0,0,0,.06); }
    .sf-inline svg { width: 16px; height: 16px; }
  </style>
  <button class="sf-inline" type="button" aria-label="${label}">${ICON}${cfg.compact ? "" : `<span>${label}</span>`}</button>`;
}

export { SuperFeedback };
if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
