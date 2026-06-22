import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import './index.css'

const root = document.getElementById('root')

function showBootError(error) {
  const message = error?.stack || error?.message || String(error)
  root.innerHTML = `
    <div class="settings-container">
      <div class="settings-shell">
        <section class="settings-section">
          <div class="section-heading">
            <h2>Settings failed to load</h2>
          </div>
          <pre class="boot-error">${message.replace(/[<>&]/g, (char) => ({
            '<': '&lt;',
            '>': '&gt;',
            '&': '&amp;',
          })[char])}</pre>
        </section>
      </div>
    </div>
  `
}

window.addEventListener('error', (event) => showBootError(event.error || event.message))
window.addEventListener('unhandledrejection', (event) => showBootError(event.reason))

try {
  root.innerHTML = '<div class="settings-container"><div class="empty-state">Loading settings...</div></div>'
  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  )
} catch (error) {
  showBootError(error)
}
