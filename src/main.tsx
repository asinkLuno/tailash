import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyMode, readStoredMode } from "./theme";
import "./index.css";

// 首帧前套用已保存的主题；跟随系统由 CSS 的 prefers-color-scheme 直接生效，无需等 JS。
applyMode(readStoredMode());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
