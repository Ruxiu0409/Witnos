import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { applyAppearance, detectTheme, resolveAppearance, syncWindowTheme } from './theme'

// Before the first paint, not in an effect: the palette has to be right on the
// first frame, or a light desktop opens the app on a flash of dark.
const theme = detectTheme()
applyAppearance(resolveAppearance(theme))
syncWindowTheme(theme)

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
