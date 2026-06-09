// Types for the vendored SuperFeedback widget (lman80/SuperFeedback) v1.1.0.
export interface SuperFeedbackConfig {
  /** The deployed backend Worker URL. */
  backendUrl: string
  /** Where Issues open, as "owner/name". */
  repo: string
  /** Human name for this app (shown in the issue). */
  app?: string
  /** Only if the backend requires an APP_KEY (ours doesn't). */
  appKey?: string
  /** How the button is presented. */
  trigger?: 'floating' | 'mounted' | 'none'
  /** For trigger:"mounted" — CSS selector or element to place the button inside. */
  mount?: string | Element
  /** Floating corner (trigger:"floating" only). */
  position?: 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left'
  /** Icon-only button (no label). */
  compact?: boolean
  label?: string
  color?: string
  attachScreenshot?: boolean
  type?: 'bug' | 'feature' | 'other'
  /** App version, attached to each report's metadata. */
  appVersion?: string
  /** Optional native screenshot override returning a PNG data URL. */
  captureScreenshot?: () => Promise<string>
  meta?: Record<string, unknown>
}

export const SuperFeedback: {
  version: string
  init(config: SuperFeedbackConfig): void
  /** Open the centered feedback panel (use with trigger:"none" / custom UI). */
  open(): void
  close(): void
  toggle(): void
  destroy(): void
}

declare global {
  interface Window {
    SuperFeedback?: typeof SuperFeedback
  }
}
