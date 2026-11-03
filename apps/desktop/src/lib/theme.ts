export type ThemePref = "system" | "light" | "dark";

const KEY = "tether.theme";

export function getThemePref(): ThemePref {
  try {
    return (localStorage.getItem(KEY) as ThemePref) || "system";
  } catch {
    return "system";
  }
}

export function setThemePref(p: ThemePref) {
  try {
    localStorage.setItem(KEY, p);
  } catch {
    /* storage unavailable: keep for this window only */
  }
  applyTheme();
}

const mq = window.matchMedia("(prefers-color-scheme: dark)");

export function applyTheme() {
  const p = getThemePref();
  const dark = p === "dark" || (p === "system" && mq.matches);
  document.documentElement.classList.toggle("dark", dark);
}

mq.addEventListener("change", applyTheme);
window.addEventListener("storage", (e) => e.key === KEY && applyTheme());
