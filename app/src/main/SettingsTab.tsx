import { useEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { listen } from "../lib/tauri";
import { api, appVersion, engineName, errorText, langName, type HotkeyStatus, type Settings, type UpdateInfo } from "../lib/api";
import { Icon } from "../lib/icons";
import { applyTheme } from "../lib/theme";
import { Field, Segment, Toggle } from "../ui";

// Вёрстка вкладки перенесена из прежнего Main.tsx как есть: редизайн настроек — отдельный этап.

const EASE_OUT = [0.23, 1, 0.32, 1] as const;

const langs = ["ru", "en", "uk", "de", "fr", "es", "it", "pt", "pl", "tr", "ja", "ko", "zh"];
const THEMES: readonly { id: string; label: string }[] = [
  { id: "system", label: "Системная" },
  { id: "dark", label: "Тёмная" },
  { id: "light", label: "Светлая" },
];

// Компоненты вынесены из SettingsPanel: объявленные внутри, они пересоздавались на каждый рендер и поле теряло фокус после первого символа.
function Row({ label, children }: { label: string; children: ReactNode }) {
  return <div className="flex items-center gap-3 py-1.5"><span className="w-56 text-sm text-ink-2">{label}</span>{children}</div>;
}

// ---- запись хоткея с клавиатуры ----

/** Модификаторы в порядке, ожидаемом парсером global_hotkey: Ctrl+Alt+Shift+Super. */
function modsOf(e: KeyboardEvent | React.KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");
  return parts.join("+");
}

/** «Super» показываем как «Win» — так короче и понятнее пользователю Windows. */
function displayHotkey(v: string) {
  return v.split("+").map((p) => (p === "Super" ? "Win" : p)).join("+");
}

const F_KEY = /^F(\d{1,2})$/;
function isFKey(token: string) {
  const m = F_KEY.exec(token);
  return !!m && Number(m[1]) >= 1 && Number(m[1]) <= 24;
}

// Соответствие KeyboardEvent.code токенам, которые понимает parse_key крейта global-hotkey
// (src-tauri зависимость global-hotkey-0.8.0/src/hotkey.rs).
const CODE_TOKENS: Record<string, string> = {
  Space: "Space", Enter: "Enter", NumpadEnter: "NumpadEnter", Tab: "Tab", CapsLock: "CapsLock",
  ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
  Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\",
  Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/", Backquote: "`",
  Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown", Insert: "Insert",
  PrintScreen: "PrintScreen", ScrollLock: "ScrollLock", Pause: "Pause", NumLock: "NumLock",
  NumpadAdd: "NumpadAdd", NumpadSubtract: "NumpadSubtract", NumpadMultiply: "NumpadMultiply",
  NumpadDivide: "NumpadDivide", NumpadDecimal: "NumpadDecimal", NumpadEqual: "NumpadEqual",
};
function codeToToken(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^Numpad[0-9]$/.test(code)) return code;
  if (/^F(\d{1,2})$/.test(code) && isFKey(code)) return code;
  return CODE_TOKENS[code] ?? null;
}

function HotkeyField({ value, status, onCommit }: { value: string; status?: HotkeyStatus; onCommit: (next: string) => void }) {
  const [recording, setRecording] = useState(false);
  const [live, setLive] = useState("");
  const [localError, setLocalError] = useState("");
  const ref = useRef<HTMLButtonElement>(null);

  function start() {
    setRecording(true); setLive(""); setLocalError("");
    // Иначе зарегистрированный хоткей перехватит нажатие раньше, чем его увидит поле.
    void api.hotkeysSuspend(true).catch(() => undefined);
    requestAnimationFrame(() => ref.current?.focus());
  }
  function cancel() {
    setRecording(false); setLive(""); setLocalError("");
    void api.hotkeysSuspend(false).catch(() => undefined);
  }
  function onKeyDown(e: React.KeyboardEvent) {
    e.preventDefault();
    if (e.key === "Escape" || e.key === "Backspace" || e.key === "Delete") { cancel(); return; }
    if (e.key === "Control" || e.key === "Alt" || e.key === "Shift" || e.key === "Meta") { setLive(modsOf(e)); setLocalError(""); return; }
    const token = codeToToken(e.code);
    if (!token) return;
    const mods = modsOf(e);
    if (!mods && !isFKey(token)) { setLocalError("Нужен Ctrl, Alt, Shift или Win"); setLive(""); return; }
    const combo = mods ? `${mods}+${token}` : token;
    setRecording(false); setLive(""); setLocalError("");
    onCommit(combo);
  }

  const label = recording ? localError || (live ? `${displayHotkey(live)}+…` : "Нажмите сочетание…") : displayHotkey(value);

  return (
    <div className="flex items-center gap-2">
      <button
        ref={ref}
        type="button"
        className={`field w-44 text-left ${localError ? "text-err" : ""}`}
        onClick={start}
        onKeyDown={recording ? onKeyDown : undefined}
        onBlur={() => recording && cancel()}
      >
        {label}
      </button>
      {!recording && status?.error && <span className="text-xs text-err">{status.error}</span>}
      {!recording && status && !status.error && <Icon name="check" size={15} className="text-ok" />}
    </div>
  );
}

// ---- о программе и обновления ----

type Phase = "idle" | "checking" | "none" | "found" | "downloading" | "installing" | "error";

function About() {
  const reduce = useReducedMotion();
  const [version, setVersion] = useState("dev");
  const [phase, setPhase] = useState<Phase>("idle");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number | null }>({ done: 0, total: null });
  const [error, setError] = useState("");
  const [installFailed, setInstallFailed] = useState(false);

  useEffect(() => {
    appVersion().then(setVersion).catch(() => undefined);
    // Фоновая проверка могла найти обновление до того, как открыли настройки.
    api.updateAvailable().then((i) => { if (i) { setInfo(i); setPhase((p) => (p === "idle" ? "found" : p)); } }).catch(() => undefined);
    const un = listen<UpdateInfo>("update:available", ({ payload }) => {
      setInfo(payload);
      setPhase((p) => (p === "downloading" || p === "installing" ? p : "found"));
    });
    const up = listen<{ downloaded: number; total: number | null }>("update:progress", ({ payload }) => {
      setProgress({ done: payload.downloaded, total: payload.total });
      // Скачали всё — дальше работает установщик, окно вот-вот закроется.
      if (payload.total && payload.downloaded >= payload.total) setPhase("installing");
    });
    return () => { un.then((f) => f()); up.then((f) => f()); };
  }, []);

  async function check() {
    setPhase("checking"); setError(""); setInstallFailed(false);
    try {
      const found = await api.updateCheck();
      setInfo(found);
      if (found) { setPhase("found"); return; }
      setPhase("none");
      window.setTimeout(() => setPhase((p) => (p === "none" ? "idle" : p)), 3000);
    } catch (e) { setError(errorText(e)); setPhase("error"); }
  }

  async function install() {
    setPhase("downloading"); setProgress({ done: 0, total: null }); setError("");
    try { await api.updateInstall(); }
    catch (e) { setError(errorText(e)); setInstallFailed(true); setPhase("error"); }
  }

  const pct = progress.total ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : null;
  const anim = {
    initial: { opacity: 0, scale: reduce ? 1 : 0.97 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, scale: reduce ? 1 : 0.97 },
    transition: { duration: reduce ? 0.15 : 0.18, ease: EASE_OUT },
  };

  return (
    <Row label={`UTranslate ${version}`}>
      <AnimatePresence mode="wait" initial={false}>
        <motion.div key={phase} {...anim} className="flex items-center gap-3">
          {phase === "idle" && <button className="pill" onClick={check}>Проверить обновления</button>}
          {phase === "checking" && <span className="pill"><span className="dot pulse" />Проверяем…</span>}
          {phase === "none" && <span className="pill text-ok"><Icon name="check" size={15} />Установлена последняя версия</span>}
          {phase === "found" && (
            <>
              <span className="text-sm text-ink-2">Доступна {info?.version}</span>
              <button className="pill active" onClick={install}>Обновить</button>
            </>
          )}
          {phase === "downloading" && (
            <span className="pill relative overflow-hidden">
              {pct === null ? "Загрузка" : `Загрузка ${pct}%`}
              {pct === null
                ? <span className="sk absolute bottom-0 left-0 h-1 w-full rounded-none" />
                : <span className="absolute bottom-0 left-0 h-1 bg-water" style={{ width: `${pct}%`, transition: "width 150ms linear" }} />}
            </span>
          )}
          {phase === "installing" && <span className="pill"><span className="dot pulse" />Перезапуск…</span>}
          {phase === "error" && (
            <>
              <span className="text-sm text-err">{error}</span>
              <button className="pill" onClick={installFailed ? install : check}>Повторить</button>
            </>
          )}
        </motion.div>
      </AnimatePresence>
    </Row>
  );
}

/** Настройки. onSettings отдаёт свежую копию каркасу: тему и хоткеи показывают другие вкладки. */
export function SettingsTab({ onSettings }: { onSettings: (s: Settings) => void }) {
  const [s, setS] = useState<Settings | null>(null);
  const [autostart, setAutostart] = useState(false);
  const [statuses, setStatuses] = useState<HotkeyStatus[]>([]);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState("");
  const savedTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    api.getSettings().then(setS);
    api.autostartGet().then(setAutostart).catch(() => undefined);
    // Занятый на этой машине хоткей должен быть подсвечен сразу, без ожидания правки.
    api.hotkeysStatus().then(setStatuses).catch(() => undefined);
  }, []);

  if (!s) return null;

  async function persist(next: Settings) {
    setS(next);
    onSettings(next);
    // Тема применяется сразу: в попап её донесёт событие settings:changed из settings_set.
    applyTheme(next.theme);
    try {
      const status = await api.setSettings(next);
      setStatuses(status);
      setSaveError("");
      setSaved(true);
      window.clearTimeout(savedTimer.current);
      savedTimer.current = window.setTimeout(() => setSaved(false), 1200);
    } catch (e) {
      setSaveError(errorText(e));
    }
  }
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => persist({ ...s, [k]: v });

  async function toggleAutostart() {
    const next = !autostart; setAutostart(next);
    try { await api.autostartSet(next); } catch (e) { setAutostart(!next); setSaveError(errorText(e)); }
  }

  const statusOf = (field: HotkeyStatus["field"]) => statuses.find((x) => x.field === field);

  return (
    <div className="card flex min-h-0 flex-1 flex-col gap-1 overflow-auto p-5">
      <div className="mb-1 flex items-center gap-2">
        <span className="text-xs font-medium uppercase tracking-wider text-ink-3">Хоткеи</span>
        <span className="text-xs text-ok" style={{ opacity: saved ? 1 : 0, transition: "opacity 150ms ease" }}>Сохранено</span>
        {saveError && <span className="text-xs text-err">{saveError}</span>}
      </div>
      <Row label="Перевести в попап"><HotkeyField value={s.hotkeyPopup} status={statusOf("hotkeyPopup")} onCommit={(v) => set("hotkeyPopup", v)} /></Row>
      <Row label="Заменить выделенное"><HotkeyField value={s.hotkeyReplace} status={statusOf("hotkeyReplace")} onCommit={(v) => set("hotkeyReplace", v)} /></Row>
      <Row label="Открыть окно"><HotkeyField value={s.hotkeyWindow} status={statusOf("hotkeyWindow")} onCommit={(v) => set("hotkeyWindow", v)} /></Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink-3">Языки</div>
      <Row label="Переводить на">
        <select className="field w-44" value={s.primaryLang} onChange={(e) => set("primaryLang", e.target.value)}>{langs.map((l) => <option key={l} value={l}>{langName(l)}</option>)}</select>
      </Row>
      <Row label="Если текст уже на этом языке — на">
        <select className="field w-44" value={s.secondaryLang} onChange={(e) => set("secondaryLang", e.target.value)}>{langs.map((l) => <option key={l} value={l}>{langName(l)}</option>)}</select>
      </Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink-3">Движки</div>
      <Row label="Порядок цепочки">
        <div className="flex gap-2">
          {s.engines.map((e, i) => (
            <button key={e} className="pill" title="Сделать первым" onClick={() => set("engines", [e, ...s.engines.filter((x) => x !== e)])}>{i === 0 && <span className="dot" />}{engineName(e)}</button>
          ))}
        </div>
      </Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink-3">Общее</div>
      <Row label="Тема">
        <Segment items={THEMES} value={s.theme} onChange={(id) => set("theme", id)} layoutId="theme-pill" />
      </Row>
      <Row label="Запускать вместе с Windows"><Toggle on={autostart} label="Запускать вместе с Windows" onClick={toggleAutostart} /></Row>
      <Row label="Вести историю"><Toggle on={s.historyEnabled} label="Вести историю" onClick={() => set("historyEnabled", !s.historyEnabled)} /></Row>
      <Row label="Размер текста в попапе">
        <Field type="number" min={12} max={24} className="w-24" value={s.fontSize} onChange={(e) => set("fontSize", Number(e.target.value) || 16)} />
      </Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink-3">О программе</div>
      <About />
    </div>
  );
}
