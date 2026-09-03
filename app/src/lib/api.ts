import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { hasTauri } from "./tauri";

export type Alternative = { pos: string; terms: string[] };
export type Translation = {
  text: string;
  detected: string;
  target: string;
  engine: string;
  alternatives: Alternative[];
  fallbackFrom: string | null;
};
export type TranslateResult = Translation & { historyId: number | null; wordMode: boolean };
export type Entry = {
  id: number;
  sourceText: string;
  resultText: string;
  sourceLang: string;
  targetLang: string;
  engine: string;
  mode: string;
  isFavorite: boolean;
  createdAt: number;
};
export type Settings = {
  hotkeyPopup: string;
  hotkeyReplace: string;
  hotkeyWindow: string;
  primaryLang: string;
  secondaryLang: string;
  engines: string[];
  theme: string;
  uiLang: string;
  historyEnabled: boolean;
  showOriginal: boolean;
  fontSize: number;
};
export type UpdateInfo = { version: string; notes: string | null; date: string | null };
export type HotkeyStatus = { field: "hotkeyPopup" | "hotkeyReplace" | "hotkeyWindow"; error: string | null };

export const api = {
  translate: (text: string, target?: string) => invoke<TranslateResult>("translate_text", { text, target }),
  copy: (text: string) => invoke<void>("copy_text", { text }),
  openMain: (text?: string) => invoke<void>("open_main", { text }),
  history: (query = "", favoritesOnly = false) => invoke<Entry[]>("history_list", { query, favoritesOnly }),
  setFavorite: (id: number, favorite: boolean) => invoke<void>("history_set_favorite", { id, favorite }),
  deleteEntry: (id: number) => invoke<void>("history_delete", { id }),
  clearHistory: () => invoke<void>("history_clear"),
  exportFavorites: () => invoke<string>("favorites_export"),
  getSettings: () => invoke<Settings>("settings_get"),
  setSettings: (settings: Settings) => invoke<HotkeyStatus[]>("settings_set", { settings }),
  hotkeysStatus: () => invoke<HotkeyStatus[]>("hotkeys_status"),
  hotkeysSuspend: (suspended: boolean) => invoke<void>("hotkeys_suspend", { suspended }),
  autostartGet: () => invoke<boolean>("autostart_get"),
  autostartSet: (enabled: boolean) => invoke<void>("autostart_set", { enabled }),
  updateCheck: () => invoke<UpdateInfo | null>("update_check"),
  updateAvailable: () => invoke<UpdateInfo | null>("update_available"),
  updateInstall: () => invoke<void>("update_install"),
};

/** Вне Tauri (отладка вёрстки в браузере) версии нет. */
export const appVersion = () => (hasTauri ? getVersion() : Promise.resolve("dev"));

export const LANGS: Record<string, string> = {
  en: "Английский", ru: "Русский", uk: "Украинский", be: "Белорусский", de: "Немецкий", fr: "Французский",
  es: "Испанский", it: "Итальянский", pt: "Португальский", pl: "Польский", tr: "Турецкий", nl: "Нидерландский",
  sv: "Шведский", cs: "Чешский", ja: "Японский", ko: "Корейский", zh: "Китайский", ar: "Арабский", he: "Иврит",
};
export const langName = (code: string | null | undefined) => (code ? LANGS[code] ?? code.toUpperCase() : "Авто");

export const ENGINE_NAMES: Record<string, string> = { google: "Google", bing: "Bing", mymemory: "MyMemory" };
export const engineName = (id: string) => ENGINE_NAMES[id] ?? id;

/** Подпись движка с причиной fallback: «Bing · Google недоступен». */
export function engineLabel(t: Translation) {
  const base = engineName(t.engine);
  if (!t.fallbackFrom) return base;
  const failed = t.fallbackFrom.split(":")[0];
  return `${base} · ${engineName(failed)} недоступен`;
}

export function speak(text: string, lang: string) {
  const u = new SpeechSynthesisUtterance(text);
  u.lang = lang;
  speechSynthesis.cancel();
  speechSynthesis.speak(u);
}

export function errorText(e: unknown) {
  const s = typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
  if (/error sending request|dns|connect|timed out|timeout/i.test(s)) return "Нет соединения";
  return s;
}
