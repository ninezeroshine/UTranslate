import { useEffect, useRef, useState, type ReactNode } from "react";
import { listen, win } from "../lib/tauri";
import { api, appVersion, engineLabel, engineName, errorText, langName, speak, type Entry, type HotkeyStatus, type Settings, type TranslateResult, type UpdateInfo } from "../lib/api";
import { Icon } from "../lib/icons";
import { applyTheme } from "../lib/theme";
import { AnimatePresence, LayoutGroup, motion, useReducedMotion, type Transition } from "motion/react";

type Tab = "translate" | "history" | "favorites" | "settings";
const TABS: { id: Tab; label: string }[] = [
  { id: "translate", label: "Перевод" },
  { id: "history", label: "История" },
  { id: "favorites", label: "Избранное" },
  { id: "settings", label: "Настройки" },
];

// Токены анимации — см. docs/motion.md.
const EASE_OUT = [0.23, 1, 0.32, 1] as const;
const T_RESIZE: Transition = { type: "spring", duration: 0.3, bounce: 0 };

export default function Main() {
  const [tab, setTab] = useState<Tab>("translate");
  const [prefill, setPrefill] = useState<{ text: string; n: number }>({ text: "", n: 0 });

  useEffect(() => {
    api.getSettings().then((s) => applyTheme(s.theme)).catch(() => undefined);
    const un = listen<string>("main:prefill", ({ payload }) => {
      setPrefill((p) => ({ text: payload, n: p.n + 1 }));
      setTab("translate");
    });
    return () => { un.then((f) => f()); };
  }, []);

  return (
    <div className="spot flex h-full flex-col">
      <div data-tauri-drag-region className="flex h-14 shrink-0 items-center pl-5 pr-3.5">
        <div className="pointer-events-none flex w-[220px] items-center gap-2.5">
          <div className="flex h-6 w-6 items-center justify-center rounded-full text-on-water" style={{ background: "radial-gradient(circle at 30% 25%, var(--mist), var(--water) 55%, var(--water-deep))" }}>
            <Icon name="translate" size={14} />
          </div>
          <span className="text-sm font-medium tracking-tight">UTranslate</span>
        </div>
        <div data-tauri-drag-region className="flex flex-1 justify-center">
          <LayoutGroup id="tabs">
            <div className="segbar flex gap-0.5 p-[3px]">
              {TABS.map((t) => (
                <button key={t.id} className={`seg ${tab === t.id ? "active" : ""}`} onClick={() => setTab(t.id)}>
                  {tab === t.id && <motion.div layoutId="tab-pill" className="tab-pill" transition={T_RESIZE} />}
                  <span className="relative">{t.label}</span>
                </button>
              ))}
            </div>
          </LayoutGroup>
        </div>
        <div className="flex w-[220px] justify-end gap-1">
          <button className="wc" onClick={() => win?.minimize()} title="Свернуть"><Icon name="minimize" size={10} /></button>
          <button className="wc" onClick={() => win?.toggleMaximize()} title="Развернуть"><Icon name="maximize" size={10} /></button>
          <button className="wc close" onClick={() => win?.hide()} title="Скрыть в трей"><Icon name="close" size={10} /></button>
        </div>
      </div>
      <div className="min-h-0 flex-1 px-5 pb-5 pt-1">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={tab}
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0, transition: { duration: 0.15, ease: EASE_OUT } }}
            exit={{ opacity: 0, transition: { duration: 0.1 } }}
            className="h-full"
          >
            {tab === "translate" && <Translate prefill={prefill} />}
            {tab === "history" && <History key="h" favorites={false} onPick={(t) => { setPrefill((p) => ({ text: t, n: p.n + 1 })); setTab("translate"); }} />}
            {tab === "favorites" && <History key="f" favorites onPick={(t) => { setPrefill((p) => ({ text: t, n: p.n + 1 })); setTab("translate"); }} />}
            {tab === "settings" && <SettingsPanel />}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}

function Translate({ prefill }: { prefill: { text: string; n: number } }) {
  const reduce = useReducedMotion();
  const [source, setSource] = useState(prefill.text);
  const [target, setTarget] = useState<string | undefined>(undefined);
  const [result, setResult] = useState<TranslateResult | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "error">("idle");
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [favorite, setFavorite] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => { api.getSettings().then(setSettings).catch(() => undefined); }, []);
  useEffect(() => { if (prefill.n > 0) setSource(prefill.text); }, [prefill]);

  useEffect(() => {
    window.clearTimeout(timer.current);
    if (!source.trim()) { setResult(null); setStatus("idle"); return; }
    timer.current = window.setTimeout(run, 600);
    return () => window.clearTimeout(timer.current);
  }, [source, target]);

  async function run() {
    setStatus("loading"); setError("");
    try { setResult(await api.translate(source, target)); setFavorite(false); setStatus("idle"); }
    catch (e) { setError(errorText(e)); setStatus("error"); }
  }

  function swap() {
    if (!result) return;
    const from = result.detected;
    setTarget(from);
    setSource(result.text);
  }

  async function toggleFavorite() {
    if (!result?.historyId) return;
    const next = !favorite; setFavorite(next);
    try { await api.setFavorite(result.historyId, next); } catch { setFavorite(!next); }
  }

  const engines = settings?.engines ?? ["google", "bing", "mymemory"];
  const targetLabel = target ?? (result ? result.target : settings?.primaryLang ?? "ru");

  return (
    <div className="flex h-full flex-col gap-3.5">
      <div className="relative grid min-h-0 flex-1 grid-cols-2 gap-4">
        <div className="card flex flex-col gap-2 rounded-[20px] p-3">
          <div className="relative flex items-center gap-2">
            <div className="pill pl-2.5!">
              <span className="tracking-wide text-ink/70">{(result?.detected ?? "auto").toUpperCase()}</span>
              <span>{result ? langName(result.detected) : "Определить язык"}</span>
            </div>
            <div className="flex-1" />
            {source && <button className="rb" onClick={() => setSource("")} title="Очистить"><Icon name="close" /></button>}
          </div>
          <textarea
            value={source}
            onChange={(e) => setSource(e.target.value)}
            placeholder="Введите или вставьте текст…"
            className="relative min-h-0 flex-1 resize-none bg-transparent px-2.5 py-2 text-base leading-relaxed outline-none placeholder:text-ink/30"
            spellCheck={false}
          />
          <div className="relative flex items-center gap-2">
            <span className="pl-2.5 text-xs text-ink/40">{source.length} / 5000</span>
            <div className="flex-1" />
            <button className="rb" onClick={() => speak(source, result?.detected ?? "en")} title="Озвучить" disabled={!source}><Icon name="speaker" /></button>
          </div>
        </div>

        <div className="card flex flex-col gap-2 rounded-[20px] p-3">
          <div className="relative flex items-center gap-2">
            <button className="pill pl-2.5!" onClick={() => { if (settings) setTarget(targetLabel === settings.primaryLang ? settings.secondaryLang : settings.primaryLang); }} title="Сменить целевой язык">
              <span className={`dot ${status === "loading" ? "pulse" : ""}`} />
              <span className="tracking-wide text-water">{targetLabel.toUpperCase()}</span>
              <span>{langName(targetLabel)}</span>
              <Icon name="chevron" size={12} className="opacity-50" />
            </button>
            <span className="text-xs text-ink/45">{status === "loading" ? "переводим…" : result ? engineLabel(result) : ""}</span>
            <div className="flex-1" />
          </div>
          <div className="relative min-h-0 flex-1 overflow-auto px-2.5 py-2" style={{ position: "relative" }}>
            <AnimatePresence mode="popLayout" initial={false}>
              {status === "error" ? (
                <motion.div key="error" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: reduce ? 0.15 : 0.18 }} className="flex flex-col gap-3">
                  <div className="text-base text-ink/85">{error}</div>
                  <button className="pill w-fit" onClick={run}><Icon name="refresh" size={15} className="opacity-70" />Повторить</button>
                </motion.div>
              ) : status === "loading" && !result ? (
                <motion.div key="skeleton" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: reduce ? 0.15 : 0.18 }} className="flex flex-col gap-2.5 py-1">
                  <div className="sk w-[92%]" /><div className="sk w-[78%]" /><div className="sk w-[40%]" />
                </motion.div>
              ) : result ? (
                <motion.div
                  key="result"
                  initial={{ opacity: 0, filter: reduce ? "none" : "blur(3px)" }}
                  animate={{ opacity: 1, filter: "blur(0px)" }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: reduce ? 0.15 : 0.18 }}
                  className="flex flex-col gap-3"
                >
                  <div className="select-text text-base leading-relaxed" style={{ textWrap: "pretty" }}>{result.text}</div>
                  {result.wordMode && result.alternatives.length > 0 && (
                    <div className="flex flex-col gap-1.5 border-t border-ink/10 pt-2">
                      {result.alternatives.slice(0, 4).map((a) => (
                        <div key={a.pos} className="flex flex-wrap items-center gap-1.5">
                          <span className="px-1 text-[11px] font-medium uppercase tracking-wider text-ink/40">{a.pos}</span>
                          {a.terms.slice(0, 6).map((t) => <button key={t} className="chip" onClick={() => api.copy(t)} title="Скопировать">{t}</button>)}
                        </div>
                      ))}
                    </div>
                  )}
                </motion.div>
              ) : (
                <motion.div key="empty" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: reduce ? 0.15 : 0.18 }} className="text-base text-ink/30">
                  Перевод появится здесь
                </motion.div>
              )}
            </AnimatePresence>
          </div>
          <div className="relative flex items-center gap-2">
            <button className="pill pl-2.5!" disabled={!result} onClick={() => { if (result) api.copy(result.text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1200); }); }}>
              <Icon name={copied ? "check" : "copy"} size={15} className="opacity-70" />{copied ? "Скопировано" : "Копировать"}
            </button>
            <button className="pill pl-2.5!" disabled={!result} onClick={() => result && speak(result.text, result.target)}><Icon name="speaker" size={15} className="opacity-70" />Озвучить</button>
            <button className={`rb ${favorite ? "warm" : ""}`} disabled={!result?.historyId} onClick={toggleFavorite} title="В избранное"><Icon name="star" /></button>
          </div>
        </div>

        <button
          className="swap absolute left-1/2 top-3 flex h-10 w-10 -translate-x-1/2 items-center justify-center rounded-full"
          onClick={swap}
          title="Поменять местами"
        >
          <Icon name="swap" />
        </button>
      </div>

      <div className="flex shrink-0 items-center gap-2">
        <span className="mr-1 text-xs text-ink/40">Движок</span>
        {engines.map((e, i) => (
          <span key={e} className={`pill h-8! px-3! ${(result ? result.engine === e : i === 0) ? "active" : "bg-ink/5!"}`}>
            {(result ? result.engine === e : i === 0) && <span className="dot" />}
            {engineName(e)}
          </span>
        ))}
        <div className="flex-1" />
        <span className="text-xs text-ink/35">{settings?.hotkeyPopup ?? "Ctrl+Alt+T"} — перевести выделенное</span>
      </div>
    </div>
  );
}

function History({ favorites, onPick }: { favorites: boolean; onPick: (text: string) => void }) {
  const reduce = useReducedMotion();
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<Entry[]>([]);
  const [confirmClear, setConfirmClear] = useState(false);
  const [error, setError] = useState("");
  const [exported, setExported] = useState("");

  const load = () => api.history(query, favorites).then(setItems).catch((e) => setError(errorText(e)));
  useEffect(() => { const t = window.setTimeout(load, 150); return () => window.clearTimeout(t); }, [query, favorites]);

  async function toggle(e: Entry) {
    await api.setFavorite(e.id, !e.isFavorite);
    load();
  }
  async function remove(e: Entry) { await api.deleteEntry(e.id); load(); }
  async function exportCsv() {
    try { setExported(await api.exportFavorites()); window.setTimeout(() => setExported(""), 6000); }
    catch (e) { setError(errorText(e)); }
  }
  async function clear() {
    if (!confirmClear) { setConfirmClear(true); window.setTimeout(() => setConfirmClear(false), 3000); return; }
    await api.clearHistory(); setConfirmClear(false); load();
  }

  const fmt = (ts: number) => {
    const d = new Date(ts * 1000);
    const today = new Date();
    const sameDay = d.toDateString() === today.toDateString();
    return sameDay ? d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" }) : d.toLocaleDateString("ru-RU", { day: "numeric", month: "short" });
  };

  return (
    <div className="card flex h-full flex-col gap-2 rounded-[20px] p-3">
      <div className="relative flex items-center gap-2">
        <div className="field flex flex-1 items-center gap-2">
          <Icon name="search" className="opacity-50" />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={favorites ? "Поиск в избранном" : "Поиск в истории"} className="flex-1 bg-transparent outline-none placeholder:text-ink/30" />
        </div>
        {!favorites && items.length > 0 && (
          <button className={`pill ${confirmClear ? "text-err" : ""}`} onClick={clear}><Icon name="trash" size={15} className="opacity-70" />{confirmClear ? "Точно очистить?" : "Очистить"}</button>
        )}
        {favorites && items.length > 0 && (
          <button className="pill" onClick={exportCsv} title="Сохранить CSV в Загрузки"><Icon name="copy" size={15} className="opacity-70" />Экспорт CSV</button>
        )}
      </div>
      {error && <div className="px-2 text-sm text-err">{error}</div>}
      {exported && <div className="px-2 text-sm text-ok">Сохранено: {exported}</div>}
      <div className="relative min-h-0 flex-1 overflow-auto">
        {items.length === 0 ? (
          <div className="flex h-full items-center justify-center text-ink/30">{query ? "Ничего не найдено" : favorites ? "Отметьте перевод звёздочкой, и он появится здесь" : "История пуста. Выделите текст и нажмите хоткей"}</div>
        ) : (
          <AnimatePresence initial={false}>
            {items.map((e, i) => (
              <motion.div
                key={e.id}
                style={{ overflow: "hidden" }}
                initial={i < 12 ? { opacity: 0, y: reduce ? 0 : 6 } : false}
                animate={{ opacity: 1, y: 0, transition: { duration: reduce ? 0.15 : 0.2, ease: EASE_OUT, delay: i < 12 ? i * 0.03 : 0 } }}
                exit={{ opacity: 0, height: 0, transition: { duration: 0.15 } }}
              >
                <div className="row flex items-center gap-3 rounded-xl px-2.5 py-1.5" onDoubleClick={() => onPick(e.sourceText)}>
                  <span className="w-11 shrink-0 text-xs text-ink/40">{fmt(e.createdAt)}</span>
                  <span className="shrink-0 rounded-full bg-ink/7 px-2 py-0.5 text-[11px] font-medium tracking-wider text-ink/80">{e.sourceLang.toUpperCase()} → {e.targetLang.toUpperCase()}</span>
                  <span className="w-[38%] shrink-0 truncate text-[13px]" title={e.sourceText}>{e.sourceText}</span>
                  <span className="min-w-0 flex-1 truncate text-[13px] text-ink/50" title={e.resultText}>{e.resultText}</span>
                  <button className={`rb h-7! w-7! ${e.isFavorite ? "warm" : ""}`} onClick={() => toggle(e)} title="В избранное"><Icon name="star" size={14} /></button>
                  <button className="rb h-7! w-7!" onClick={() => api.copy(e.resultText)} title="Копировать перевод"><Icon name="copy" size={14} /></button>
                  <button className="rb h-7! w-7!" onClick={() => remove(e)} title="Удалить"><Icon name="trash" size={14} /></button>
                </div>
              </motion.div>
            ))}
          </AnimatePresence>
        )}
      </div>
    </div>
  );
}

const langs = ["ru", "en", "uk", "de", "fr", "es", "it", "pt", "pl", "tr", "ja", "ko", "zh"];
const THEMES = [
  { id: "system", label: "Системная" },
  { id: "dark", label: "Тёмная" },
  { id: "light", label: "Светлая" },
];

// Компоненты вынесены из SettingsPanel: объявленные внутри, они пересоздавались на каждый рендер и поле теряло фокус после первого символа.
function Row({ label, children }: { label: string; children: ReactNode }) {
  return <div className="flex items-center gap-3 py-1.5"><span className="w-56 text-sm text-ink/70">{label}</span>{children}</div>;
}
function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button onClick={onClick} className={`toggle-track ${on ? "on" : ""}`}>
      <span className={`toggle-thumb ${on ? "on" : ""}`} />
    </button>
  );
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
              <span className="text-sm text-ink/70">Доступна {info?.version}</span>
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

function SettingsPanel() {
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
    <div className="card flex h-full flex-col gap-1 overflow-auto rounded-[20px] p-5">
      <div className="mb-1 flex items-center gap-2">
        <span className="text-xs font-medium uppercase tracking-wider text-ink/40">Хоткеи</span>
        <span className="text-xs text-ok" style={{ opacity: saved ? 1 : 0, transition: "opacity 150ms ease" }}>Сохранено</span>
        {saveError && <span className="text-xs text-err">{saveError}</span>}
      </div>
      <Row label="Перевести в попап"><HotkeyField value={s.hotkeyPopup} status={statusOf("hotkeyPopup")} onCommit={(v) => set("hotkeyPopup", v)} /></Row>
      <Row label="Заменить выделенное"><HotkeyField value={s.hotkeyReplace} status={statusOf("hotkeyReplace")} onCommit={(v) => set("hotkeyReplace", v)} /></Row>
      <Row label="Открыть окно"><HotkeyField value={s.hotkeyWindow} status={statusOf("hotkeyWindow")} onCommit={(v) => set("hotkeyWindow", v)} /></Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink/40">Языки</div>
      <Row label="Переводить на">
        <select className="field w-44" value={s.primaryLang} onChange={(e) => set("primaryLang", e.target.value)}>{langs.map((l) => <option key={l} value={l}>{langName(l)}</option>)}</select>
      </Row>
      <Row label="Если текст уже на этом языке — на">
        <select className="field w-44" value={s.secondaryLang} onChange={(e) => set("secondaryLang", e.target.value)}>{langs.map((l) => <option key={l} value={l}>{langName(l)}</option>)}</select>
      </Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink/40">Движки</div>
      <Row label="Порядок цепочки">
        <div className="flex gap-2">
          {s.engines.map((e, i) => (
            <button key={e} className="pill" title="Сделать первым" onClick={() => set("engines", [e, ...s.engines.filter((x) => x !== e)])}>{i === 0 && <span className="dot" />}{engineName(e)}</button>
          ))}
        </div>
      </Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink/40">Общее</div>
      <Row label="Тема">
        <div className="segbar flex gap-0.5 p-[3px]">
          {THEMES.map((t) => (
            <button key={t.id} className={`seg ${s.theme === t.id ? "active" : ""}`} onClick={() => set("theme", t.id)}>
              {s.theme === t.id && <motion.div layoutId="theme-pill" className="tab-pill" transition={T_RESIZE} />}
              <span className="relative">{t.label}</span>
            </button>
          ))}
        </div>
      </Row>
      <Row label="Запускать вместе с Windows"><Toggle on={autostart} onClick={toggleAutostart} /></Row>
      <Row label="Вести историю"><Toggle on={s.historyEnabled} onClick={() => set("historyEnabled", !s.historyEnabled)} /></Row>
      <Row label="Размер текста в попапе">
        <input type="number" min={12} max={24} className="field w-24" value={s.fontSize} onChange={(e) => set("fontSize", Number(e.target.value) || 16)} />
      </Row>
      <div className="mb-1 mt-4 text-xs font-medium uppercase tracking-wider text-ink/40">О программе</div>
      <About />
    </div>
  );
}
