import { Component, type CSSProperties, type ErrorInfo, type ReactNode } from 'react'
import { api } from '../lib/api'

interface Props {
  region: string
  children: ReactNode
  fallbackStyle?: CSSProperties
}

/** Keep a failed region from unmounting its siblings. The fallback deliberately
 * uses no store, icons, or animation code that could repeat the original error. */
export class ErrorBoundary extends Component<Props, { failed: boolean }> {
  state = { failed: false }

  static getDerivedStateFromError() {
    return { failed: true }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    const message = `ui render error: region=${this.props.region}\n${error?.stack || String(error)}\n${info.componentStack || ''}`
    console.error(message)
    // Report directly, without the store/toasts. A logging failure must never
    // throw from the boundary while it is handling the original exception.
    try {
      void api.frontendLog(message).catch(() => {})
    } catch { /* native bridge unavailable */ }
  }

  render() {
    if (!this.state.failed) return this.props.children
    return (
      <div role="alert" aria-label={`${this.props.region} error`} style={{
        margin: 12, padding: 16, borderRadius: 10,
        background: 'var(--bg-elev, #fff)', color: 'var(--text, #222)',
        border: '1px solid var(--border, #ccc)', alignSelf: 'flex-start',
        ...this.props.fallbackStyle,
      }}>
        <p style={{ margin: '0 0 10px' }}>Something went wrong — reload</p>
        <button type="button" className="btn btn-ghost" onClick={() => window.location.reload()}>
          Reload
        </button>
      </div>
    )
  }
}
