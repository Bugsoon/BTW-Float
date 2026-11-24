import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./index.css";
import { BallWindow } from "./windows/BallWindow";
import { CaptureWindow } from "./windows/CaptureWindow";
import { PopupWindow } from "./windows/PopupWindow";
import { SettingsWindow } from "./windows/SettingsWindow";

function resolveWindowKind(): string {
  try {
    const label = getCurrentWindow().label;
    if (
      label === "ball" ||
      label === "popup" ||
      label === "settings" ||
      label === "capture"
    ) {
      return label;
    }
  } catch {
    // fallback to URL param in browser preview
  }
  const params = new URLSearchParams(window.location.search);
  return params.get("window") ?? "popup";
}

function AppRouter() {
  const kind = resolveWindowKind();
  if (kind === "ball") return <BallWindow />;
  if (kind === "capture") return <CaptureWindow />;
  if (kind === "settings") return <SettingsWindow />;
  return <PopupWindow />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AppRouter />
  </React.StrictMode>
);
