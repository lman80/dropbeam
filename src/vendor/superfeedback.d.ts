/** Public community idea; timestamps are Unix seconds. */
export interface CommunityIdea {
  id: number;
  number: number;
  title: string;
  summary: string;
  status: 'suggested' | 'popular' | 'accepted' | 'building' | 'testing' | 'shipped';
  votes: number;
  voted: boolean;
  createdAt?: number;
  version?: string;
  shippedAt?: number;
}

export interface CommunityPayload {
  ok: true;
  app: { repo: string; syncedAt: number };
  ideas: CommunityIdea[];
  shipped: CommunityIdea[];
  notices: { id: number; title: string; version: string; shippedAt: number }[];
  you: { votes: number; submitted: number; shipped: number; supporter: boolean };
  fund: { currency: string; monthCents: number; goalCents: number; supporters: number; allTimeCents: number; shippedThisMonth: number; supportUrl?: string | null };
}

/**
 * What gets sent: message/category, selected screenshot captured before the panel opens,
 * user images, and recent diagnostics (200 entries / 40 KB UTF-8 JSON, newest last).
 * Lines: HH:mm:ss.SSS LEVEL [source] text. Sources: app breadcrumbs; console
 * error/warn/info/log (500 characters per line); error stacks; failed fetch/XHR
 * method + query-free URL + status/network error; navigation URLs; interactive
 * clicks (tag + label up to 40 characters). Widget events and input values are excluded.
 * Metadata includes page/device context plus uptime, timezone, colorScheme, screen/DPR,
 * network, optional memoryMB, reduceMotion, per-page sessionId and screenshot status
 * (attached, attached (annotated, N shapes), declined, or capture failed: reason).
 * Markup composites the drawing onto the capture at its native size before sending.
 * Crashes persist for next launch.
 * Sensitive apps: installers should disable captureLogs/captureCrashes and review
 * screenshot/custom context exposure. attachScreenshot:false only defaults the switch
 * off; capture still occurs at open. Automatic text/URL redaction is not provided.
 */
/** Support nudge eligibility; all fields are optional. */
export interface SupportNudgeOptions {
  /** Shared timestamp cooldown; defaults to 30 days. */
  cooldownDays?: number;
  /** Defaults to 3 launches. */
  minLaunches?: number;
  /** Defaults to 3 completed moments. */
  minMoments?: number;
  /** Silence after a recorded tip; defaults to 180 days. */
  afterTipDays?: number;
  /** Ask after a shipped notice; defaults to true. */
  thankYou?: boolean;
}

/** Configuration for the single-file SuperFeedback web widget. */
export interface SuperFeedbackConfig {
  /** Deployed backend Worker URL (required). */
  backendUrl: string;
  /** Repository receiving issues, in owner/name form (required). */
  repo: string;
  /** App display name shown in the panel and report. */
  app?: string;
  /** Backend APP_KEY, when required; defaults to empty. */
  appKey?: string;
  /** Trigger presentation; defaults to draggable. */
  trigger?: 'draggable' | 'floating' | 'mounted' | 'none';
  /** Selector or element receiving the inline trigger; implies mounted. */
  mount?: string | Element | null;
  /** Initial position; defaults to right-center for draggable, bottom-right for floating. */
  position?: 'right-center' | 'left-center' | 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left';
  /** Accent hex color; defaults to #6d5efc. */
  color?: string;
  /** Panel theme; auto follows the system and is the default. */
  theme?: 'auto' | 'light' | 'dark';
  /** Floating or mounted button label; defaults to Feedback. */
  label?: string;
  /** Use an icon-only floating or mounted button; defaults to false. */
  compact?: boolean;
  /** Initial report category; defaults to bug. */
  type?: 'bug' | 'feature' | 'other';
  /** Initial screenshot switch state on every open; defaults to true. */
  attachScreenshot?: boolean;
  /** Offer the markup editor (pen, circle, arrow, rectangle) on the screenshot; defaults to true. */
  markup?: boolean;
  /** Repo for feedback about the widget itself (the "Feedback on SuperFeedback" link under Send); defaults to lman80/SuperFeedback, false hides the link. */
  widgetFeedback?: string | boolean;
  /** Maximum user attachments; defaults to 5, zero disables attachments. */
  maxImages?: number;
  /** Capture diagnostics and breadcrumbs; false records/sends no logs, including queued logs. Defaults to true. */
  captureLogs?: boolean;
  /** Maximum serialized breadcrumbs; defaults to 200, clamped to 1–200; UTF-8 JSON capped at 40 KB. */
  maxLogs?: number;
  /** Persist unhandled crashes and report on next launch; defaults to true. */
  captureCrashes?: boolean;
  /** Optional feedback invitation; defaults to false. */
  nudge?: boolean | {
    /** Invitation text; defaults to Got feedback? We'd love to hear it 💜. */
    message?: string;
    /** Delay before showing the invitation; defaults to 45000 milliseconds. */
    delayMs?: number;
    /** Minimum days between invitations; defaults to 7. */
    cooldownDays?: number;
  };
  /** Show Ideas and fetch community data; defaults to true. */
  community?: boolean;
  /** Stripe Payment Link. StoreKit products without a url are ignored on web. */
  support?: ({ url: string; products?: string[] } | { url?: undefined; products: string[] }) & {
    /** What tips fund; max 60 characters, defaults to the AI tools and servers. */
    purpose?: string;
  };
  /** Support invitations at completed moments and after shipped notices; defaults to true. */
  supportNudge?: boolean | SupportNudgeOptions;
  /**
   * Daily anonymous check-in to the backend (POST /checkin): widget/app version, platform, OS and
   * this config summary (appKey only as set/unset, no user data). Once per UTC day per repo, or
   * when the widget or app version changes; silent on failure. Defaults to true; false disables.
   */
  checkin?: boolean;
  /** App version included in report metadata and the check-in. */
  appVersion?: string;
  /** Optional build number sent with the check-in (meta.build is used when absent). */
  build?: string;
  /** Static metadata, overridden by setContext and the send-time environment snapshot. */
  meta?: Record<string, unknown>;
  /** Nonce applied to every injected style element; must match the page's style-src nonce. */
  styleNonce?: string;
  /** Bundled html-to-image module or its Promise; avoids CDN imports. captureScreenshot takes precedence. */
  captureModule?: {
    toPng: (node: HTMLElement, options?: { cacheBust?: boolean; pixelRatio?: number; filter?: (node: HTMLElement) => boolean }) => Promise<string>;
  } | Promise<{
    toPng: (node: HTMLElement, options?: { cacheBust?: boolean; pixelRatio?: number; filter?: (node: HTMLElement) => boolean }) => Promise<string>;
  }>;
  /** Capture at open, never send; returns a data URL or null. Bypasses all capture module imports. */
  captureScreenshot?: (config: SuperFeedbackConfig) => string | null | Promise<string | null>;
}

export declare const SuperFeedback: {
  readonly version: '3.3.1';
  /** Initialize or replace the current window's widget and flush queued reports. */
  init(config: SuperFeedbackConfig): void;
  /** Capture the app, then open the panel, even when the trigger is disabled. */
  open(): Promise<void>;
  /** Read the origin-wide supporter ID; undefined before initialization. */
  voterId(): string | undefined;
  /** Capture and open Ideas; falls back to Feedback when unavailable. */
  openIdeas(): Promise<void>;
  /** Capture and open Support; falls back to Feedback when unconfigured. */
  openSupport(): Promise<void>;
  /** Close the panel and discard the draft, or cancel a pending open. */
  close(): void;
  /** Close an open panel, otherwise capture and open it. */
  toggle(): void | Promise<void>;
  /** Remove hosts, timers, and installed listeners. */
  destroy(): void;
  /** Record a breadcrumb for the next report; level defaults to info. */
  log(message: unknown, level?: string): void;
  /** Record a successful user outcome; persisted count caps at 1000, last name is report metadata, distinct names (up to 20) go in the check-in. */
  moment(name: string): void;
  /** Persist whether the trigger is visible; does not prevent open(). */
  setEnabled(enabled: boolean): void;
  /** Read the current trigger visibility preference, enabled by default. */
  isEnabled(): boolean;
  /** Shallow-merge runtime values into report metadata at send time. */
  setContext(context: Record<string, unknown>): void;
};

declare global {
  interface Window {
    SuperFeedback: typeof SuperFeedback;
  }
}
