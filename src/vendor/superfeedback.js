// SuperFeedback web widget (web / Electron renderer / Tauri webview). v3.3.1
//
// Liquid-glass UI: frosted panel, auto light/dark (theme), segmented control, iOS switch,
// bottom-sheet on phones. Sending is fire-and-forget. Users can attach their own image(s)
// in addition to the automatic screenshot, and mark the screenshot up (pen / circle /
// arrow / rectangle) before sending.
//
// Key options:
//   theme:   "auto" (default) | "light" | "dark"      color: accent (default indigo)
//   trigger: "draggable" (default) | "floating" | "mounted" (with `mount`) | "none" (call open())
//   compact: icon-only floating button                 maxImages: max user attachments (default 5)
//   nudge:   true | { message, delayMs, cooldownDays } — gently invite feedback (default off)
//   markup:  true (default) — let users draw on the screenshot before sending
//   widgetFeedback: "lman80/SuperFeedback" (default) — repo for feedback about the widget itself; false hides the link
//   checkin: true (default) — daily anonymous version/config check-in (PROTOCOL.md "POST /checkin"); false disables
//
//   import { SuperFeedback } from "./superfeedback.js";
//   SuperFeedback.init({ backendUrl, repo, app, theme: "auto", nudge: true });

const SESSION_ID = (() => { try { return globalThis.crypto.randomUUID(); } catch (_) { return `${Date.now()}-${Math.random().toString(36).slice(2)}`; } })();
const CAPTURE_CDN = "https://esm.sh/html-to-image@1.11.13";
const ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>`;
const IMG_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="M21 15l-5-5L5 21"/></svg>`;
const TYPES = [
  { v: "bug", label: "Bug", emoji: "🐞" },
  { v: "feature", label: "Idea", emoji: "✨" },
  { v: "other", label: "Other", emoji: "💬" },
];

// Markup editor: resolution-independent shapes ({ tool, color, points } in 0…1 image
// coordinates) drawn over the capture and composited at native size only on Done.
const SVG = (body) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;
const PEN_ICON = SVG(`<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/>`);
const MARKUP_TOOLS = [
  { v: "pen", label: "Pen", icon: PEN_ICON },
  { v: "circle", label: "Circle", icon: SVG(`<circle cx="12" cy="12" r="9"/>`) },
  { v: "arrow", label: "Arrow", icon: SVG(`<path d="M5 19L19 5"/><path d="M11 5h8v8"/>`) },
  { v: "rect", label: "Rectangle", icon: SVG(`<rect x="4" y="5" width="16" height="14" rx="1.5"/>`) },
];
const UNDO_ICON = SVG(`<path d="M3 7v6h6"/><path d="M3.5 13a9 9 0 1 0 2.2-6.1L3 10"/>`);
const CLEAR_ICON = SVG(`<path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M6 6l1 14h10l1-14"/>`);
const MARKUP_COLORS = [{ v: "#ff3b30", label: "Red" }, { v: "#ffcc00", label: "Yellow" }, { v: "#0a84ff", label: "Blue" }];
const MAX_SHOT_BYTES = 2e6;
// Feedback about the widget itself ("SuperFeedback within SuperFeedback") files here.
const WIDGET_FEEDBACK_REPO = "lman80/SuperFeedback";

const SuperFeedback = {
  version: "3.3.1",
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
      position: config.trigger === "floating" ? "bottom-right" : "right-center", label: "Feedback", color: "#6d5efc", theme: "auto",
      type: "bug", attachScreenshot: true, trigger: "draggable", mount: null,
      community: true, supportNudge: true, checkin: true, compact: false, nudge: false, maxImages: 5, captureLogs: true, maxLogs: 200, captureCrashes: true, markup: true,
      widgetFeedback: WIDGET_FEEDBACK_REPO,
      ...config,
    };
    if (this._cfg.mount) this._cfg.trigger = "mounted";
    this._voter = storageRead("superfeedback:voter") || globalThis.crypto.randomUUID();
    storageWrite("superfeedback:voter", this._voter);
    if (this._cfg.support?.products && !this._cfg.support.url && !this._warnedProducts) {
      console.warn("[SuperFeedback] Web support requires a Stripe url; products are ignored.");
      this._warnedProducts = true;
    }
    this._community = null; this._communityAt = 0; this._communityUnavailable = false;
    this._communityFlight = null; this._voteFlights = new Set(); this._voteRevision = 0; this._communityStale = true;
    try {
      const cached = JSON.parse(storageRead(this._key("community"), "null"));
      if (cached?.data?.ok) { this._community = cached.data; this._communityAt = cached.t; }
    } catch (_) {}
    this._context = {};
    this._startedAt = Date.now();
    this._enabled = storageRead(this._key("enabled"), "1") !== "0";
    this._logs = [];
    if (this._cfg.captureLogs) this._installLogCapture();
    if (this._cfg.captureCrashes) this._installCrashCapture();
    const start = () => this._mount();
    if (document.readyState === "loading") this._listen(document, "DOMContentLoaded", start);
    else start();
    if (typeof window !== "undefined") window.SuperFeedback = SuperFeedback;
  },

  async open() {
    const host = this._panelHost;
    if (!host) return;
    this._hideNudge();
    host.__selectTab("feedback");
    this.fetchCommunity();
    if (host.__isOpen() || this._opening) return this._opening;
    const token = {};
    this._openToken = token;
    let focused = document.activeElement;
    while (focused?.shadowRoot?.activeElement) focused = focused.shadowRoot.activeElement;
    host.__returnFocus = focused;
    const opening = this._opening = (async () => {
      this._setHostsVisible(false);
      let shot = null, failure = null;
      try {
        // Give native window capture a painted frame with both hosts hidden.
        // (rAF never fires in a background tab, so cap the wait — the panel must always open.)
        await new Promise((resolve) => { requestAnimationFrame(() => requestAnimationFrame(resolve)); setTimeout(resolve, 150); });
        if (this._panelHost !== host) return;
        // Never leave the user staring at a hidden button: a slow capture library or a hung
        // native bridge must not block the panel. The report still sends, with the reason in meta.
        shot = await Promise.race([
          this._capture(),
          new Promise((_, reject) => setTimeout(() => reject(new Error("timeout after 8s")), 8000)),
        ]);
        if (!shot) failure = "capture returned no image";
      } catch (error) {
        // html-to-image rejects with the raw image-load Event when a resource can't be fetched.
        failure = typeof Event !== "undefined" && error instanceof Event
          ? `${error.type} event (a page resource could not be loaded)` : safeStr(error?.message || error);
      }
      finally {
        if (this._panelHost === host) this._setHostsVisible(true);
      }
      if (this._panelHost === host && this._openToken === token) host.__open(shot, failure);
    })();
    try { await opening; }
    finally { if (this._opening === opening) this._opening = null; }
  },
  voterId() { return this._voter; },
  async openIdeas() { const host = this._panelHost; await this.open(); if (host === this._panelHost && host?.__isOpen()) host.__selectTab("ideas", true); },
  async openSupport() { const host = this._panelHost; await this.open(); if (host === this._panelHost && host?.__isOpen()) host.__selectTab("support", true); },
  close() { this._openToken = null; this._panelHost?.__close(); },
  toggle() { return this._panelHost?.__isOpen() ? this.close() : this.open(); },
  setEnabled(value) {
    this._enabled = !!value;
    if (this._cfg) storageWrite(this._key("enabled"), this._enabled ? "1" : "0");
    if (this._triggerHost) this._triggerHost.style.display = this._enabled ? "" : "none";
    if (!this._enabled) this._hideNudge();
  },
  isEnabled() { return this._enabled !== false; },
  setContext(context) {
    this._context = { ...this._context, ...context };
    if (this._context.busy === "true") this._hideNudge();
  },
  moment(name) {
    if (typeof name !== "string" || !name.trim()) return;
    const count = Math.max(0, Math.floor(Number(storageRead("superfeedback:moments", "0")) || 0));
    storageWrite("superfeedback:moments", String(Math.min(1000, count + 1)));
    storageWrite("superfeedback:lastMoment", name);
    rememberMoment(name);
    const now = Date.now();
    if (this._lastMomentEvaluation != null && now - this._lastMomentEvaluation < 2000) return;
    this._lastMomentEvaluation = now;
    this._maybeSupportNudge();
  },
  // Effective Stripe link: local config wins, else the studio link the backend serves (fund.supportUrl).
  _supportUrl() { return this._cfg?.support?.url || (typeof this._community?.fund?.supportUrl === "string" && this._community.fund.supportUrl) || null; },
  _hideNudge() {
    clearTimeout(this._nudgeHideTimer);
    const nudge = this._panelHost?.shadowRoot.querySelector(".sf-nudge");
    if (nudge) { nudge.classList.remove("sf-show"); nudge.inert = true; }
  },
  _key(suffix, cfg = this._cfg) { return `superfeedback:${cfg.repo}:${suffix}`; },
  _listen(target, event, listener, options) {
    target.addEventListener(event, listener, options);
    (this._cleanups ||= []).push(() => target.removeEventListener(event, listener, options));
  },

  // Record a custom breadcrumb / error code; included with the next report.
  log(message, level = "info") { if (this._cfg?.captureLogs) this._pushLog(level, typeof message === "string" ? message : safeStr(message)); },

  destroy() {
    this.close();
    this._opening = null;
    clearTimeout(this._nudgeTimer);
    clearTimeout(this._nudgeHideTimer);
    clearTimeout(this._communityTimer);
    clearTimeout(this._checkinTimer);
    for (const cleanup of this._cleanups || []) cleanup();
    this._cleanups = [];
    for (const h of [this._panelHost, this._triggerHost]) if (h && h.parentNode) h.parentNode.removeChild(h);
    this._panelHost = this._triggerHost = null;
    this._cfg = null;
  },

  _mount() {
    this._mountPanel();
    const t = this._cfg.trigger;
    if (t === "mounted") this._mountTriggerInto(this._cfg.mount);
    else if (t === "draggable") this._mountDraggable();
    else if (t !== "none") this._mountFloating();
    this.setEnabled(this._enabled);
    this._prewarmCapture();
    this._listen(window, "online", () => this._flushOutbox());
    storageWrite(this._key("launches"), String((Number(storageRead(this._key("launches"), "0")) || 0) + 1));
    this._maybeNudge();
    const cfg = this._cfg;
    const flushed = cfg.captureCrashes ? this._flushPendingCrashes() : this._flushOutbox();
    this._communityTimer = setTimeout(async () => {
      await flushed;
      if (this._cfg === cfg && navigator.onLine !== false) this.fetchCommunity();
    }, 3000);
    if (cfg.checkin !== false) this._checkinTimer = setTimeout(() => this._checkin(cfg), 2000);
  },

  // Fleet check-in (PROTOCOL.md "POST /checkin"): once per UTC day per repo, or right away when the
  // widget or app version changed. Fire-and-forget: no retry, no outbox, silent on any failure.
  _checkin(cfg = this._cfg) {
    if (!cfg || cfg.checkin === false || checkinFlights.has(cfg.repo)) return Promise.resolve(false);
    const key = this._key("checkin", cfg), day = new Date().toISOString().slice(0, 10);
    const widget = `web/${this.version}`, appVersion = cfg.appVersion ? String(cfg.appVersion) : "";
    const build = cfg.build ?? cfg.meta?.build, stamp = JSON.stringify({ day, widget, appVersion, build: build ? String(build) : "" });
    // The in-memory stamp covers re-init in the same page and pages where localStorage is unavailable.
    if (checkinStamps.get(cfg.repo) === stamp || storageRead(key, null) === stamp) return Promise.resolve(false);
    const body = { repo: cfg.repo, app: cfg.app || "", widget, appVersion, ...(build ? { build: String(build) } : {}),
      platform: "web", os: webOS(), config: {
      trigger: cfg.trigger, position: cfg.position, theme: cfg.theme, type: cfg.type,
      community: cfg.community !== false, captureLogs: !!cfg.captureLogs, captureCrashes: !!cfg.captureCrashes,
      markup: cfg.markup !== false, attachScreenshot: cfg.attachScreenshot !== false, maxImages: cfg.maxImages,
      compact: !!cfg.compact, nudge: !!cfg.nudge, widgetFeedback: widgetFeedbackRepo(cfg) || false,
      supportNudge: cfg.supportNudge !== false,
      support: cfg.support?.url ? "url" : cfg.community !== false ? "backend" : "off",
      appKey: !!cfg.appKey, enabled: this._cfg === cfg ? this.isEnabled() : storageRead(this._key("enabled", cfg), "1") !== "0",
      moments: momentNames(),
    } };
    checkinFlights.add(cfg.repo);
    let request;
    try {
      request = fetch(cfg.backendUrl.replace(/\/$/, "") + "/checkin", { method: "POST", keepalive: true,
        // A CORS-simple content type: no preflight, which older Chromium/Electron refuse for keepalive.
        headers: { "Content-Type": "text/plain;charset=UTF-8" }, body: JSON.stringify(body) });
    } catch (_) { request = Promise.reject(); }
    // Any HTTP answer (including an old backend's 405) counts for today; offline tries again next launch.
    return Promise.resolve(request).then(() => { checkinStamps.set(cfg.repo, stamp); storageWrite(key, stamp); return true; }, () => false)
      .finally(() => checkinFlights.delete(cfg.repo));
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
    let draft = 0;
    const max = Math.max(0, this._cfg.maxImages ?? 5);
    const fileInput = $(".sf-file"), thumbs = $(".sf-thumbs"), addBtn = $(".sf-addimg");
    const renderThumbs = () => {
      thumbs.innerHTML = host.__images.map((_, i) =>
        `<span class="sf-thumb"><button class="sf-thumb-x" type="button" data-i="${i}" aria-label="Remove image">✕</button></span>`).join("");
      // CSSOM assignment also works under nonce-only style-src (inline attributes do not).
      thumbs.querySelectorAll(".sf-thumb").forEach((thumb, i) => { thumb.style.backgroundImage = `url("${host.__images[i]}")`; });
      thumbs.querySelectorAll(".sf-thumb-x").forEach((b) =>
        b.addEventListener("click", () => { host.__images.splice(+b.dataset.i, 1); renderThumbs(); syncAdd(); }));
    };
    const syncAdd = () => { addBtn.style.display = host.__images.length >= max ? "none" : ""; };
    host.__clearImages = () => { draft++; host.__images = []; renderThumbs(); syncAdd(); };
    addBtn.addEventListener("click", () => fileInput.click());
    fileInput.addEventListener("change", async () => {
      const generation = draft;
      const files = Array.from(fileInput.files || []); fileInput.value = "";
      for (const f of files.slice(0, max - host.__images.length)) {
        try {
          const image = await fileToDataURL(f);
          if (draft !== generation || this._panelHost !== host) return;
          if (host.__images.length < max) host.__images.push(image);
        } catch (_) {}
      }
      renderThumbs(); syncAdd();
    });

    // Screenshot row: clean capture + markup shapes. The annotated PNG is only composited
    // on Done, so re-opening the editor always draws on the untouched capture.
    host.__shapes = [];
    host.__originalShot = null;
    const syncShot = () => {
      const count = host.__shapes.length;
      $(".sf-row").hidden = !host.__originalShot;
      if (host.__screenshot) $(".sf-shot-preview").src = host.__screenshot;
      else $(".sf-shot-preview").removeAttribute("src");
      const open = $(".sf-mk-open-label");
      if (open) open.textContent = count ? "Edit markup" : "Mark up";
    };
    host.__syncShot = syncShot;
    const markup = this._cfg.markup === false ? null : installMarkup(root, host, async (shapes) => {
      // Composite first, publish after: a send during the export must never claim
      // annotations the stored screenshot does not have yet.
      const base = host.__originalShot;
      let annotated = base, kept = shapes;
      try { if (shapes.length) annotated = await compositeMarkup(base, shapes); }
      catch (_) { kept = []; host.__toast("Couldn't save the markup", "err"); }
      if (this._panelHost !== host || host.__originalShot !== base) return;
      host.__shapes = kept; host.__screenshot = annotated;
      syncShot();
    });
    host.__markup = markup;
    root.querySelectorAll(".sf-markup-open").forEach((b) => b.addEventListener("click", () => markup?.open()));

    const resetDrag = installPanelDrag(modal, $(".sf-head"), (fn) => this._listen(window, "resize", fn));
    host.__isOpen = () => modal.classList.contains("sf-show");
    host.__open = (shot, failure) => {
      this._hideNudge();
      host.__captureFailure = failure;
      host.__screenshot = shot;
      host.__originalShot = shot;
      host.__shapes = [];
      syncShot();
      $(".sf-shot").checked = this._cfg.attachScreenshot;
      resetDrag();
      modal.inert = false; modal.setAttribute("aria-hidden", "false");
      backdrop.classList.add("sf-show"); modal.classList.add("sf-show");
      setTimeout(() => { if (host.__isOpen() && host.__tab === "feedback") $(".sf-text").focus(); }, 60);
    };
    host.__close = () => {
      markup?.close(false);
      const focused = host.__returnFocus;
      host.__returnFocus = null;
      if (focused?.isConnected) focused.focus({ preventScroll: true });
      modal.inert = true; modal.setAttribute("aria-hidden", "true");
      backdrop.classList.remove("sf-show"); modal.classList.remove("sf-show");
      host.__clearImages(); host.__screenshot = null;
      host.__originalShot = null; host.__shapes = [];
      $(".sf-text").value = ""; $(".sf-send").disabled = true;
      host.__setWidgetMode(false);
      root.querySelectorAll(".sf-seg-btn[data-type]").forEach((b) => b.classList.toggle("sf-active", b.dataset.type === this._cfg.type));
    };
    host.__toggle = () => this.toggle();
    // Widget-feedback mode: the same card, filed to the SuperFeedback repo instead of the app's.
    const widgetLink = $(".sf-widget-fb");
    host.__widgetMode = false;
    host.__setWidgetMode = (on) => {
      host.__widgetMode = !!on && !!widgetFeedbackRepo(this._cfg);
      const w = host.__widgetMode, app = this._cfg.app || "";
      $(".sf-feedback-title").textContent = w ? "Feedback on SuperFeedback" : "Send feedback";
      $(".sf-feedback-sub").textContent = w ? "" : app;
      $(".sf-text").placeholder = w ? "What about this feedback card could be better?" : "What went wrong, or what would you like?";
      if (widgetLink) widgetLink.textContent = w ? `← Back to ${app ? "feedback for " + app : "feedback"}` : "Feedback on SuperFeedback";
    };
    widgetLink?.addEventListener("click", () => { host.__setWidgetMode(!host.__widgetMode); $(".sf-text").focus(); });
    $(".sf-text").addEventListener("input", () => { $(".sf-send").disabled = !$(".sf-text").value.trim(); });
    host.__clearImages();
    let toastTimer = null;
    const toastQueue = [];
    const nextToast = () => {
      const item = toastQueue.shift();
      if (!item) { toastTimer = null; toast.className = "sf-toast"; return; }
      toast.textContent = item.msg;
      toast.className = "sf-toast sf-show" + (item.kind ? " sf-" + item.kind : "");
      toastTimer = setTimeout(nextToast, item.duration);
      item.onShown?.();
    };
    host.__toast = (msg, kind = "", duration = kind === "err" ? 7000 : 2600, onShown) => {
      toastQueue.push({ msg, kind, duration, onShown });
      if (!toastTimer) nextToast();
    };
    this._cleanups.push(() => clearTimeout(toastTimer));
    this._installCommunityViews(host);

    root.querySelectorAll(".sf-seg-btn[data-type]").forEach((b) =>
      b.addEventListener("click", () => {
        root.querySelectorAll(".sf-seg-btn[data-type]").forEach((x) => x.classList.remove("sf-active"));
        b.classList.add("sf-active");
      }));

    backdrop.addEventListener("click", host.__close);
    $(".sf-cancel").addEventListener("click", host.__close);
    $(".sf-send").addEventListener("click", () => this._submit(root));
    this._listen(document, "keydown", (e) => {
      if (!host.__isOpen()) return;
      // While the markup editor is up it owns Escape and the focus trap: cancelling the
      // drawing must never also close the panel and throw away the draft.
      const editing = !!markup?.isOpen();
      if (e.key === "Escape") {
        e.preventDefault(); e.stopPropagation();
        if (editing) markup.close(false); else this.close();
        return;
      }
      if (e.key !== "Tab") return;
      const scope = editing ? markup.element() : modal;
      const focusable = [...scope.querySelectorAll('button,input,textarea,select,a[href],[tabindex]')]
        .filter((el) => !el.disabled && el.tabIndex >= 0 && el.getClientRects().length && getComputedStyle(el).visibility !== "hidden");
      const first = focusable[0], last = focusable[focusable.length - 1];
      const active = root.activeElement;
      if (!scope.contains(active) || (e.shiftKey ? active === first : active === last)) {
        e.preventDefault(); (e.shiftKey ? last : first)?.focus();
      }
    }, true);
  },

  _mountDraggable() {
    const cfg = this._cfg, key = this._key("pos");
    const host = document.createElement("div");
    host.setAttribute("data-superfeedback-trigger", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = `<style nonce="${esc(cfg.styleNonce || "")}">
      :host { all: initial; position: fixed; z-index: 2147482999; width: 50px; height: 50px; }
      .sf-safe { position: fixed; visibility: hidden; pointer-events: none;
        padding: env(safe-area-inset-top, 0px) env(safe-area-inset-right, 0px) env(safe-area-inset-bottom, 0px) env(safe-area-inset-left, 0px); }
      .sf-fab { display: grid; place-items: center; width: 50px; height: 50px; padding: 0;
        border: none; border-radius: 50%; background: ${cfg.color}; color: white; cursor: pointer;
        box-shadow: 0 6px 20px ${hexA(cfg.color, .35)}; touch-action: none; opacity: .62;
        transition: opacity .15s, transform .15s; }
      .sf-fab:hover, .sf-fab.sf-pressed { opacity: 1; }
      .sf-fab.sf-dragging { transform: scale(1.08); }
      svg { width: 23px; height: 23px; pointer-events: none; }
    </style><span class="sf-safe"></span><button class="sf-fab" type="button" aria-label="Send feedback">${ICON}</button>`;
    document.body.appendChild(host); this._triggerHost = host;
    const button = root.querySelector("button");
    const bounds = () => {
      const css = getComputedStyle(root.querySelector(".sf-safe"));
      return { left: 12 + parseFloat(css.paddingLeft), right: Math.max(12 + parseFloat(css.paddingLeft), innerWidth - 62 - parseFloat(css.paddingRight)),
        top: 12 + parseFloat(css.paddingTop), bottom: Math.max(12 + parseFloat(css.paddingTop), innerHeight - 62 - parseFloat(css.paddingBottom)) };
    };
    let pos = { side: cfg.position.includes("left") ? "left" : "right", y: cfg.position.startsWith("top") ? 0 : cfg.position.startsWith("bottom") ? 1 : .5 };
    try {
      const saved = JSON.parse(storageRead(key, "null"));
      if (saved && ["left", "right"].includes(saved.side) && Number.isFinite(saved.y)) pos = { side: saved.side, y: fit(0, 1, saved.y) };
    } catch (_) {}
    let x = 0, y = 0, drag = null;
    const apply = () => { host.style.left = `${x}px`; host.style.top = `${y}px`; };
    const restore = () => { const b = bounds(); x = b[pos.side]; y = b.top + pos.y * (b.bottom - b.top); apply(); };
    const snap = () => {
      const b = bounds(); pos.side = x + 25 < innerWidth / 2 ? "left" : "right";
      x = b[pos.side]; y = fit(b.top, b.bottom, y); pos.y = (y - b.top) / (b.bottom - b.top || 1);
      apply(); storageWrite(key, JSON.stringify(pos));
    };
    button.addEventListener("pointerdown", (e) => {
      if (!e.isPrimary || e.button !== 0 || drag) return;
      drag = { id: e.pointerId, sx: e.clientX, sy: e.clientY, ox: x, oy: y, moved: false };
      button.classList.add("sf-pressed"); button.setPointerCapture(e.pointerId); e.preventDefault();
    });
    button.addEventListener("pointermove", (e) => {
      if (!drag || drag.id !== e.pointerId) return;
      if (Math.hypot(e.clientX - drag.sx, e.clientY - drag.sy) > 6) drag.moved = true;
      if (!drag.moved) return;
      button.classList.add("sf-dragging");
      x = drag.ox + e.clientX - drag.sx; y = drag.oy + e.clientY - drag.sy; apply();
    });
    const end = (e) => {
      if (!drag || drag.id !== e.pointerId) return;
      const tap = !drag.moved && e.type === "pointerup";
      drag = null; button.classList.remove("sf-pressed", "sf-dragging");
      if (button.hasPointerCapture(e.pointerId)) button.releasePointerCapture(e.pointerId);
      snap(); if (tap) this.open();
    };
    for (const type of ["pointerup", "pointercancel", "lostpointercapture"]) button.addEventListener(type, end);
    // Pointer taps are handled above; keyboard/assistive activation remains accessible.
    button.addEventListener("click", (e) => { if (e.detail === 0) this.open(); });
    this._listen(window, "resize", () => {
      if (drag) end({ pointerId: drag.id, type: "pointercancel" });
      restore();
    });
    restore();
  },

  _mountFloating() {
    const host = document.createElement("div");
    host.setAttribute("data-superfeedback-trigger", "");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = FLOATING_TEMPLATE(this._cfg);
    document.body.appendChild(host);
    this._triggerHost = host;
    root.querySelector(".sf-fab").addEventListener("click", () => this.open());
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

  fetchCommunity() {
    const cfg = this._cfg, host = this._panelHost;
    if (!cfg || cfg.community === false || this._legacyBackends?.has(cfg.backendUrl)) return Promise.resolve();
    if (this._communityFlight) return this._communityFlight;
    if (navigator.onLine === false) { host?.__renderCommunity(); return Promise.resolve(); }
    const revision = this._voteRevision;
    const flight = (async () => {
      try {
        const res = await fetch(cfg.backendUrl.replace(/\/$/, "") + `/community?repo=${encodeURIComponent(cfg.repo)}&voter=${encodeURIComponent(this._voter)}`);
        let data; try { data = await res.json(); } catch (_) {}
        if (this._cfg !== cfg) return;
        if (!res.ok || data?.ok === false) {
          if (res.status === 503 && data?.error === "community not configured") (this._legacyBackends ||= new Set()).add(cfg.backendUrl);
          this._communityUnavailable = true;
          return;
        }
        if (!data?.ok || !Array.isArray(data.ideas) || !Array.isArray(data.shipped) || !data.you || !data.fund) { this._communityUnavailable = true; return; }
        // Preserve votes changed after this request started.
        if (this._community && (this._voteFlights.size || revision !== this._voteRevision)) {
          data.ideas = this._community.ideas;
          data.you.votes = this._community.you.votes;
          Object.assign(this._community, data);
          data = this._community;
        }
        if (data.you.supporter && (!this._community?.you.supporter || !storageRead("superfeedback:lastTip"))) {
          storageWrite("superfeedback:lastTip", String(Date.now()));
        }
        this._community = data; this._communityAt = Date.now(); this._communityUnavailable = false; this._communityStale = false;
        if (!this._voteFlights.size) storageWrite(this._key("community", cfg), JSON.stringify({ t: this._communityAt, data }));
        let seen;
        try { seen = JSON.parse(storageRead(this._key("seen-notices", cfg), "[]")); } catch (_) {}
        const ids = new Set(Array.isArray(seen) ? seen : []);
        for (const notice of data.notices || []) if (!ids.has(notice.id)) {
          host?.__toast(`🎉 Your suggestion shipped in ${notice.version}: ${notice.title}`, "", 6000, () => {
            const timer = setTimeout(() => { if (this._cfg === cfg) this._maybeSupportNudge(notice.id); }, 5000);
            this._cleanups.push(() => clearTimeout(timer));
          });
          ids.add(notice.id);
        }
        storageWrite(this._key("seen-notices", cfg), JSON.stringify([...ids]));
      } catch (_) {
        if (this._cfg === cfg) this._communityStale = true;
      } finally {
        if (this._cfg === cfg) { this._communityFlight = null; host?.__renderCommunity(); }
      }
    })();
    this._communityFlight = flight;
    host?.__renderCommunity();
    return flight;
  },

  async _vote(id) {
    const cfg = this._cfg, host = this._panelHost, data = this._community;
    const idea = data?.ideas.find((idea) => idea.id === id);
    if (!idea || this._voteFlights.has(id)) return;
    const before = { votes: idea.votes, voted: idea.voted }, delta = idea.voted ? -1 : 1;
    this._voteFlights.add(id); this._voteRevision++;
    idea.voted = !idea.voted; idea.votes += delta; data.you.votes += delta;
    host.__renderCommunity();
    try {
      const res = await fetch(cfg.backendUrl.replace(/\/$/, "") + "/vote", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ repo: cfg.repo, ideaId: id, voter: this._voter, on: idea.voted }),
      });
      const body = await res.json();
      if (!res.ok || body.ok !== true) throw new Error("Vote failed");
      if (this._cfg !== cfg) return;
      data.you.votes += Number(body.voted) - Number(idea.voted);
      idea.votes = body.votes; idea.voted = body.voted;
    } catch (_) {
      if (this._cfg !== cfg) return;
      Object.assign(idea, before); data.you.votes -= delta;
      host.__toast("Couldn't vote — try again", "err");
    } finally {
      if (this._cfg === cfg) {
        this._voteFlights.delete(id);
        if (!this._voteFlights.size) storageWrite(this._key("community"), JSON.stringify({ t: this._communityAt, data }));
        host.__renderCommunity();
      }
    }
  },

  _installCommunityViews(host) {
    const root = host.shadowRoot, $ = (s) => root.querySelector(s);
    const tabs = [...root.querySelectorAll("[data-tab]")];
    host.__tab = "feedback";
    let filter = "top";
    host.__selectTab = (name, focus = false) => {
      const target = tabs.find((b) => b.dataset.tab === name && !b.hidden);
      name = target ? name : "feedback";
      host.__tab = name;
      tabs.forEach((b) => {
        const active = b.dataset.tab === name;
        b.classList.toggle("sf-active", active); b.setAttribute("aria-selected", String(active)); b.tabIndex = active ? 0 : -1;
      });
      root.querySelectorAll("[data-view]").forEach((view) => { view.hidden = view.dataset.view !== name; });
      $(".sf-modal").setAttribute("aria-label", name === "ideas" ? "Ideas & roadmap" : name === "support" ? "Support development" : "Send feedback");
      if (focus) (target || $(".sf-text")).focus();
    };
    tabs.forEach((b) => {
      b.addEventListener("click", () => host.__selectTab(b.dataset.tab));
      b.addEventListener("keydown", (e) => {
        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
        e.preventDefault();
        const visible = tabs.filter((tab) => !tab.hidden), i = visible.indexOf(b);
        const next = e.key === "Home" ? 0 : e.key === "End" ? visible.length - 1 : (i + (e.key === "ArrowRight" ? 1 : -1) + visible.length) % visible.length;
        host.__selectTab(visible[next].dataset.tab, true);
      });
    });
    $(".sf-suggest").addEventListener("click", () => {
      host.__selectTab("feedback");
      root.querySelector('[data-type="feature"]').click(); $(".sf-text").focus();
    });
    root.querySelectorAll("[data-filter]").forEach((b) => b.addEventListener("click", () => { filter = b.dataset.filter; host.__renderCommunity(); }));
    $(".sf-ideas-list").addEventListener("click", (e) => { const b = e.target.closest("[data-vote]"); if (b) this._vote(Number(b.dataset.vote)); });
    $(".sf-support-pay").addEventListener("click", () => {
      const url = this._supportUrl();
      window.open(url + (url.includes("?") ? "&" : "?") + "client_reference_id=" + encodeURIComponent(this._voter), "_blank", "noopener");
    });
    host.__renderCommunity = () => {
      const data = this._community, cfg = this._cfg;
      tabs.find((b) => b.dataset.tab === "ideas").hidden = cfg.community === false || this._communityUnavailable || !!this._legacyBackends?.has(cfg.backendUrl);
      tabs.find((b) => b.dataset.tab === "support").hidden = !this._supportUrl();
      $(".sf-tabs").hidden = tabs.filter((b) => !b.hidden).length === 1;
      const focusedTab = tabs.includes(root.activeElement) || (host.__isOpen() && tabs.some((b) => b.dataset.tab === host.__tab && b.hidden));
      host.__selectTab(host.__tab, focusedTab);
      $(".sf-supporter").hidden = !data?.you.supporter;
      $(".sf-thanks").hidden = !data?.you.supporter;
      root.querySelectorAll("[data-filter]").forEach((b) => { b.classList.toggle("sf-active", b.dataset.filter === filter); b.setAttribute("aria-pressed", String(b.dataset.filter === filter)); });
      const ideas = [...(filter === "shipped" ? data?.shipped || [] : data?.ideas || [])];
      if (filter !== "shipped") ideas.sort(filter === "new" ? (a, b) => b.createdAt - a.createdAt : (a, b) => b.votes - a.votes || b.createdAt - a.createdAt);
      const focusId = root.activeElement?.dataset.vote;
      $(".sf-ideas-list").innerHTML = ideas.map((idea) => {
        const status = idea.status === "shipped" ? `Shipped${idea.version ? " v" + idea.version : ""}` : idea.status.charAt(0).toUpperCase() + idea.status.slice(1);
        return `<div class="sf-idea"><div class="sf-idea-copy"><div class="sf-idea-title">${esc(idea.title)}</div><div class="sf-summary">${esc(idea.summary)}</div><span class="sf-status">${esc(status)}</span></div>${filter === "shipped" ? `<span class="sf-sub">${plural(idea.votes, "vote")}</span>` : `<button type="button" class="sf-vote${idea.voted ? " sf-active" : ""}" data-vote="${idea.id}" aria-pressed="${idea.voted}" aria-label="${esc(idea.voted ? "Remove vote" : `Vote for ${idea.title}, ${plural(idea.votes, "vote")}`)}">⌃<br>${idea.votes}</button>`}</div>`;
      }).join("") || `<p class="sf-caption">${data ? "No public ideas yet. Be the first." : "Loading…"}</p>`;
      if (focusId) $(".sf-ideas-list").querySelector(`[data-vote="${focusId}"]`)?.focus();
      const age = Math.max(0, Math.floor((Date.now() - this._communityAt) / 60000));
      $(".sf-updated").textContent = data && (this._communityFlight || this._communityStale || navigator.onLine === false) ? `Last updated ${age < 1 ? "just now" : age < 60 ? age + " minutes ago" : age < 1440 ? Math.floor(age / 60) + " hours ago" : Math.floor(age / 1440) + " days ago"}` : "";
      $(".sf-you").textContent = data ? `You: ${plural(data.you.votes, "vote")} · ${plural(data.you.submitted, "idea")} · ${data.you.shipped} shipped` : "";
      const fund = data?.fund;
      $(".sf-fund").hidden = !fund;
      if (fund) {
        const money = new Intl.NumberFormat(undefined, { style: "currency", currency: fund.currency, maximumFractionDigits: 0 });
        $(".sf-fund-money").textContent = `${money.format(fund.monthCents / 100)} / ${money.format(fund.goalCents / 100)}`;
        const percent = fund.goalCents > 0 ? fit(0, 100, fund.monthCents / fund.goalCents * 100) : 0;
        $(".sf-meter-fill").style.width = `${percent}%`;
        $(".sf-meter").setAttribute("aria-valuenow", String(percent));
        $(".sf-fund-caption").textContent = "Community development fund · " + new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric", timeZone: "UTC" }).format(new Date());
        $(".sf-fund-stats").textContent = `${fund.supporters} supporters this month · ${fund.shippedThisMonth} improvements shipped`;
      }
    };
    host.__renderCommunity();
  },

  _maybeSupportNudge(ideaId) {
    const cfg = this._cfg, host = this._panelHost;
    if (!cfg || !this._supportUrl() || cfg.supportNudge === false || !host) return;
    const o = typeof cfg.supportNudge === "object" && cfg.supportNudge || {};
    const thankYou = ideaId != null, now = Date.now(), key = this._key("nudge");
    if (!this.isEnabled() || host.__isOpen() || this._opening || this._sessionNudge ||
      now - this._startedAt < 10000 || this._context.busy === "true" || this._community?.you.supporter ||
      storageRead("superfeedback:supportNudgeOptOut") === "1" ||
      now - Number(storageRead(key, "0")) < (o.cooldownDays ?? 30) * 864e5 ||
      now - Number(storageRead("superfeedback:lastTip", "0")) < (o.afterTipDays ?? 180) * 864e5) return;
    let shown = [];
    if (thankYou) {
      if (o.thankYou === false) return;
      try { const ids = JSON.parse(storageRead(this._key("thankyou-shown"), "[]")); if (Array.isArray(ids)) shown = ids; } catch (_) {}
      if (shown.includes(ideaId)) return;
    } else if (Number(storageRead(this._key("launches"), "0")) < (o.minLaunches ?? 3) ||
      Number(storageRead("superfeedback:moments", "0")) < (o.minMoments ?? 3)) return;
    this._sessionNudge = "support"; storageWrite(key, String(now));
    if (thankYou) storageWrite(this._key("thankyou-shown"), JSON.stringify([...shown, ideaId]));
    const purpose = typeof cfg.support.purpose === "string" && cfg.support.purpose.trim()
      ? cfg.support.purpose.trim().slice(0, 60) : "the AI tools and servers";
    const supporters = this._community?.fund.supporters;
    const root = host.shadowRoot, nudge = root.querySelector(".sf-nudge");
    root.querySelector(".sf-nudge-msg").textContent = thankYou
      ? "You helped build this. Want to help fund the next one? 💜"
      : `This app is free. If it's useful, you can help fund ${purpose} that keep it improving 💜` +
        (supporters >= 5 ? ` ${supporters} people supported this month.` : "");
    const button = root.querySelector(".sf-nudge-open"); button.textContent = "Support";
    const dismiss = () => { this._hideNudge(); storageWrite(key, String(Date.now())); };
    button.onclick = () => { this._hideNudge(); this.openSupport(); };
    root.querySelector(".sf-nudge-x").hidden = true;
    root.querySelector(".sf-nudge-links").hidden = false;
    root.querySelector(".sf-nudge-later").onclick = dismiss;
    root.querySelector(".sf-nudge-optout").onclick = () => {
      storageWrite("superfeedback:supportNudgeOptOut", "1"); dismiss();
    };
    nudge.inert = false; nudge.classList.add("sf-show");
    this._nudgeHideTimer = setTimeout(dismiss, 14000);
  },

  _maybeNudge() {
    const n = this._cfg.nudge;
    if (!n || !this._panelHost) return;
    const o = typeof n === "object" ? n : {};
    const delay = o.delayMs ?? 45000, cooldown = (o.cooldownDays ?? 7) * 864e5;
    const msg = o.message || "Got feedback? We'd love to hear it 💜";
    const key = this._key("nudge");
    try { if (Date.now() - parseInt(localStorage.getItem(key) || "0", 10) < cooldown) return; } catch (_) {}
    this._nudgeTimer = setTimeout(() => {
      const host = this._panelHost; if (!host || !this.isEnabled() || host.__isOpen() || this._opening || this._context.busy === "true" || Date.now() - this._startedAt < 10000 || this._sessionNudge || Date.now() - Number(storageRead(key, "0")) < cooldown) return;
      this._sessionNudge = "feedback";
      storageWrite(key, String(Date.now()));
      const root = host.shadowRoot, nudge = root.querySelector(".sf-nudge");
      root.querySelector(".sf-nudge-msg").textContent = msg;
      root.querySelector(".sf-nudge-open").textContent = "Sure";
      root.querySelector(".sf-nudge-links").hidden = true;
      root.querySelector(".sf-nudge-x").hidden = false;
      nudge.inert = false; nudge.classList.add("sf-show");
      const remember = () => { try { localStorage.setItem(key, String(Date.now())); } catch (_) {} };
      const hide = () => this._hideNudge();
      root.querySelector(".sf-nudge-open").onclick = () => { hide(); remember(); this.open(); };
      root.querySelector(".sf-nudge-x").onclick = () => { hide(); remember(); };
      this._nudgeHideTimer = setTimeout(hide, 14000);
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
    const active = root.querySelector(".sf-seg-btn[data-type].sf-active");
    const host = this._panelHost, shapes = host.__shapes?.length || 0;
    const attached = shapes ? `attached (annotated, ${shapes} shape${shapes === 1 ? "" : "s"})` : "attached";
    const payload = {
      message, type: active ? active.dataset.type : this._cfg.type,
      images: (host.__images || []).slice(),
      logs: this._serializeLogs(), screenshot: $(".sf-shot").checked ? host.__screenshot : null, meta: this._meta(
        host.__captureFailure ? `capture failed: ${host.__captureFailure}` :
          $(".sf-shot").checked && host.__screenshot ? attached : "declined"),
    };
    if (host.__widgetMode) {
      Object.assign(payload, { repo: widgetFeedbackRepo(this._cfg), app: "SuperFeedback" });
      Object.assign(payload.meta, { hostApp: this._cfg.app || "", hostRepo: this._cfg.repo });
    }
    $(".sf-text").value = "";
    this.close();
    this._sendInBackground(payload);
  },

  async _sendInBackground(payload) {
    const host = this._panelHost, cfg = this._cfg;
    const item = { id: globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random()}`, t: Date.now(), attempts: 1, voter: this._voter, ...payload };
    const key = this._key("outbox", cfg), flight = `${key}:${item.id}`;
    this._inFlight ||= new Set();
    this._inFlight.add(flight);
    let result;
    try {
      this._saveOutbox(key, [...this._readOutbox(key), item]);
      result = await this._send(item, cfg);
      if (result.status === 413) dropImages(item, true);
      const current = this._readOutbox(key).filter((x) => x.id !== item.id);
      if (!result.ok) current.push(item);
      this._saveOutbox(key, current);
    } finally { this._inFlight.delete(flight); }
    if (host === this._panelHost) {
      const matched = result.ok && result.idea;
      host?.__toast(matched ? `Matched with “${result.idea.title}” — ${result.idea.votes} people support it. You've been added.` :
        result.ok ? "Thanks! Feedback sent ✓" : "Couldn't send — will retry next launch", result.ok ? "ok" : "err", matched ? 8000 : undefined);
    }
  },

  _readOutbox(key) {
    try { const items = JSON.parse(storageRead(key, "[]")); return Array.isArray(items) ? items.filter((x) => x && typeof x.id === "string" && Number.isFinite(x.t) && Number.isFinite(x.attempts)) : []; }
    catch (_) { return []; }
  },
  _saveOutbox(key, items) {
    items = items.filter((x) => x.attempts <= 5).sort((a, b) => a.t - b.t).slice(-10);
    if (!storageWrite(key, JSON.stringify(items))) {
      items.forEach((item) => dropImages(item));
      return storageWrite(key, JSON.stringify(items));
    }
    return true;
  },
  async _flushOutbox() {
    const cfg = this._cfg, key = this._key("outbox", cfg);
    this._flushing ||= new Set();
    if (this._flushing.has(key)) return;
    this._flushing.add(key);
    try {
      this._saveOutbox(key, this._readOutbox(key).filter((x) => x.attempts < 5));
      const pass = this._readOutbox(key);
      for (const item of pass) {
        if (this._cfg !== cfg) break;
        if (item.attempts >= 5) continue; // Even if pruning exhausted records failed (storage unavailable).
        if (this._inFlight?.has(`${key}:${item.id}`)) continue;
        item.attempts++;
        // Persist the attempt before fetching, including interrupted launches.
        if (!this._saveOutbox(key, this._readOutbox(key).map((x) => x.id === item.id ? item : x))) break;
        const result = await this._send(item, cfg);
        if (result.status === 413) dropImages(item, true);
        const current = this._readOutbox(key).filter((x) => x.id !== item.id);
        if (!result.ok && item.attempts < 5) current.push(item);
        this._saveOutbox(key, current);
        if (!result.ok) break;
      }
    } finally { this._flushing.delete(key); }
  },

  _setHostsVisible(v) {
    for (const h of [this._panelHost, this._triggerHost]) if (h) h.style.visibility = v ? "visible" : "hidden";
  },

  // Fetch the DOM-capture library once at startup (idle time), so the first tap on the
  // button screenshots in milliseconds instead of waiting on a cold CDN fetch.
  _prewarmCapture() {
    const cfg = this._cfg;
    if (typeof cfg.captureScreenshot === "function" || cfg.captureModule) return;
    const warm = () => { if (this._cfg !== cfg) return; this._captureModule ||= import(/* @vite-ignore */ CAPTURE_CDN).catch(() => { this._captureModule = null; }); };
    if (typeof requestIdleCallback === "function") requestIdleCallback(warm, { timeout: 3000 }); else setTimeout(warm, 500);
  },

  async _capture() {
    const cfg = this._cfg;
    if (typeof cfg.captureScreenshot === "function") return await cfg.captureScreenshot(cfg);
    const mod = await (cfg.captureModule || (this._captureModule ||= import(/* @vite-ignore */ CAPTURE_CDN).catch((e) => { this._captureModule = null; throw e; })));
    try { return await captureViewport(mod, false); }
    catch (error) {
      // Fonts and images are what usually fail; a plainer picture beats no picture.
      try { return await captureViewport(mod, true); } catch (_) { throw error; }
    }
  },

  _meta(screenshot = "capture failed: no capture available") {
    const cfg = this._cfg || {}, meta = {};
    const read = (key, get) => { try { const value = get(); if (value !== undefined) meta[key] = value; } catch (_) {} };
    read("url", () => location.href);
    read("platform", () => navigator.platform);
    read("userAgent", () => navigator.userAgent);
    read("locale", () => navigator.language);
    read("viewport", () => `${window.innerWidth}x${window.innerHeight}`);
    Object.assign(meta, { widget: `web/${SuperFeedback.version}`, os: "web" },
      cfg.appVersion ? { appVersion: cfg.appVersion } : {}, cfg.meta || {}, this._context);
    read("uptime", () => Math.max(0, Math.round((Date.now() - this._startedAt) / 1000)));
    read("timezone", () => Intl.DateTimeFormat().resolvedOptions().timeZone);
    read("colorScheme", () => matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    read("screen", () => `${window.screen.width}x${window.screen.height} @${window.devicePixelRatio || 1}x`);
    read("network", () => navigator.onLine ? "online" : "offline");
    read("network", () => navigator.connection?.effectiveType ? `${meta.network} (${navigator.connection.effectiveType})` : meta.network);
    read("memoryMB", () => performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : undefined);
    read("reduceMotion", () => matchMedia("(prefers-reduced-motion: reduce)").matches);
    read("sessionId", () => SESSION_ID);
    read("lastMoment", () => storageRead("superfeedback:lastMoment"));
    read("screenshot", () => screenshot);
    return meta;
  },
  async _send({ repo, app, message, type, screenshot, images, logs, meta, voter = this._voter }, cfg = this._cfg) {
    try {
      const res = await fetch(cfg.backendUrl.replace(/\/$/, "") + "/report", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ repo: repo || cfg.repo, voter, app: app ?? (cfg.app || ""), type, message,
          screenshot: screenshot || undefined, images: images?.length ? images : undefined,
          logs: cfg.captureLogs && logs?.length ? logs : undefined, appKey: cfg.appKey || undefined, meta: meta || this._meta() }),
      });
      let body; try { body = await res.json(); } catch (_) {}
      return { ok: res.ok && body?.ok === true, status: res.status, idea: body?.idea };
    } catch (_) { return { ok: false, status: 0 }; }
  },

  _installLogCapture() {
    const patch = (target, key, wrap) => {
      try {
        const original = target[key], wrapper = wrap(original);
        target[key] = wrapper;
        this._cleanups.push(() => { if (target[key] === wrapper) target[key] = original; });
      } catch (_) {}
    };
    const record = (level, text, source) => { try { this._pushLog(level, text, source); } catch (_) {} };
    for (const lvl of ["error", "warn", "info", "log"]) {
      patch(console, lvl, (original) => (...args) => {
        record(lvl, args.map(safeStr).join(" "), "console");
        return original?.apply(console, args);
      });
    }
    this._listen(window, "error", (e) => {
      if (!isWidgetEvent(e)) record("error", `${e.message} @ ${e.filename}:${e.lineno}:${e.colno} ${safeStr(e.error?.stack || "")}`, "error");
    });
    this._listen(window, "unhandledrejection", (e) => {
      if (!isWidgetEvent(e)) record("error", "Unhandled rejection: " + safeStr(e.reason), "error");
    });
    const base = this._cfg.backendUrl.replace(/\/$/, "");
    const backend = networkURL(base + "/report"), checkin = networkURL(base + "/checkin");
    const requestInfo = (input, init) => {
      try { return { method: String(init?.method || input?.method || "GET").toUpperCase(), url: networkURL(input?.url || input) }; }
      catch (_) { return { method: "GET", url: "unknown" }; }
    };
    const net = (info, result) => {
      if (info && !(info.method === "POST" && (info.url === backend || info.url === checkin))) record("error", `${info.method} ${info.url} ${result}`, "net");
    };
    patch(window, "fetch", (original) => async function (...args) {
      const info = requestInfo(...args);
      try {
        const response = await original.apply(this, args);
        if (response.status >= 400) net(info, response.status);
        return response;
      } catch (error) { net(info, `network error: ${safeStr(error?.message || error)}`); throw error; }
    });
    if (typeof XMLHttpRequest !== "undefined") {
      const requests = new WeakMap();
      patch(XMLHttpRequest.prototype, "open", (original) => function (method, url, ...args) {
        const result = original.call(this, method, url, ...args);
        requests.set(this, requestInfo(url, { method }));
        return result;
      });
      const pending = new Set();
      this._cleanups.push(() => { for (const remove of pending) remove(); pending.clear(); });
      patch(XMLHttpRequest.prototype, "send", (original) => function (...args) {
        const xhr = this, info = requests.get(xhr);
        const events = ["error", "timeout", "abort", "loadend"];
        let finished = false;
        const remove = () => { for (const event of events) xhr.removeEventListener(event, done); pending.delete(remove); };
        const done = (event) => {
          if (finished) return;
          finished = true; remove();
          if (event.type !== "loadend") net(info, event.type === "error" ? "network error" : event.type);
          else if (xhr.status >= 400 || xhr.status === 0) net(info, xhr.status || "network error");
        };
        pending.add(remove); for (const event of events) xhr.addEventListener(event, done);
        try { return original.apply(xhr, args); }
        catch (error) { remove(); throw error; }
      });
    }
    const nav = (event) => {
      if (!event || !isWidgetEvent(event)) record("info", location.href, "nav");
    };
    for (const key of ["pushState", "replaceState"]) patch(history, key, (original) => function (...args) {
      const result = original.apply(this, args); nav(); return result;
    });
    for (const event of ["popstate", "hashchange"]) this._listen(window, event, nav);
    this._listen(document, "click", (event) => {
      if (isWidgetEvent(event)) return;
      const element = event.composedPath().find((node) => node.matches?.('button,a,input,select,textarea,[role="button"],[role="menuitem"],[role="tab"],summary,label'));
      if (!element) return;
      // Form controls and editable descendants never contribute their text/value.
      const clone = element.cloneNode(true);
      clone.querySelectorAll('input,select,textarea,[contenteditable]').forEach((node) => node.remove());
      const text = element.matches('input,select,textarea') || element.isContentEditable ? "" : clone.textContent;
      const label = element.getAttribute("aria-label") || text?.trim() || element.getAttribute("name") || element.id || "";
      record("info", `${element.tagName.toLowerCase()} ${label.replace(/\s+/g, " ").slice(0, 40)}`.trim(), "ui");
    }, true);
  },

  _pushLog(level, text, source = "app") {
    if (!this._cfg?.captureLogs) return;
    if (!this._logs) this._logs = [];
    this._logs.push({ t: Date.now(), level: safeStr(level).replace(/\s+/g, " ").slice(0, 20), source,
      text: safeStr(text).replace(/[\r\n]+/g, " ").slice(0, 2000) });
    if (this._logs.length > 200) this._logs.splice(0, this._logs.length - 200);
  },

  _serializeLogs() {
    if (!this._cfg?.captureLogs || !this._logs?.length) return undefined;
    const configured = this._cfg?.maxLogs;
    const max = Number.isFinite(configured) ? fit(1, 200, Math.floor(configured)) : 200;
    const lines = this._logs.slice(-max).map((e) => {
      const line = `${new Date(e.t).toISOString().slice(11, 23)} ${e.level.toUpperCase()} [${e.source}] ${e.text}`;
      return e.source === "console" ? line.slice(0, 500) : line;
    });
    // Bound the actual UTF-8 JSON array, including escaping and separators.
    const encoder = new TextEncoder();
    while (lines.length && encoder.encode(JSON.stringify(lines)).length > 40000) lines.shift();
    return lines;
  },

  // Crash capture: an unhandled error is saved to localStorage, then auto-sent on the NEXT
  // launch (the app may be too broken to send right now). See ../../docs/crash-reports.md.
  _installCrashCapture() {
    const persist = (event, msg) => { if (!isWidgetEvent(event)) this._persistCrash(msg); };
    this._listen(window, "error", (e) => persist(e, `${e.message} @ ${e.filename}:${e.lineno}` + (e.error && e.error.stack ? "\n" + e.error.stack : "")));
    this._listen(window, "unhandledrejection", (e) => persist(e, "Unhandled rejection: " + safeStr(e.reason && e.reason.stack ? e.reason.stack : e.reason)));
  },

  _crashKey() { return this._key("pending"); },

  _persistCrash(message) {
    try {
      const key = this._crashKey();
      const arr = JSON.parse(localStorage.getItem(key) || "[]");
      arr.push({ t: Date.now(), message: String(message).slice(0, 800), logs: this._serializeLogs() || [] });
      localStorage.setItem(key, JSON.stringify(arr.slice(-5)));
    } catch (_) {}
  },

  async _flushPendingCrashes() {
    const cfg = this._cfg, key = this._crashKey();
    let arr;
    try { arr = JSON.parse(storageRead(key, "[]")); } catch (_) { await this._flushOutbox(); return; }
    if (!Array.isArray(arr) || !arr.length) { await this._flushOutbox(); return; }
    // Transfer pending crashes to the durable outbox before removing their backup.
    const outboxKey = this._key("outbox", cfg);
    const items = arr.map((c, i) => ({
      id: `crash-${c.t}-${i}`, t: c.t, attempts: 0, type: "crash",
      message: "App error recovered from a previous session:\n" + c.message,
      logs: c.logs, meta: this._meta(),
    }));
    const current = this._readOutbox(outboxKey);
    if (!this._saveOutbox(outboxKey, [...current, ...items.filter((c) => !current.some((x) => x.id === c.id))])) { await this._flushOutbox(); return; }
    try { localStorage.removeItem(key); } catch (_) {}
    await this._flushOutbox();
  },

};

function isWidgetHost(node) { return node?.getAttributeNames?.().some((name) => name.startsWith("data-superfeedback-")) || false; }

const PIXEL = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
// XML 1.0 forbids C0 controls other than tab/LF/CR, and U+FFFE/U+FFFF. One in any text node or
// input value makes the serialized SVG malformed, and the browser refuses the whole image.
const XML_ILLEGAL = /%(?:0[0-8BCEF]|1[0-9A-F]|EF%BF%B[EF])/gi;

function pageBackground() {
  for (const el of [document.documentElement, document.body]) {
    const color = el && getComputedStyle(el).backgroundColor;
    if (color && color !== "transparent" && !/^rgba\(.*,\s*0\)$/.test(color)) return color;
  }
  return "#fff";
}

// What the user sees: the current viewport at the current scroll position. html-to-image
// alone renders the top of the document, rejects the whole capture when one <img> fails
// to decode, and emits invalid XML for control characters — each seen in real installs.
async function captureViewport(mod, plain) {
  const root = document.documentElement;
  const width = root.clientWidth || window.innerWidth, height = root.clientHeight || window.innerHeight;
  const ratio = Math.min(window.devicePixelRatio || 1, 2);
  const svg = await mod.toSvg(root, {
    width, height, cacheBust: plain, skipFonts: plain, imagePlaceholder: PIXEL,
    // A negative margin keeps position:fixed elements where they are on screen (a transform would move them).
    style: { margin: `${-window.scrollY}px 0 0 ${-window.scrollX}px`, width: `${root.scrollWidth}px`, height: "auto" },
    filter: (node) => !isWidgetHost(node) && !(node instanceof HTMLImageElement && (plain || (node.complete && !node.naturalWidth)))
      && !(plain && (node instanceof HTMLVideoElement || node instanceof HTMLIFrameElement)),
  });
  const img = new Image();
  img.src = svg.replace(XML_ILLEGAL, "");
  await img.decode();
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(width * ratio); canvas.height = Math.round(height * ratio);
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = pageBackground();
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL("image/png");
}
function isWidgetEvent(event) { return event.composedPath().some(isWidgetHost); }
function networkURL(value) {
  try { const url = new URL(value, location.href); url.search = ""; url.hash = ""; url.username = ""; url.password = ""; return url.href; }
  catch (_) { return "unknown URL"; }
}

const checkinFlights = new Set(), checkinStamps = new Map();
function momentNames() {
  try { const names = JSON.parse(storageRead("superfeedback:momentNames", "[]")); return Array.isArray(names) ? names.slice(-20) : []; }
  catch (_) { return []; }
}
function rememberMoment(name) {
  const trimmed = name.trim().slice(0, 40), names = momentNames();
  if (!names.includes(trimmed)) storageWrite("superfeedback:momentNames", JSON.stringify([...names, trimmed].slice(-20)));
}
function webOS() {
  try {
    const ua = navigator.userAgent || "";
    let m;
    if ((m = ua.match(/(?:iPhone|CPU) OS (\d+)[._](\d+)/))) return `iOS ${m[1]}.${m[2]}`;
    if ((m = ua.match(/Android (\d+(?:\.\d+)?)/))) return `Android ${m[1]}`;
    if (/Windows/.test(ua)) return "Windows";
    if (/Macintosh|Mac OS X/.test(ua)) return "macOS";
    if (/CrOS/.test(ua)) return "ChromeOS";
    if (/Linux/.test(ua)) return "Linux";
  } catch (_) {}
  return "web";
}
function storageRead(key, fallback) { try { return localStorage.getItem(key) ?? fallback; } catch (_) { return fallback; } }
function storageWrite(key, value) { try { localStorage.setItem(key, value); return true; } catch (_) { return false; } }
function dropImages(item, tooLarge = false) {
  if (item.screenshot || item.images?.length) {
    item.meta = { ...item.meta, screenshot: tooLarge ? "dropped: too large" : "dropped: storage full" };
  }
  delete item.screenshot; delete item.images;
  const suffix = " (screenshot dropped: too large)";
  if (tooLarge && !item.message.endsWith(suffix)) item.message += suffix;
}
function fit(lo, hi, value) { return Math.max(lo, Math.min(Math.max(lo, hi), value)); }

// AshCut's header drag: compose offsets with the existing centred scale/fade.
function installPanelDrag(modal, head, onResize) {
  let dx = 0, dy = 0, drag = null;
  const apply = () => { modal.style.setProperty("--sf-dx", `${dx}px`); modal.style.setProperty("--sf-dy", `${dy}px`); };
  const geom = () => { const r = modal.getBoundingClientRect(); return { left: r.left - dx, top: r.top - dy, w: r.width }; };
  const clamp = (g, x, y) => [fit(-(g.left + g.w - Math.min(g.w, 140)), innerWidth - Math.min(g.w, 140) - g.left, x), fit(-g.top, innerHeight - 44 - g.top, y)];
  const end = () => {
    if (drag && head.hasPointerCapture(drag.id)) head.releasePointerCapture(drag.id);
    drag = null; modal.classList.remove("sf-dragging");
  };
  head.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !e.isPrimary || innerWidth <= 520 || drag) return;
    if (e.composedPath().slice(0, e.composedPath().indexOf(head)).some((n) => n.matches?.("button,input,textarea,select,label,a"))) return;
    drag = { id: e.pointerId, sx: e.clientX, sy: e.clientY, ox: dx, oy: dy, g: geom() };
    modal.classList.add("sf-dragging"); head.setPointerCapture(e.pointerId); e.preventDefault();
  });
  head.addEventListener("pointermove", (e) => {
    if (!drag || drag.id !== e.pointerId) return;
    [dx, dy] = clamp(drag.g, drag.ox + e.clientX - drag.sx, drag.oy + e.clientY - drag.sy); apply();
  });
  for (const type of ["pointerup", "pointercancel", "lostpointercapture"]) head.addEventListener(type, (e) => { if (drag?.id === e.pointerId) end(); });
  onResize(() => {
    end();
    if (innerWidth <= 520) dx = dy = 0;
    else [dx, dy] = clamp(geom(), dx, dy);
    apply();
  });
  return () => {
    end(); dx = dy = 0; modal.classList.add("sf-dragging"); apply();
    void modal.offsetWidth; modal.classList.remove("sf-dragging");
  };
}

// Markup editor. Lives in the panel's own shadow root, above the modal, and owns every
// pointer while it is up. Shapes stay normalised (0…1 of the image) so the same drawing
// renders at any display scale and composites cleanly at the capture's native size.
function installMarkup(root, host, onCommit) {
  const $ = (s) => root.querySelector(s);
  const layer = $(".sf-markup-layer"), stage = $(".sf-mk-stage"), canvas = $(".sf-mk-canvas");
  if (!layer) return null;
  const modal = $(".sf-modal"), ctx = canvas.getContext("2d");
  let image = null, source = null, shapes = [], draw = null, frame = 0, opened = false;
  let tool = "pen", color = MARKUP_COLORS[0].v;

  const select = (selector, value, attribute) =>
    root.querySelectorAll(selector).forEach((b) => b.classList.toggle("sf-active", b.dataset[attribute] === value));
  const setTool = (v) => { tool = v; select(".sf-mk-tool", v, "tool"); };
  const setColor = (v) => { color = v; select(".sf-mk-swatch", v, "color"); };

  const live = () => (draw ? shapes.concat([draw.shape]) : shapes);
  const paint = () => {
    frame = 0;
    if (!image || !canvas.width) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    paintShapes(ctx, live(), canvas.width, canvas.height, image.naturalWidth);
  };
  const schedule = () => { if (!frame) frame = requestAnimationFrame(paint); };
  const layout = () => {
    if (!image || !opened) return;
    const box = stage.getBoundingClientRect();
    const scale = Math.min(box.width / image.naturalWidth, box.height / image.naturalHeight) || 1;
    const w = Math.max(1, Math.round(image.naturalWidth * scale)), h = Math.max(1, Math.round(image.naturalHeight * scale));
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    canvas.style.width = `${w}px`; canvas.style.height = `${h}px`;
    canvas.width = Math.max(1, Math.round(w * dpr)); canvas.height = Math.max(1, Math.round(h * dpr));
    paint();
  };
  const syncBar = () => {
    const empty = !shapes.length && !draw;
    root.querySelectorAll(".sf-mk-undo, .sf-mk-clear").forEach((b) => { b.disabled = empty; });
    $(".sf-mk-count").textContent = shapes.length ? `${shapes.length} shape${shapes.length === 1 ? "" : "s"}` : "";
  };

  const at = (e) => {
    const r = canvas.getBoundingClientRect();
    return { x: fit(0, 1, (e.clientX - r.left) / (r.width || 1)), y: fit(0, 1, (e.clientY - r.top) / (r.height || 1)) };
  };
  canvas.addEventListener("pointerdown", (e) => {
    if (!image || draw || !e.isPrimary || (e.pointerType === "mouse" && e.button !== 0)) return;
    const p = at(e);
    draw = { id: e.pointerId, sx: e.clientX, sy: e.clientY, moved: false,
      shape: { tool, color, points: tool === "pen" ? [p] : [p, p] } };
    try { canvas.setPointerCapture(e.pointerId); } catch (_) {}
    e.preventDefault();
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!draw || draw.id !== e.pointerId) return;
    e.preventDefault();
    if (Math.hypot(e.clientX - draw.sx, e.clientY - draw.sy) > 3) draw.moved = true;
    if (!draw.moved) return;
    const p = at(e), points = draw.shape.points;
    // A tap that never moves draws nothing; a long freehand stroke stays bounded.
    if (draw.shape.tool !== "pen") points[1] = p;
    else if (points.length < 4000) points.push(p);
    else points[points.length - 1] = p;
    schedule();
  });
  const end = (e, commit) => {
    if (!draw || draw.id !== e.pointerId) return;
    const pending = draw;
    draw = null;
    if (canvas.hasPointerCapture?.(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
    if (commit && pending.moved) shapes.push(pending.shape);
    syncBar(); schedule();
  };
  canvas.addEventListener("pointerup", (e) => end(e, true));
  for (const type of ["pointercancel", "lostpointercapture"]) canvas.addEventListener(type, (e) => end(e, false));

  root.querySelectorAll(".sf-mk-tool").forEach((b) => b.addEventListener("click", () => setTool(b.dataset.tool)));
  root.querySelectorAll(".sf-mk-swatch").forEach((b) => {
    // CSSOM, not a style attribute: inline attributes are blocked under a nonce-only style-src.
    b.style.background = b.dataset.color;
    b.addEventListener("click", () => setColor(b.dataset.color));
  });
  $(".sf-mk-undo").addEventListener("click", () => { shapes.pop(); syncBar(); schedule(); });
  $(".sf-mk-clear").addEventListener("click", () => { shapes = []; syncBar(); schedule(); });
  $(".sf-mk-cancel").addEventListener("click", () => api.close(false));
  $(".sf-mk-done").addEventListener("click", () => api.close(true));
  // The scrim swallows clicks so the panel underneath stays untouched; it never discards work.
  layer.addEventListener("pointerdown", (e) => { if (e.target === layer || e.target === stage) e.preventDefault(); });

  const onResize = () => layout();
  const api = {
    element: () => layer,
    isOpen: () => opened,
    async open() {
      if (opened || !host.__originalShot) return;
      try {
        if (source !== host.__originalShot) { image = await loadImage(host.__originalShot); source = host.__originalShot; }
      } catch (_) { image = null; source = null; host.__toast?.("Couldn't open the screenshot", "err"); return; }
      if (!host.__isOpen?.()) return;
      shapes = (host.__shapes || []).map(cloneShape);
      draw = null; opened = true;
      setTool(tool); setColor(color); syncBar();
      layer.hidden = false; layer.setAttribute("aria-hidden", "false");
      modal.inert = true;
      window.addEventListener("resize", onResize);
      layout();
      setTimeout(() => { if (opened) $(".sf-mk-done").focus({ preventScroll: true }); }, 30);
    },
    close(commit) {
      if (!opened) return;
      opened = false; draw = null;
      if (frame) { cancelAnimationFrame(frame); frame = 0; }
      window.removeEventListener("resize", onResize);
      layer.hidden = true; layer.setAttribute("aria-hidden", "true");
      modal.inert = false;
      const committed = shapes.map(cloneShape);
      shapes = [];
      if (commit) onCommit(committed);
      const back = root.querySelector(".sf-mk-open") || root.querySelector(".sf-shot-btn");
      if (back?.isConnected && host.__isOpen?.()) back.focus({ preventScroll: true });
    },
  };
  return api;
}

function cloneShape(s) { return { tool: s.tool, color: s.color, points: s.points.map((p) => ({ x: p.x, y: p.y })) }; }

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("image load failed"));
    img.src = src;
  });
}

// Shapes are normalised, so the same routine paints the on-screen preview and the
// native-size export; the stroke is defined in image pixels and scaled to the target.
function paintShapes(ctx, shapes, pxW, pxH, imageWidth) {
  const width = Math.max(3, imageWidth / 300) * (pxW / (imageWidth || pxW));
  ctx.save();
  ctx.lineCap = "round"; ctx.lineJoin = "round"; ctx.lineWidth = width;
  for (const shape of shapes) {
    const p = (shape.points || []).map((q) => [q.x * pxW, q.y * pxH]);
    if (!p.length) continue;
    ctx.strokeStyle = ctx.fillStyle = shape.color || MARKUP_COLORS[0].v;
    const a = p[0], b = p[p.length - 1];
    ctx.beginPath();
    if (shape.tool === "pen") {
      ctx.moveTo(a[0], a[1]);
      for (let i = 1; i < p.length; i++) ctx.lineTo(p[i][0], p[i][1]);
      if (p.length === 1) ctx.lineTo(a[0] + .01, a[1]);
      ctx.stroke();
    } else if (shape.tool === "rect") {
      ctx.rect(Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.abs(b[0] - a[0]), Math.abs(b[1] - a[1]));
      ctx.stroke();
    } else if (shape.tool === "arrow") {
      paintArrow(ctx, a, b, width);
    } else {
      ctx.ellipse((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, Math.abs(b[0] - a[0]) / 2, Math.abs(b[1] - a[1]) / 2, 0, 0, Math.PI * 2);
      ctx.stroke();
    }
  }
  ctx.restore();
}

function paintArrow(ctx, a, b, width) {
  const angle = Math.atan2(b[1] - a[1], b[0] - a[0]);
  const head = Math.min(Math.max(width * 3.4, 8), Math.hypot(b[0] - a[0], b[1] - a[1]) || 1);
  const baseX = b[0] - Math.cos(angle) * head, baseY = b[1] - Math.sin(angle) * head;
  ctx.moveTo(a[0], a[1]); ctx.lineTo(baseX, baseY); ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(b[0], b[1]);
  ctx.lineTo(baseX - Math.sin(angle) * head * .45, baseY + Math.cos(angle) * head * .45);
  ctx.lineTo(baseX + Math.sin(angle) * head * .45, baseY - Math.cos(angle) * head * .45);
  ctx.closePath(); ctx.fill();
}

// Flatten the shapes onto the capture at its native size. Oversized results shrink by
// 0.75 (up to five times), the same ceiling the backend accepts for a capture.
async function compositeMarkup(source, shapes) {
  const image = await loadImage(source);
  let scale = 1, url = source;
  for (let attempt = 0; attempt < 6; attempt++) {
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const ctx = canvas.getContext("2d");
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    paintShapes(ctx, shapes, canvas.width, canvas.height, image.naturalWidth);
    url = canvas.toDataURL("image/png");
    if (dataURLBytes(url) <= MAX_SHOT_BYTES) break;
    scale *= 0.75;
  }
  return url;
}

function dataURLBytes(url) {
  const base64 = String(url).slice(String(url).indexOf(",") + 1);
  return Math.ceil(base64.length * 3 / 4);
}

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
      const type = "image/jpeg";
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

function widgetFeedbackRepo(cfg) {
  const repo = cfg.widgetFeedback === true ? WIDGET_FEEDBACK_REPO : cfg.widgetFeedback;
  return typeof repo === "string" && repo.includes("/") ? repo.trim() : null;
}

function PANEL_TEMPLATE(cfg) {
  const accent = cfg.color || "#6d5efc";
  const seg = TYPES.map((t) =>
    `<button class="sf-seg-btn${t.v === cfg.type ? " sf-active" : ""}" type="button" data-type="${t.v}"><span>${t.emoji}</span>${t.label}</button>`).join("");
  const markup = cfg.markup !== false;
  const shotImg = `<img class="sf-shot-preview" alt="Captured screenshot" />`;
  const preview = markup
    ? `<button class="sf-shot-btn sf-markup-open" type="button" aria-label="Mark up the screenshot">${shotImg}</button>` : shotImg;
  const markupButton = markup
    ? `<button class="sf-mk-open sf-markup-open" type="button">${PEN_ICON}<span class="sf-mk-open-label">Mark up</span></button>` : "";
  return `
  <style nonce="${esc(cfg.styleNonce || "")}">
    ${TOKENS}
    *, *::before, *::after { box-sizing: border-box; }
    [hidden] { display: none !important; }
    .sf-community-view { max-height: min(70vh, 560px); overflow-y: auto; overscroll-behavior: contain; }
    .sf-community-view p { line-height: 1.5; }
    .sf-community-view .sf-h { margin-bottom: 12px; }
    .sf-suggest, .sf-support-pay { width: 100%; margin: 12px 0; }
    .sf-idea { display: flex; align-items: center; gap: 12px; padding: 12px 0; border-bottom: 1px solid var(--sf-field-bd); }
    .sf-idea-copy { flex: 1; min-width: 0; }
    .sf-idea-title { font-weight: 600; font-size: 14px; overflow-wrap: anywhere; }
    .sf-summary { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--sf-muted); font-size: 12px; margin: 5px 0; }
    .sf-status, .sf-supporter { display: inline-block; border-radius: 999px; padding: 3px 6px; background: var(--sf-seg); font-size: 10px; }
    .sf-vote { flex: 0 0 auto; min-width: 44px; border: 1px solid var(--sf-field-bd); border-radius: 10px; padding: 5px 9px; background: var(--sf-field); color: var(--sf-fg); cursor: pointer; font: inherit; font-size: 13px; }
    .sf-vote.sf-active { background: ${accent}; color: #fff; }
    .sf-meter { height: 12px; border-radius: 999px; overflow: hidden; background: var(--sf-seg); margin: 8px 0; }
    .sf-meter-fill { height: 100%; background: ${accent}; border-radius: inherit; }
    .sf-thanks { font-size: 13px; }
    @media (prefers-reduced-motion: reduce) { *, *::before, *::after { animation: none !important; transition: none !important; } }
    .sf-backdrop { position: fixed; inset: 0; z-index: 2147483000; background: rgba(15,12,30,.18); opacity: 0; pointer-events: none; transition: opacity .22s ease; }
    .sf-backdrop.sf-show { opacity: 1; pointer-events: auto; }
    .sf-modal { position: fixed; left: 50%; top: 50%; z-index: 2147483001; width: 360px; max-width: calc(100vw - 32px);
      color: var(--sf-fg); font-family: ${FONT}; background: var(--sf-bg); border: 1px solid var(--sf-border);
      border-radius: 22px; box-shadow: var(--sf-shadow); padding: 20px;
      backdrop-filter: blur(28px) saturate(180%); -webkit-backdrop-filter: blur(28px) saturate(180%);
      opacity: 0; pointer-events: none; transform: translate(calc(-50% + var(--sf-dx, 0px)),calc(-50% + var(--sf-dy, 0px))) scale(.94);
      transition: opacity .24s ease, transform .26s cubic-bezier(.2,.9,.25,1); }
    .sf-modal.sf-show { opacity: 1; pointer-events: auto; transform: translate(calc(-50% + var(--sf-dx, 0px)),calc(-50% + var(--sf-dy, 0px))) scale(1); }
    .sf-modal.sf-dragging { transition: none; }
    @media (min-width: 521px) { .sf-head { cursor: move; user-select: none; touch-action: none; } .sf-head:hover { background: var(--sf-hover); } }
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
    .sf-caption { margin: 6px 2px 0; color: var(--sf-muted); font-size: 12px; line-height: 1.4; }
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
    .sf-row[hidden] { display: none; }
    .sf-shot-preview { display: block; max-height: 64px; max-width: 80px; border-radius: 8px; object-fit: contain; }
    .sf-shot-btn { padding: 2px; border: 1px solid var(--sf-field-bd); border-radius: 10px; background: transparent; cursor: pointer;
      line-height: 0; transition: border-color .15s; }
    .sf-shot-btn:hover { border-color: ${accent}; }
    .sf-shot-label { display: flex; align-items: center; gap: 9px; flex: 1 1 auto; cursor: pointer; }
    .sf-mk-open { display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--sf-field-bd); background: var(--sf-field);
      color: var(--sf-fg); border-radius: 11px; padding: 7px 10px; font-family: inherit; font-size: 12.5px; font-weight: 600;
      cursor: pointer; white-space: nowrap; transition: border-color .15s; }
    .sf-mk-open:hover { border-color: ${accent}; }
    .sf-mk-open svg { width: 14px; height: 14px; }
    .sf-markup-layer { position: fixed; inset: 0; z-index: 2147483003; display: none; flex-direction: column;
      background: rgba(10,8,20,.92); color: #fff; font-family: ${FONT}; -webkit-tap-highlight-color: transparent; }
    .sf-markup-layer:not([hidden]) { display: flex; }
    .sf-mk-stage { position: relative; flex: 1 1 auto; min-height: 0; margin: 14px 14px 0; }
    .sf-mk-canvas { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); touch-action: none;
      cursor: crosshair; border-radius: 10px; background: #fff; box-shadow: 0 18px 60px rgba(0,0,0,.6); }
    .sf-mk-bar { display: flex; flex-wrap: wrap; align-items: center; justify-content: center; gap: 8px;
      padding: 12px 12px calc(14px + env(safe-area-inset-bottom)); }
    .sf-mk-group { display: flex; align-items: center; gap: 4px; padding: 4px; border-radius: 14px; background: rgba(255,255,255,.12); }
    .sf-mk-btn { display: inline-flex; align-items: center; gap: 6px; border: none; background: transparent; color: #fff;
      font-family: inherit; font-size: 13px; font-weight: 600; padding: 8px 10px; border-radius: 10px; cursor: pointer;
      transition: background .15s, color .15s; }
    .sf-mk-btn:hover:not(:disabled) { background: rgba(255,255,255,.16); }
    .sf-mk-btn:disabled { opacity: .4; cursor: default; }
    .sf-mk-btn.sf-active { background: #fff; color: #15131f; }
    .sf-mk-btn svg { width: 16px; height: 16px; }
    .sf-mk-swatch { width: 26px; height: 26px; padding: 0; border-radius: 50%; border: 2px solid transparent; cursor: pointer; }
    .sf-mk-swatch.sf-active { border-color: #fff; box-shadow: 0 0 0 2px rgba(0,0,0,.45); }
    .sf-mk-count { font-size: 12px; font-weight: 600; color: rgba(255,255,255,.7); min-width: 54px; text-align: center; }
    .sf-mk-done { background: ${accent}; color: #fff; box-shadow: 0 6px 18px ${hexA(accent, .5)}; }
    .sf-mk-done:hover:not(:disabled) { background: ${accent}; filter: brightness(1.08); }
    .sf-send:disabled { opacity: .45; cursor: default; }
    .sf-widget-fb { display: block; margin: 12px auto 0; border: none; background: transparent; color: var(--sf-muted);
      font-family: inherit; font-size: 11.5px; padding: 2px 6px; cursor: pointer; }
    .sf-widget-fb:hover { color: var(--sf-fg); text-decoration: underline; }
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
    .sf-toast.sf-err { background: rgba(196,40,52,.9); }
    .sf-nudge { position: fixed; right: 20px; bottom: 88px; z-index: 2147482998; display: flex; flex-wrap: wrap; align-items: center; gap: 10px;
      color: var(--sf-fg); font-family: ${FONT}; font-size: 13.5px; font-weight: 500; padding: 11px 12px 11px 16px; border-radius: 16px;
      background: var(--sf-bg); border: 1px solid var(--sf-border); box-shadow: var(--sf-shadow); max-width: 300px;
      backdrop-filter: blur(22px) saturate(180%); -webkit-backdrop-filter: blur(22px) saturate(180%);
      opacity: 0; pointer-events: none; transform: translateY(12px) scale(.96); transition: opacity .25s ease, transform .25s cubic-bezier(.2,.9,.25,1); }
    .sf-nudge.sf-show { opacity: 1; pointer-events: auto; transform: translateY(0) scale(1); }
    .sf-nudge-msg { flex: 1; min-width: 0; overflow-wrap: anywhere; }
    .sf-nudge-links { flex-basis: 100%; color: var(--sf-muted); font-size: 11px; }
    .sf-nudge-links button { border: none; background: transparent; color: var(--sf-muted); font: inherit; padding: 2px 0; cursor: pointer; }
    .sf-nudge-links button:hover { text-decoration: underline; }
    .sf-nudge-links .sf-nudge-optout { font-size: 10px; }
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
  <div class="sf-modal" inert aria-hidden="true" role="dialog" aria-modal="true" aria-label="Send feedback">
    <div class="sf-grabber"></div>
    <div class="sf-seg sf-tabs" role="tablist" aria-label="SuperFeedback">
      <button type="button" class="sf-seg-btn" role="tab" id="sf-tab-feedback" aria-controls="sf-view-feedback" data-tab="feedback">Feedback</button>
      <button type="button" class="sf-seg-btn" role="tab" id="sf-tab-ideas" aria-controls="sf-view-ideas" data-tab="ideas">Ideas</button>
      <button type="button" class="sf-seg-btn" role="tab" id="sf-tab-support" aria-controls="sf-view-support" data-tab="support">Support <span class="sf-supporter" hidden>Supporter</span></button>
    </div>
    <div data-view="feedback" id="sf-view-feedback" role="tabpanel" aria-labelledby="sf-tab-feedback">
    <div class="sf-head" title="Drag to move"><h2 class="sf-h sf-feedback-title">Send feedback</h2><span class="sf-sub sf-feedback-sub">${cfg.app ? esc(cfg.app) : ""}</span></div>
    <div class="sf-seg" role="group" aria-label="Feedback type">${seg}</div>
    <textarea class="sf-text" placeholder="What went wrong, or what would you like?"></textarea>
    <p class="sf-caption">Includes a screenshot${cfg.captureLogs ? " and recent app logs" : ""}</p>
    <div class="sf-attach">
      <button class="sf-addimg" type="button">${IMG_ICON} Add image</button>
      <div class="sf-thumbs"></div>
    </div>
    <input type="file" class="sf-file" accept="image/*" multiple hidden />
    <div class="sf-row" hidden>
      ${preview}
      <label class="sf-shot-label">
        <span class="sf-switch"><input type="checkbox" class="sf-shot"${cfg.attachScreenshot ? " checked" : ""}/><span class="sf-slider"></span></span>
        Attach screenshot
      </label>
      ${markupButton}
    </div>
    <div class="sf-actions">
      <button class="sf-cancel" type="button">Cancel</button>
      <button class="sf-send" type="button" disabled>Send feedback</button>
    </div>
    ${widgetFeedbackRepo(cfg) ? `<button class="sf-widget-fb" type="button">Feedback on SuperFeedback</button>` : ""}
    </div>
    <div data-view="ideas" id="sf-view-ideas" role="tabpanel" aria-labelledby="sf-tab-ideas" class="sf-community-view" hidden>
      <h2 class="sf-h">Ideas &amp; roadmap</h2>
      <button type="button" class="sf-cancel sf-suggest">+ Suggest an idea</button>
      <div class="sf-seg" role="group" aria-label="Filter ideas">
        <button type="button" class="sf-seg-btn" data-filter="top">Top</button>
        <button type="button" class="sf-seg-btn" data-filter="new">New</button>
        <button type="button" class="sf-seg-btn" data-filter="shipped">Shipped</button>
      </div>
      <p class="sf-caption sf-updated"></p>
      <div class="sf-ideas-list"></div>
      <p class="sf-caption sf-you"></p>
    </div>
    <div data-view="support" id="sf-view-support" role="tabpanel" aria-labelledby="sf-tab-support" class="sf-community-view" hidden>
      <h2 class="sf-h">This app is free.</h2>
      <p>If it's useful to you, optional contributions help pay for the AI tools, infrastructure and development that keep improving it. Supporting doesn't unlock anything — every improvement ships to everyone.</p>
      <div class="sf-fund" hidden>
        <p class="sf-caption sf-fund-caption"></p>
        <p class="sf-fund-money"></p>
        <div class="sf-meter" role="progressbar" aria-label="Community development fund" aria-valuemin="0" aria-valuemax="100" aria-valuenow="0"><div class="sf-meter-fill"></div></div>
        <p class="sf-caption sf-fund-stats"></p>
      </div>
      <p class="sf-thanks" hidden>Thank you for supporting development 💜</p>
      <button type="button" class="sf-send sf-support-pay">Support development</button>
      <p class="sf-caption">Tips are optional and unlock nothing.</p>
    </div>
  </div>
  ${markup ? MARKUP_TEMPLATE() : ""}
  <div class="sf-nudge" inert>
    <span class="sf-nudge-msg"></span>
    <button class="sf-nudge-open" type="button">Sure</button>
    <button class="sf-nudge-x" type="button" aria-label="Dismiss">✕</button>
    <div class="sf-nudge-links" hidden><button class="sf-nudge-later" type="button">Not now</button> · <button class="sf-nudge-optout" type="button">Don't ask again</button></div>
  </div>
  <div class="sf-toast" role="status"></div>`;
}

function MARKUP_TEMPLATE() {
  const tools = MARKUP_TOOLS.map((t, i) =>
    `<button class="sf-mk-btn sf-mk-tool${i ? "" : " sf-active"}" type="button" data-tool="${t.v}" aria-label="${t.label}" title="${t.label}">${t.icon}</button>`).join("");
  const swatches = MARKUP_COLORS.map((c, i) =>
    `<button class="sf-mk-swatch${i ? "" : " sf-active"}" type="button" data-color="${c.v}" aria-label="${c.label}" title="${c.label}"></button>`).join("");
  return `
  <div class="sf-markup-layer" hidden aria-hidden="true" role="dialog" aria-modal="true" aria-label="Mark up the screenshot">
    <div class="sf-mk-stage">
      <canvas class="sf-mk-canvas" role="img" aria-label="Screenshot markup canvas"></canvas>
    </div>
    <div class="sf-mk-bar">
      <div class="sf-mk-group">${tools}</div>
      <div class="sf-mk-group">${swatches}</div>
      <div class="sf-mk-group">
        <button class="sf-mk-btn sf-mk-undo" type="button" aria-label="Undo last shape" title="Undo">${UNDO_ICON}</button>
        <button class="sf-mk-btn sf-mk-clear" type="button" aria-label="Clear all shapes" title="Clear">${CLEAR_ICON}</button>
      </div>
      <span class="sf-mk-count" aria-live="polite"></span>
      <div class="sf-mk-group">
        <button class="sf-mk-btn sf-mk-cancel" type="button">Cancel</button>
        <button class="sf-mk-btn sf-mk-done" type="button">Done</button>
      </div>
    </div>
  </div>`;
}

function FLOATING_TEMPLATE(cfg) {
  const pos = {
    "right-center": "top:calc(50% - 26px);right:22px;", "left-center": "top:calc(50% - 26px);left:22px;",
    "bottom-right": "bottom:22px;right:22px;", "bottom-left": "bottom:22px;left:22px;",
    "top-right": "top:22px;right:22px;", "top-left": "top:22px;left:22px;",
  }[cfg.position] || "bottom:22px;right:22px;";
  const compact = cfg.compact || !cfg.label;
  const label = cfg.label || "Feedback";
  const accent = cfg.color || "#6d5efc";
  return `
  <style nonce="${esc(cfg.styleNonce || "")}">
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
  <style nonce="${esc(cfg.styleNonce || "")}">
    :host { all: initial; display: inline-block; }
    .sf-inline { display: inline-flex; align-items: center; gap: 7px; cursor: pointer; border: none; background: transparent;
      padding: 8px 10px; border-radius: 10px; color: ${accent}; font-family: ${FONT}; font-size: 14px; font-weight: 600; transition: background .15s; }
    .sf-inline:hover { background: rgba(120,120,140,.14); }
    .sf-inline svg { width: 16px; height: 16px; }
  </style>
  <button class="sf-inline" type="button" aria-label="${label}">${ICON}${cfg.compact ? "" : `<span>${label}</span>`}</button>`;
}

function plural(n, word) { return `${n} ${word}${n === 1 ? "" : "s"}`; }
function esc(s) { return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }
// Traverse at most three object levels and stop as soon as the output budget is spent.
// Never invoke getters or toJSON hooks, or stringify a whole object/array.
function safeStr(v) {
  let out = "", truncated = false;
  const seen = new WeakSet();
  const add = (text) => {
    const remaining = 500 - out.length;
    out += text.slice(0, remaining);
    if (text.length > remaining) truncated = true;
  };
  const quote = (text) => JSON.stringify(text.slice(0, 501));
  const visit = (value, depth) => {
    if (out.length >= 500) { truncated = true; return; }
    if (value === null || typeof value !== "object") {
      add(typeof value === "string" ? quote(value) : String(value)); return;
    }
    if (seen.has(value)) { add('"[Circular]"'); return; }
    if (depth >= 3) { add("…"); return; }
    seen.add(value);
    const array = Array.isArray(value);
    add(array ? "[" : "{");
    let count = 0;
    for (const key in value) {
      if (out.length >= 500 || count >= 100) { truncated = true; break; }
      if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
      if (count++) add(",");
      if (!array) { add(quote(key)); add(":"); }
      if (out.length >= 500) { truncated = true; break; }
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (descriptor && "value" in descriptor) visit(descriptor.value, depth + 1);
      else add('"[Getter]"');
    }
    add(array ? "]" : "}");
    seen.delete(value);
  };
  try {
    if (typeof v === "string") return v;
    else if (v instanceof Error) add(v.stack || (v.name + ": " + v.message));
    else visit(v, 0);
  } catch (_) { add("[unserializable]"); }
  return truncated ? out.slice(0, 499) + "…" : out;
}
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
