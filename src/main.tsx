import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { TrayPanel } from "./components/TrayPanel";
import { applyTheme, getTheme } from "./lib/theme";

applyTheme(getTheme());

/** The menu bar panel is its own small window; everything else is the app. */
function isPanel(): boolean {
  try {
    return (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } })
      .__TAURI_INTERNALS__?.metadata?.currentWindow?.label === "tray";
  } catch {
    return false;
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isPanel() ? <TrayPanel /> : <App />}</React.StrictMode>,
);
