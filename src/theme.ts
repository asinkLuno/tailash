//! 主题模式：跟随系统 / 浅色 / 深色，选择存 localStorage。
//! 跟随系统不设 data-theme，交给 daisyUI 的 dark --prefersdark 响应 prefers-color-scheme。
export type ThemeMode = "light" | "dark" | "system";

const KEY = "tailash-theme";

export function readStoredMode(): ThemeMode {
  const v = localStorage.getItem(KEY);
  return v === "light" || v === "dark" ? v : "system";
}

export function storeMode(mode: ThemeMode): void {
  localStorage.setItem(KEY, mode);
}

export function applyMode(mode: ThemeMode): void {
  const html = document.documentElement;
  if (mode === "system") html.removeAttribute("data-theme");
  else html.setAttribute("data-theme", mode);
}
