// Types for the vendored SuperFeedback widget (lman80/SuperFeedback).
export interface SuperFeedbackConfig {
  /** The deployed backend Worker URL. */
  backendUrl: string
  /** Where Issues open, as "owner/name". */
  repo: string
  /** Human name for this app (shown in the issue). */
  app?: string
  /** Only if the backend requires an APP_KEY (ours doesn't). */
  appKey?: string
  position?: 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left'
  label?: string
  /** App version, attached to each report's metadata. */
  appVersion?: string
  /** Optional native screenshot override returning a PNG data URL. */
  captureScreenshot?: () => Promise<string>
  meta?: Record<string, unknown>
}

export const SuperFeedback: {
  version: string
  init(config: SuperFeedbackConfig): void
  destroy(): void
}
