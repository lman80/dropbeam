// SuperFeedback web widget (web / Electron renderer / Tauri webview). v1.3.0
//
// Liquid-glass UI: frosted panel, auto light/dark (theme), segmented control, iOS switch,
// bottom-sheet on phones. Sending is fire-and-forget. Users can attach their own image(s)
// in addition to the automatic screenshot.
//
// Key options:
//   theme:   "auto" (default) | "light" | "dark"      color: accent (default indigo)
//   trigger: "floating" (default) | "mounted" (with `mount`) | "none" (call open())
//   compact: icon-only floating button                 maxImages: max user attachments (default 5)
//   nudge:   true | { message, delayMs, cooldownDays } — gently invite feedback (default off)
//
//   import { SuperFeedback } from "./superfeedback.js";
//   SuperFeedback.init({ backendUrl, repo, app, theme: "auto", nudge: true });

const CAPTURE_CDN = "https://esm.sh/html-to-image@1.11.13";
const ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>`;
const IMG_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="M21 15l-5-5L5 21"/></svg>`;
const TYPES = [
  { v: "bug", label: "Bug", emoji: "🐞" },
  { v: "feature", label: "Idea", emoji: "✨" },
  { v: "other", label: "Other", emoji: "💬" },
];

const SuperFeedback = {
  version: "1.3.0",
  _cfg: null,
  _panelHost: null,
  _triggerHost: null,

  init(config = {}) {
    if (!config.backendUrl || !config.repo) {
      console.error("[SuperFeedback] init requires { backendUrl, repo }");
      return;
    }
    this.destroy();
    this._cfg = {
      position: "bottom-right", label: "Feedback", color: "#6d5efc", theme: "auto",
      type: "bug", attachScreenshot: true, trigger: "floating", mount: null,
      compact: false, nudge: false, maxImages: 5,
      ...config,
    };
    if (this._cfg.mount) this._cfg.trigger = "mounted";
    const start = () => this._mount();
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
    else start();
    if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
  },

  open() { this._panelHost && this._panelHost.__open(); },
  close() { this._panelHost && this._panelHost.__close(); },
  toggle() { this._panelHost && this._panelHost.__toggle(); },

  destroy() {
    for (const h of [this._panelHost, this._triggerHost]) if (h && h.parentNode) h.parentNode.removeChild(h);
    this._panelHost = this._triggerHost = null;
  },

  _mount() {
    this._mountPanel();
    const t = this._cfg.trigger;
    if (t === "mounted") this._mountTriggerInto(this._cfg.mount);
    else if (t !== "none") this._mountFloating();
    this._maybeNudge();
  },

  _mountPanel() {
    const host = document.createElement("div");
    host.setAttribute("data-superfeedback-panel", "");
    host.className = themeClass(this._cfg.theme);
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = PANEL_TEMPLATE(this._cfg);
    document.body.appendChild(host);
    this._panelHost = host;

    const $ = (s) => root.querySelector(s);
    const modal = $(".sf-modal"), backdrop = $(".sf-backdrop"), toast = $(".sf-toast");

    // Image attachments
    host.__images = [];
    const max = this._cfg.maxImages || 5;
    const fileInput = $(".sf-file"), thumbs = $(".sf-thumbs"), addBtn = $(".sf-addimg");
    const renderThumbs = () => {
      thumbs.innerHTML = host.__images.map((src, i) =>
        `<span class="sf-thumb" style="background-image:url('${src}')"><button class="sf-thumb-x" type="button" data-i="${i}" aria-label="Remove image">✕</button></span>`).join("");
      thumbs.querySelectorAll(".sf-thumb-x").forEach((b) =>
        b.addEventListener("click", () => { host.__images.splice(+b.dataset.i, 1); renderThumbs(); syncAdd(); }));
    };
    const syncAdd = () => { addBtn.style.display = host.__images.length >= max ? "none" : ""; };
    host.__clearImages = () => { host.__images = []; renderThumbs(); syncAdd(); };
    addBtn.addEventListener("click", () => fileInput.click());
    fileInput.addEventListener("change", async () => {
      const files = Array.from(fileInput.files || []); fileInput.value = "";
      for (const f of files.slice(0, max - host.__images.length)) {
        try { host.__images.push(await fileToDataURL(f)); } catch (_) {}
      }
      renderThumbs(); syncAdd();
    });

    host.__open = () => { backdrop.classList.add("sf-show"); modal.classList.add("sf-show"); setTimeout(() => $(".sf-text").focus(), 60); };
    host.__close = () => { backdrop.classList.remove("sf-show"); modal.classList.remove("sf-show"); host.__clearImages(); };
    host.__toggle = () => (modal.classList.contains("sf-show") ? host.__close() : host.__open());

    let toastTimer = null;
    host.__toast = (msg, kind = "", onClick = null) => {
      clearTimeout(toastTimer);
      toast.textContent = msg;
      toast.className = "sf-toast sf-show" + (kind ? " sf-" + kind : "");
      toast.onclick = onClick;
      toastTimer = setTimeout(() => { toast.className = "sf-toast"; toast.onclick = null; }, kind === "err" ? 7000 : 2600);
    };

    root.querySelectorAll(".sf-seg-btn").forEach((b) =>
      b.addEventListener("click", () => {
        root.querySelectorAll(".sf-seg-btn").forEach((x) => x.classList.remove("sf-active"));
        b.classList.add("sf-active");
      }));

    backdrop.addEventListener("click", host.__close);
    $(".sf-cancel").addEventListener("click", host.__close);
    $(".sf-send").addEventListener("click", () => this._submit(root));
    root.addEventListener("keydown", (e) => { if (e.key === "Escape") host.__close(); });
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
    if (!el) { console.warn("[SuperFeedback] mount target not found:", target, "— falling back to floating button"); this._mountFloating(); return; }
    const host = document.createElement("span");
    host.setAttribute("data-superfeedback-trigger", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = INLINE_TEMPLATE(this._cfg);
    el.appendChild(host);
    this._triggerHost = host;
    root.querySelector(".sf-inline").addEventListener("click", () => this.open());
  },

  _maybeNudge() {
    const n = this._cfg.nudge;
    if (!n || !this._panelHost) return;
    const o = typeof n === "object" ? n : {};
    const delay = o.delayMs ?? 45000, cooldown = (o.cooldownDays ?? 7) * 864e5;
    const msg = o.message || "Got feedback? We'd love to hear it 💜";
    const key = "superfeedback:nudge:" + this._cfg.repo;
    try { if (Date.now() - parseInt(localStorage.getItem(key) || "0", 10) < cooldown) return; } catch (_) {}
    setTimeout(() => {
      const host = this._panelHost; if (!host) return;
      const root = host.shadowRoot, nudge = root.querySelector(".sf-nudge");
      root.querySelector(".sf-nudge-msg").textContent = msg;
      nudge.classList.add("sf-show");
      const remember = () => { try { localStorage.setItem(key, String(Date.now())); } catch (_) {} };
      const hide = () => nudge.classList.remove("sf-show");
      root.querySelector(".sf-nudge-open").onclick = () => { hide(); remember(); this.open(); };
      root.querySelector(".sf-nudge-x").onclick = () => { hide(); remember(); };
      setTimeout(() => { if (nudge.classList.contains("sf-show")) hide(); }, 14000);
    }, delay);
  },

  _submit(root) {
    const $ = (s) => root.querySelector(s);
    const message = $(".sf-text").value.trim();
    if (!message) {
      const t = $(".sf-text"); t.classList.add("sf-shake"); t.focus();
      setTimeout(() => t.classList.remove("sf-shake"), 500);
      return;
    }
    const active = root.querySelector(".sf-seg-btn.sf-active");
    const payload = {
      message, type: active ? active.dataset.type : this._cfg.type,
      wantShot: $(".sf-shot").checked, images: (this._panelHost.__images || []).slice(), screenshot: undefined,
    };
    $(".sf-text").value = "";
    this.close();
    this._sendInBackground(payload);
  },

  async _sendInBackground(payload) {
    const host = this._panelHost;
    if (payload.wantShot && payload.screenshot === undefined) {
      this._setHostsVisible(false);
      try { payload.screenshot = await this._capture(); } catch (_) { payload.screenshot = null; }
      this._setHostsVisible(true);
    }
    try {
      const res = await this._send({ message: payload.message, type: payload.type, screenshot: payload.screenshot || null, images: payload.images });
      if (res && res.ok) host && host.__toast("Thanks! Feedback sent ✓", "ok");
      else host && host.__toast("Couldn't send — tap to retry", "err", () => this._sendInBackground(payload));
    } catch (_) {
      host && host.__toast("Couldn't send — tap to retry", "err", () => this._sendInBackground(payload));
    }
  },

  _setHostsVisible(v) {
    for (const h of [this._panelHost, this._triggerHost]) if (h) h.style.visibility = v ? "visible" : "hidden";
  },

  async _capture() {
    const cfg = this._cfg;
    if (typeof cfg.captureScreenshot === "function") return await cfg.captureScreenshot();
    const mod = await import(/* @vite-ignore */ CAPTURE_CDN);
    return await mod.toPng(document.documentElement, { cacheBust: true, pixelRatio: Math.min(window.devicePixelRatio || 1, 2) });
  },

  async _send({ message, type, screenshot, images }) {
    const cfg = this._cfg;
    const meta = {
      url: location.href, platform: navigator.platform, userAgent: navigator.userAgent,
      locale: navigator.language, viewport: `${window.innerWidth}x${window.innerHeight}`,
      ...(cfg.appVersion ? { appVersion: cfg.appVersion } : {}), ...(cfg.meta || {}),
    };
    const res = await fetch(cfg.backendUrl.replace(/\/$/, "") + "/report", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        repo: cfg.repo, app: cfg.app || "", type, message,
        screenshot: screenshot || undefined,
        images: images && images.length ? images : undefined,
        appKey: cfg.appKey || undefined, meta,
      }),
    });
    try { return await res.json(); } catch { return { ok: false, error: `HTTP ${res.status}` }; }
  },
};

function themeClass(t) { return t === "dark" ? "sf-dark" : t === "light" ? "sf-light" : "sf-auto"; }

// Read a File, downscale, and return a data URL (keeps uploads small).
function fileToDataURL(file, maxDim = 1600, quality = 0.85) {
  return new Promise((resolve, reject) => {
    if (!file || !/^image\//.test(file.type)) return reject(new Error("not an image"));
    const img = new Image(); const url = URL.createObjectURL(file);
    img.onload = () => {
      URL.revokeObjectURL(url);
      let w = img.width, h = img.height;
      const s = Math.min(1, maxDim / Math.max(w, h));
      w = Math.round(w * s); h = Math.round(h * s);
      const c = document.createElement("canvas"); c.width = w; c.height = h;
      c.getContext("2d").drawImage(img, 0, 0, w, h);
      const type = file.type === "image/png" ? "image/png" : "image/jpeg";
      try { resolve(c.toDataURL(type, quality)); } catch (e) { reject(e); }
    };
    img.onerror = () => { URL.revokeObjectURL(url); reject(new Error("image load failed")); };
    img.src = url;
  });
}

const TOKENS = `
  :host {
    --sf-bg: rgba(255,255,255,.72); --sf-fg: #15131f; --sf-muted: #6c6c7a;
    --sf-field: rgba(255,255,255,.5); --sf-field-bd: rgba(120,120,140,.26);
    --sf-border: rgba(255,255,255,.6); --sf-shadow: 0 24px 70px rgba(20,16,40,.30);
    --sf-seg: rgba(120,120,140,.12); --sf-hover: rgba(120,120,140,.12);
  }
  :host(.sf-dark) {
    --sf-bg: rgba(32,32,38,.72); --sf-fg: #f3f3f8; --sf-muted: #a2a2b2;
    --sf-field: rgba(255,255,255,.07); --sf-field-bd: rgba(255,255,255,.14);
    --sf-border: rgba(255,255,255,.12); --sf-shadow: 0 24px 70px rgba(0,0,0,.55);
    --sf-seg: rgba(255,255,255,.08); --sf-hover: rgba(255,255,255,.10);
  }
  @media (prefers-color-scheme: dark) {
    :host(.sf-auto) {
      --sf-bg: rgba(32,32,38,.72); --sf-fg: #f3f3f8; --sf-muted: #a2a2b2;
      --sf-field: rgba(255,255,255,.07); --sf-field-bd: rgba(255,255,255,.14);
      --sf-border: rgba(255,255,255,.12); --sf-shadow: 0 24px 70px rgba(0,0,0,.55);
      --sf-seg: rgba(255,255,255,.08); --sf-hover: rgba(255,255,255,.10);
    }
  }`;

const FONT = `-apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", Roboto, system-ui, sans-serif`;

function PANEL_TEMPLATE(cfg) {
  const accent = cfg.color || "#6d5efc";
  const seg = TYPES.map((t) =>
    `<button class="sf-seg-btn${t.v === cfg.type ? " sf-active" : ""}" type="button" data-type="${t.v}"><span>${t.emoji}</span>${t.label}</button>`).join("");
  return `
  <style>
    ${TOKENS}
    *, *::before, *::after { box-sizing: border-box; }
    .sf-backdrop { position: fixed; inset: 0; z-index: 2147483000; background: rgba(15,12,30,.32);
      backdrop-filter: blur(2px); -webkit-backdrop-filter: blur(2px); opacity: 0; pointer-events: none; transition: opacity .22s ease; }
    .sf-backdrop.sf-show { opacity: 1; pointer-events: auto; }
    .sf-modal { position: fixed; left: 50%; top: 50%; z-index: 2147483001; width: 360px; max-width: calc(100vw - 32px);
      color: var(--sf-fg); font-family: ${FONT}; background: var(--sf-bg); border: 1px solid var(--sf-border);
      border-radius: 22px; box-shadow: var(--sf-shadow); padding: 20px;
      backdrop-filter: blur(28px) saturate(180%); -webkit-backdrop-filter: blur(28px) saturate(180%);
      opacity: 0; pointer-events: none; transform: translate(-50%,-50%) scale(.94);
      transition: opacity .24s ease, transform .26s cubic-bezier(.2,.9,.25,1); }
    .sf-modal.sf-show { opacity: 1; pointer-events: auto; transform: translate(-50%,-50%) scale(1); }
    .sf-grabber { display: none; width: 38px; height: 5px; border-radius: 3px; background: var(--sf-field-bd); margin: -6px auto 12px; }
    .sf-head { display: flex; align-items: baseline; justify-content: space-between; margin: 0 0 14px; }
    .sf-h { font-size: 17px; font-weight: 700; letter-spacing: -.01em; margin: 0; }
    .sf-sub { font-size: 12px; color: var(--sf-muted); }
    .sf-seg { display: flex; gap: 4px; padding: 4px; border-radius: 13px; background: var(--sf-seg); margin-bottom: 12px; }
    .sf-seg-btn { flex: 1; display: inline-flex; align-items: center; justify-content: center; gap: 5px; border: none;
      background: transparent; color: var(--sf-muted); cursor: pointer; font-family: inherit; font-size: 13px; font-weight: 600;
      padding: 8px 6px; border-radius: 10px; transition: background .18s, color .18s, box-shadow .18s; }
    .sf-seg-btn:hover { color: var(--sf-fg); }
    .sf-seg-btn.sf-active { background: ${accent}; color: #fff; box-shadow: 0 4px 14px ${hexA(accent, .4)}; }
    .sf-text { width: 100%; min-height: 96px; resize: vertical; color: var(--sf-fg); background: var(--sf-field);
      border: 1px solid var(--sf-field-bd); border-radius: 14px; padding: 12px 14px; font-family: inherit; font-size: 15px;
      line-height: 1.4; outline: none; transition: border-color .18s, box-shadow .18s; }
    .sf-text::placeholder { color: var(--sf-muted); }
    .sf-text:focus { border-color: ${accent}; box-shadow: 0 0 0 4px ${hexA(accent, .18)}; }
    .sf-shake { animation: sf-shake .4s; }
    @keyframes sf-shake { 0%,100%{transform:translateX(0)} 20%,60%{transform:translateX(-6px)} 40%,80%{transform:translateX(6px)} }
    .sf-attach { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin-top: 12px; }
    .sf-addimg { display: inline-flex; align-items: center; gap: 7px; border: 1px dashed var(--sf-field-bd); background: var(--sf-field);
      color: var(--sf-muted); border-radius: 11px; padding: 8px 12px; font-family: inherit; font-size: 13px; font-weight: 600; cursor: pointer; transition: color .15s; }
    .sf-addimg:hover { color: var(--sf-fg); }
    .sf-addimg svg { width: 15px; height: 15px; }
    .sf-thumbs { display: flex; gap: 8px; flex-wrap: wrap; }
    .sf-thumb { position: relative; width: 46px; height: 46px; border-radius: 10px; background-size: cover; background-position: center; border: 1px solid var(--sf-border); }
    .sf-thumb-x { position: absolute; top: -6px; right: -6px; width: 20px; height: 20px; border-radius: 50%; border: none;
      background: #c42834; color: #fff; font-size: 11px; line-height: 1; cursor: pointer; }
    .sf-row { display: flex; align-items: center; gap: 9px; margin: 14px 2px 4px; font-size: 14px; color: var(--sf-fg); }
    .sf-switch { position: relative; width: 42px; height: 25px; flex: 0 0 auto; }
    .sf-switch input { position: absolute; opacity: 0; width: 100%; height: 100%; margin: 0; cursor: pointer; }
    .sf-slider { position: absolute; inset: 0; border-radius: 999px; background: var(--sf-field-bd); transition: background .2s; }
    .sf-slider::before { content: ""; position: absolute; width: 21px; height: 21px; left: 2px; top: 2px; border-radius: 50%;
      background: #fff; box-shadow: 0 1px 3px rgba(0,0,0,.3); transition: transform .2s; }
    .sf-switch input:checked + .sf-slider { background: ${accent}; }
    .sf-switch input:checked + .sf-slider::before { transform: translateX(17px); }
    .sf-actions { display: flex; gap: 10px; margin-top: 18px; }
    .sf-cancel { background: var(--sf-hover); color: var(--sf-fg); border: none; border-radius: 13px; padding: 12px 16px;
      font-family: inherit; font-size: 15px; font-weight: 600; cursor: pointer; transition: filter .15s; }
    .sf-send { flex: 1; color: #fff; border: none; border-radius: 13px; padding: 12px; cursor: pointer; font-family: inherit;
      font-size: 15px; font-weight: 700; background: ${accent}; box-shadow: 0 8px 22px ${hexA(accent, .45)}; transition: transform .1s, filter .15s; }
    .sf-send:hover, .sf-cancel:hover { filter: brightness(1.06); }
    .sf-send:active { transform: scale(.97); }
    .sf-toast { position: fixed; left: 50%; bottom: 26px; z-index: 2147483002; transform: translateX(-50%) translateY(10px);
      color: #fff; font-family: ${FONT}; font-size: 14px; font-weight: 600; padding: 12px 18px; border-radius: 14px;
      background: rgba(28,24,46,.82); backdrop-filter: blur(18px) saturate(180%); -webkit-backdrop-filter: blur(18px) saturate(180%);
      border: 1px solid rgba(255,255,255,.14); box-shadow: 0 12px 34px rgba(0,0,0,.34); max-width: 84vw;
      opacity: 0; pointer-events: none; transition: opacity .2s ease, transform .2s ease; }
    .sf-toast.sf-show { opacity: 1; pointer-events: auto; transform: translateX(-50%) translateY(0); }
    .sf-toast.sf-ok { background: rgba(16,138,72,.88); }
    .sf-toast.sf-err { background: rgba(196,40,52,.9); cursor: pointer; }
    .sf-nudge { position: fixed; right: 20px; bottom: 88px; z-index: 2147482998; display: flex; align-items: center; gap: 10px;
      color: var(--sf-fg); font-family: ${FONT}; font-size: 13.5px; font-weight: 500; padding: 11px 12px 11px 16px; border-radius: 16px;
      background: var(--sf-bg); border: 1px solid var(--sf-border); box-shadow: var(--sf-shadow); max-width: 300px;
      backdrop-filter: blur(22px) saturate(180%); -webkit-backdrop-filter: blur(22px) saturate(180%);
      opacity: 0; pointer-events: none; transform: translateY(12px) scale(.96); transition: opacity .25s ease, transform .25s cubic-bezier(.2,.9,.25,1); }
    .sf-nudge.sf-show { opacity: 1; pointer-events: auto; transform: translateY(0) scale(1); }
    .sf-nudge-open { border: none; background: ${accent}; color: #fff; border-radius: 10px; padding: 7px 12px; font-family: inherit;
      font-size: 13px; font-weight: 700; cursor: pointer; white-space: nowrap; }
    .sf-nudge-x { border: none; background: transparent; color: var(--sf-muted); cursor: pointer; font-size: 15px; padding: 2px 4px; line-height: 1; }
    @media (max-width: 520px) {
      .sf-modal { left: 0; right: 0; top: auto; bottom: 0; width: 100%; max-width: 100%; border-radius: 24px 24px 0 0;
        padding: 14px 18px calc(20px + env(safe-area-inset-bottom)); transform: translateY(110%); }
      .sf-modal.sf-show { transform: translateY(0); }
      .sf-grabber { display: block; }
      .sf-nudge { left: 16px; right: 16px; bottom: calc(88px + env(safe-area-inset-bottom)); max-width: none; }
      .sf-text { font-size: 16px; }
    }
  </style>
  <div class="sf-backdrop"></div>
  <div class="sf-modal" role="dialog" aria-modal="true" aria-label="Send feedback">
    <div class="sf-grabber"></div>
    <div class="sf-head"><h2 class="sf-h">Send feedback</h2><span class="sf-sub">${cfg.app ? esc(cfg.app) : ""}</span></div>
    <div class="sf-seg" role="tablist">${seg}</div>
    <textarea class="sf-text" placeholder="What went wrong, or what would you like?"></textarea>
    <div class="sf-attach">
      <button class="sf-addimg" type="button">${IMG_ICON} Add image</button>
      <div class="sf-thumbs"></div>
    </div>
    <input type="file" class="sf-file" accept="image/*" multiple hidden />
    <label class="sf-row">
      <span class="sf-switch"><input type="checkbox" class="sf-shot"${cfg.attachScreenshot ? " checked" : ""}/><span class="sf-slider"></span></span>
      Attach screenshot
    </label>
    <div class="sf-actions">
      <button class="sf-cancel" type="button">Cancel</button>
      <button class="sf-send" type="button">Send feedback</button>
    </div>
  </div>
  <div class="sf-nudge">
    <span class="sf-nudge-msg"></span>
    <button class="sf-nudge-open" type="button">Sure</button>
    <button class="sf-nudge-x" type="button" aria-label="Dismiss">✕</button>
  </div>
  <div class="sf-toast" role="status"></div>`;
}

function FLOATING_TEMPLATE(cfg) {
  const pos = {
    "bottom-right": "bottom:22px;right:22px;", "bottom-left": "bottom:22px;left:22px;",
    "top-right": "top:22px;right:22px;", "top-left": "top:22px;left:22px;",
  }[cfg.position] || "bottom:22px;right:22px;";
  const compact = cfg.compact || !cfg.label;
  const label = cfg.label || "Feedback";
  const accent = cfg.color || "#6d5efc";
  return `
  <style>
    :host { all: initial; }
    .sf-fab { position: fixed; ${pos} z-index: 2147482999; display: inline-flex; align-items: center; gap: 9px; cursor: pointer;
      border: none; border-radius: 999px; color: #fff; background: linear-gradient(135deg, ${accent}, ${shade(accent, 18)});
      box-shadow: 0 10px 28px ${hexA(accent, .5)}, inset 0 1px 0 rgba(255,255,255,.25); font-family: ${FONT}; font-size: 14.5px; font-weight: 600;
      ${compact ? "padding:0;width:52px;height:52px;justify-content:center;" : "padding:13px 18px;"} transition: transform .12s ease, box-shadow .2s ease; }
    .sf-fab:hover { transform: translateY(-1px); box-shadow: 0 14px 34px ${hexA(accent, .6)}, inset 0 1px 0 rgba(255,255,255,.25); }
    .sf-fab:active { transform: scale(.96); }
    .sf-fab svg { width: 19px; height: 19px; }
    @media (max-width: 520px) { .sf-fab { ${pos.includes("bottom") ? "bottom:calc(20px + env(safe-area-inset-bottom));" : ""} } }
  </style>
  <button class="sf-fab" type="button" aria-label="${label}">${ICON}${compact ? "" : label}</button>`;
}

function INLINE_TEMPLATE(cfg) {
  const label = cfg.label || "Feedback";
  const accent = cfg.color || "#6d5efc";
  return `
  <style>
    :host { all: initial; display: inline-block; }
    .sf-inline { display: inline-flex; align-items: center; gap: 7px; cursor: pointer; border: none; background: transparent;
      padding: 8px 10px; border-radius: 10px; color: ${accent}; font-family: ${FONT}; font-size: 14px; font-weight: 600; transition: background .15s; }
    .sf-inline:hover { background: rgba(120,120,140,.14); }
    .sf-inline svg { width: 16px; height: 16px; }
  </style>
  <button class="sf-inline" type="button" aria-label="${label}">${ICON}${cfg.compact ? "" : `<span>${label}</span>`}</button>`;
}

function esc(s) { return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }
function hexA(hex, a) { const { r, g, b } = parseHex(hex); return `rgba(${r},${g},${b},${a})`; }
function shade(hex, pct) { const { r, g, b } = parseHex(hex); const f = (n) => Math.max(0, Math.min(255, Math.round(n * (1 - pct / 100)))); return `rgb(${f(r)},${f(g)},${f(b)})`; }
function parseHex(hex) {
  let h = String(hex).replace("#", "");
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  const n = parseInt(h || "6d5efc", 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 };
}

export { SuperFeedback };
if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
