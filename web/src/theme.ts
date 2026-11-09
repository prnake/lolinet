import { useEffect, useState } from "react";

// 主题：auto 跟系统；light / dark 手动。选择存 localStorage，首屏由 index.html 里的内联脚本先设好 data-theme，避免闪一下。
export type ThemePref = "auto" | "light" | "dark";
const KEY = "mesh.theme";
const media = () => window.matchMedia("(prefers-color-scheme: light)");

function readPref(): ThemePref {
  const v = localStorage.getItem(KEY);
  return v === "light" || v === "dark" ? v : "auto";
}

function apply(pref: ThemePref) {
  const light = pref === "light" || (pref === "auto" && media().matches);
  document.documentElement.dataset.theme = light ? "light" : "dark";
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", light ? "#f5f5f2" : "#000000");
}

export function useTheme() {
  const [pref, setPref] = useState<ThemePref>(readPref);
  useEffect(() => {
    apply(pref);
    if (pref === "auto") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, pref);
    if (pref !== "auto") return;
    const m = media();
    const onChange = () => apply("auto");
    m.addEventListener("change", onChange);
    return () => m.removeEventListener("change", onChange);
  }, [pref]);
  return [pref, setPref] as const;
}
