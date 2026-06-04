import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App'

// Apply the OS theme immediately to avoid a flash; App refines it from settings.
if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
  document.documentElement.classList.add('dark')
}

createRoot(document.getElementById('root')!).render(<App />)
