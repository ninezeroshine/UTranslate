import { useEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { listen } from "../lib/tauri";
import { api, appVersion, engineName, errorText, langName, type HotkeyStatus, type Settings, type UpdateInfo } from "../lib/api";
import { Icon, type IconName } from "../lib/icons";
import { applyTheme } from "../lib/theme";
import { Card, IconButton, Keys, Pill, Segment, Toggle } from "../ui";

// Бенто по design/bento/Settings.dc.html: плитки разного размера вместо таблицы строк.
// Что в макете помечено «план» (ключи DeepL/Gemini, статусы движков, «показывать оригинал»),
// здесь не рисуется — в коде этого нет.

const EASE_OUT = [0.23, 1, 0.32, 1] as const;
const FONT_MIN = 12;
const FONT_MAX = 24;
/** Сколько строк отдаёт history_list: больше не сосчитать, поэтому у числа появляется «+». */
const HISTORY_LIMIT = 500;
const REPO = "github.com/ninezeroshine/UTranslate";

const langs = ["ru", "en", "uk", "de", "fr", "es", "it", "pt", "pl", "tr", "ja", "ko", "zh"];

/** «1 запись, 2 записи, 5 записей» — иначе подтверждение выглядит машинным. */
function plural(n: number) {
  const t = n % 10;
  if (t === 1 && n % 100 !== 11) return "запись";
  if (t >= 2 && t <= 4 && (n % 100 < 12 || n % 100 > 14)) return "записи";
  return "записей";
}

const TONE = { ok: "text-ok", err: "text-err", warn: "text-warn" } as const;
const DOT = { ok: "bg-ok", err: "bg-err", warn: "bg-warn" } as const;

/** Строка состояния: точка и слово. Один вид у хоткеев, автосохранения и обновлений. */
function Status({ tone, children }: { tone: keyof typeof TONE; children: ReactNode }) {
  return (
    <span className={`flex min-w-0 items-center gap-[7px] text-xs ${TONE[tone]}`}>
      <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${DOT[tone]}`} />
      <span className="truncate">{children}</span>
    </span>
  );
}

/** Плитка: иконка в скруглённом квадрате, заголовок, пояснение — и содержимое под шапкой. */
function Tile({ icon, title, hint, className = "", children }: {
  icon: IconName; title: string; hint: string; className?: string; children: ReactNode;
}) {
  return (
    <Card className={`flex flex-col gap-3.5 p-[18px] ${className}`}>
      <div className="flex items-center gap-2.5">
        <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-[9px] bg-[var(--water-soft)] text-water">
          <Icon name={icon} size={15} />
        </div>
        <div className="flex min-w-0 flex-col gap-px">
          <span className="text-sm font-semibold tracking-[-0.01em]">{title}</span>
          <span className="text-[11px] text-ink-3">{hint}</span>
        </div>
      </div>
      {children}
    </Card>
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

type HotkeySuspensionLease = { id: number; ready: Promise<void> };

type HotkeySuspensionCoordinator = {
  acquire: () => HotkeySuspensionLease | null;
  release: (lease: HotkeySuspensionLease) => Promise<void>;
};

/** Один владелец на всю плитку: IPC идёт строго по очереди, а устаревший release видит новый lease. */
function createHotkeySuspensionCoordinator(): HotkeySuspensionCoordinator {
  let activeLeaseId: number | null = null;
  let nextLeaseId = 0;
  let suspended = false;
  let serial = Promise.resolve();

  function enqueue(task: () => Promise<void>): Promise<void> {
    const result = serial.then(task);
    serial = result.catch(() => undefined);
    return result;
  }

  return {
    acquire() {
      if (activeLeaseId !== null) return null;
      const id = ++nextLeaseId;
      activeLeaseId = id;
      const ready = enqueue(async () => {
        // Lease мог быть отменён до своей очереди; новый владелец сам выполнит suspend.
        if (activeLeaseId !== id) return;
        if (!suspended) {
          await api.hotkeysSuspend(true);
          suspended = true;
        }
      }).catch((error) => {
        if (activeLeaseId === id) activeLeaseId = null;
        throw error;
      });
      return { id, ready };
    },
    release(lease) {
      if (activeLeaseId === lease.id) activeLeaseId = null;
      return enqueue(async () => {
        // Между вызовом release и его местом в очереди запись могла перейти к другому полю.
        if (activeLeaseId !== null || !suspended) return;
        await api.hotkeysSuspend(false);
        suspended = false;
      });
    },
  };
}

// SettingsTab размонтируется при смене вкладки, поэтому очередь живёт столько же, сколько main webview.
const hotkeySuspensionCoordinator = createHotkeySuspensionCoordinator();

function HotkeyField({ label, value, status, coordinator, onCommit }: {
  label: string;
  value: string;
  status?: HotkeyStatus;
  coordinator: HotkeySuspensionCoordinator;
  onCommit: (next: string) => Promise<void>;
}) {
  const [recording, setRecording] = useState(false);
  const [live, setLive] = useState("");
  const [localError, setLocalError] = useState("");
  const ref = useRef<HTMLButtonElement>(null);
  const mountedRef = useRef(true);
  const recordingRef = useRef(false);
  const savingRef = useRef(false);
  const leaseRef = useRef<HotkeySuspensionLease | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      recordingRef.current = false;
      const lease = leaseRef.current;
      leaseRef.current = null;
      if (lease && !savingRef.current) void coordinator.release(lease).catch(() => undefined);
    };
  }, [coordinator]);

  function start() {
    if (recordingRef.current || leaseRef.current) return;
    const lease = coordinator.acquire();
    if (!lease) return;
    leaseRef.current = lease;
    setLive(""); setLocalError("");
    // Иначе зарегистрированный хоткей перехватит нажатие раньше, чем его увидит поле.
    void lease.ready.then(() => {
      if (mountedRef.current && leaseRef.current === lease) {
        recordingRef.current = true;
        setRecording(true);
        requestAnimationFrame(() => {
          if (recordingRef.current && leaseRef.current === lease) ref.current?.focus();
        });
      }
    }).catch(async () => {
      if (leaseRef.current === lease) leaseRef.current = null;
      if (mountedRef.current) {
        recordingRef.current = false;
        setRecording(false);
        setLocalError("Не удалось отключить хоткеи");
      }
      await coordinator.release(lease).catch(() => undefined);
    });
  }
  function cancel() {
    recordingRef.current = false;
    setRecording(false); setLive(""); setLocalError("");
    const lease = leaseRef.current;
    leaseRef.current = null;
    if (lease) void coordinator.release(lease).catch(() => {
      if (mountedRef.current) setLocalError("Не удалось включить хоткеи");
    });
  }
  async function onKeyDown(e: React.KeyboardEvent) {
    e.preventDefault();
    if (e.key === "Escape" || e.key === "Backspace" || e.key === "Delete") { cancel(); return; }
    if (e.key === "Control" || e.key === "Alt" || e.key === "Shift" || e.key === "Meta") { setLive(modsOf(e)); setLocalError(""); return; }
    const token = codeToToken(e.code);
    if (!token) return;
    const mods = modsOf(e);
    if (!mods && !isFKey(token)) { setLocalError("Нужен Ctrl, Alt, Shift или Win"); setLive(""); return; }
    const combo = mods ? `${mods}+${token}` : token;
    const lease = leaseRef.current;
    if (!lease) return;
    recordingRef.current = false;
    savingRef.current = true;
    setRecording(false); setLive(""); setLocalError("");
    try {
      await lease.ready;
      if (!mountedRef.current || leaseRef.current !== lease) return;
      await onCommit(combo);
    } catch {
      if (mountedRef.current && leaseRef.current === lease) setLocalError("Не удалось сохранить хоткей");
    } finally {
      savingRef.current = false;
      if (leaseRef.current === lease) leaseRef.current = null;
      await coordinator.release(lease).catch(() => {
        if (mountedRef.current) setLocalError("Не удалось включить хоткеи");
      });
    }
  }

  return (
    <>
      <button
        ref={ref}
        type="button"
        aria-label={`${label}: ${value}. Нажмите, чтобы записать новое сочетание`}
        className={`hkfield w-42 shrink-0 ${recording ? "rec" : ""}`}
        onClick={start}
        onKeyDown={recording ? onKeyDown : undefined}
        onBlur={() => leaseRef.current && !savingRef.current && cancel()}
      >
        {recording
          ? (live ? <><Keys combo={live} /><span>+…</span></> : <span>Нажмите сочетание…</span>)
          : <Keys combo={value} />}
      </button>
      {recording ? (
        <span className="truncate text-xs text-ink-3">
          {localError || "Esc — отмена. Глобальные хоткеи сняты."}
        </span>
      ) : localError ? (
        <Status tone="err">{localError}</Status>
      ) : status?.error ? (
        <Status tone={status.error.startsWith("Совпадает") ? "warn" : "err"}>{status.error}</Status>
      ) : (
        <Status tone="ok">Свободно</Status>
      )}
    </>
  );
}

// ---- тема: миниатюра окна ----

/** Цвета миниатюры абсолютные, не токены: превью показывает чужую тему, а не текущую. */
function MiniWindow({ dark }: { dark: boolean }) {
  const c = dark ? { bg: "#121a1e", card: "#1a2429", accent: "#63b6c6" } : { bg: "#e7ecee", card: "#f8fafa", accent: "#2f6e7c" };
  return (
    <span className="absolute inset-0" style={{ background: c.bg }}>
      <span className="absolute" style={{ left: 5, top: 5, right: 5, height: 7, borderRadius: 4, background: c.card }} />
      <span className="absolute" style={{ left: 5, top: 16, width: 24, bottom: 5, borderRadius: 5, background: c.card }} />
      <span className="absolute" style={{ left: 33, top: 16, right: 5, bottom: 5, borderRadius: 5, background: c.card }} />
      <span className="absolute" style={{ left: 36, top: 19, width: 12, height: 4, borderRadius: 2, background: c.accent }} />
    </span>
  );
}

/** Системная — кадр пополам: слева светлая половина, справа тёмная. */
function ThemeMini({ mode }: { mode: "system" | "dark" | "light" }) {
  return (
    <span className="relative block h-10 w-16 overflow-hidden rounded-[10px] border border-line">
      {mode === "system" ? (
        <>
          <span className="absolute inset-y-0 left-0 block w-8 overflow-hidden">
            <span className="absolute block h-10 w-16"><MiniWindow dark={false} /></span>
          </span>
          <span className="absolute inset-y-0 right-0 block w-8 overflow-hidden">
            <span className="absolute block h-10 w-16" style={{ left: -32 }}><MiniWindow dark /></span>
          </span>
        </>
      ) : (
        <MiniWindow dark={mode === "dark"} />
      )}
    </span>
  );
}

const THEMES: readonly { id: string; label: ReactNode }[] = ([
  { id: "system", label: "Системная" },
  { id: "dark", label: "Тёмная" },
  { id: "light", label: "Светлая" },
] as const).map(({ id, label }) => ({
  id,
  label: (
    <span className="flex flex-col items-center gap-2">
      <ThemeMini mode={id} />
      <span className="text-xs font-medium">{label}</span>
    </span>
  ),
}));

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
  const [copied, setCopied] = useState(false);

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

  /** Плагина opener в проекте нет, открыть браузер нечем — поэтому копируем ссылку. */
  function copyRepo() {
    void api.copy(`https://${REPO}`).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    });
  }

  const pct = progress.total ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : null;
  const anim = {
    initial: { opacity: 0, scale: reduce ? 1 : 0.97 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, scale: reduce ? 1 : 0.97 },
    transition: { duration: reduce ? 0.15 : 0.18, ease: EASE_OUT },
  };

  return (
    <Tile icon="info" title={`UTranslate ${version}`} hint="Обновления через GitHub, установка тихая">
      <div className="flex min-h-9 items-center" aria-live="polite">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div key={phase} {...anim} className="flex min-w-0 items-center gap-3">
            {phase === "idle" && <Pill icon="refresh" onClick={check}>Проверить обновления</Pill>}
            {phase === "checking" && <span className="pill"><span className="dot pulse" />Проверяем…</span>}
            {phase === "none" && <Status tone="ok">Установлена последняя версия</Status>}
            {phase === "found" && (
              <>
                <span className="whitespace-nowrap text-[13px] text-ink-2">Доступна {info?.version}</span>
                <Pill variant="water" onClick={install}>Обновить</Pill>
              </>
            )}
            {phase === "downloading" && <span className="text-[13px] text-ink-2">{pct === null ? "Загрузка…" : `Загрузка ${pct} %`}</span>}
            {phase === "installing" && <span className="pill"><span className="dot pulse" />Устанавливается…</span>}
            {phase === "error" && (
              <>
                <span className="truncate text-[13px] text-err">{error}</span>
                <Pill icon="refresh" onClick={installFailed ? install : check}>Повторить</Pill>
              </>
            )}
          </motion.div>
        </AnimatePresence>
      </div>

      {(phase === "downloading" || phase === "installing") && (
        <div className="flex flex-col gap-1.5">
          {/* Прогресс приходит из update:progress (lib.rs do_install); пока размер неизвестен — бегущая полоса. */}
          {pct === null
            ? <div className="sk h-[5px] rounded-full" />
            : <div className="h-[5px] overflow-hidden rounded-full bg-tile-2">
                <div className="h-full rounded-full bg-water" style={{ width: `${pct}%`, transition: "width 150ms linear" }} />
              </div>}
          <span className="text-[11px] text-ink-3">Окно закроется само на установке.</span>
        </div>
      )}

      <button type="button" className="flex items-center gap-2 self-start text-[12px] text-water" onClick={copyRepo} title="Скопировать ссылку">
        <Icon name="github" size={14} className="text-ink-3" />
        {copied ? "Ссылка скопирована" : REPO}
      </button>
      <span className="text-[11px] text-ink-3">Данные: settings.json и utranslate.db в %APPDATA%\com.utranslate.app</span>
    </Tile>
  );
}

// ---- вкладка ----

/** Настройки. onSettings отдаёт свежую копию каркасу: тему и хоткеи показывают другие вкладки. */
export function SettingsTab({ onSettings }: { onSettings: (s: Settings) => void }) {
  const reduce = useReducedMotion();
  const hotkeyCoordinator = hotkeySuspensionCoordinator;
  const [s, setS] = useState<Settings | null>(null);
  const [autostart, setAutostart] = useState(false);
  const [statuses, setStatuses] = useState<HotkeyStatus[]>([]);
  const [note, setNote] = useState<{ text: string; tone: "ok" | "err" } | null>(null);
  // Порядок движков во время перетаскивания: в настройки он уезжает один раз, на отпускании.
  const [drag, setDrag] = useState<{ id: string; y0: number; step: number; dy: number; order: string[] } | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  // Сколько записей уйдёт: считаем по списку истории, поэтому дальше лимита видно только «не меньше».
  const [clearCount, setClearCount] = useState<{ n: number; capped: boolean } | null>(null);
  const noteTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    api.getSettings().then(setS).catch(() => flash("Не удалось прочитать настройки", "err"));
    api.autostartGet().then(setAutostart).catch(() => undefined);
    // Занятый на этой машине хоткей должен быть подсвечен сразу, без ожидания правки.
    api.hotkeysStatus().then(setStatuses).catch(() => undefined);
    return () => {
      window.clearTimeout(noteTimer.current);
    };
  }, [hotkeyCoordinator]);

  /** Видимый ответ на действие: «Сохранено» гаснет быстро, ошибка висит дольше. */
  function flash(text: string, tone: "ok" | "err" = "ok") {
    setNote({ text, tone });
    window.clearTimeout(noteTimer.current);
    noteTimer.current = window.setTimeout(() => setNote(null), tone === "ok" ? 1400 : 6000);
  }

  if (!s) {
    return (
      <div className="flex min-h-0 flex-1 flex-col gap-4">
        {[96, 150, 150].map((h) => (
          <Card key={h} className="flex flex-col gap-3 p-[18px]" style={{ height: h }} aria-hidden>
            <div className="sk w-40" /><div className="sk w-full" /><div className="sk w-2/3" />
          </Card>
        ))}
        <span className="sr-only" role="status">Настройки загружаются</span>
      </div>
    );
  }
  const cur = s;

  async function persist(next: Settings) {
    setS(next);
    onSettings(next);
    // Тема применяется сразу: в попап её донесёт событие settings:changed из settings_set.
    applyTheme(next.theme);
    try {
      setStatuses(await api.setSettings(next));
      flash("Сохранено");
    } catch (e) {
      flash(`Не удалось сохранить: ${errorText(e)}`, "err");
    }
  }
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => persist({ ...cur, [k]: v });

  async function toggleAutostart() {
    const next = !autostart; setAutostart(next);
    try { await api.autostartSet(next); flash("Сохранено"); }
    catch (e) { setAutostart(!next); flash(`Не удалось сохранить: ${errorText(e)}`, "err"); }
  }

  const statusOf = (field: HotkeyStatus["field"]) => statuses.find((x) => x.field === field);

  function changePrimaryLang(next: string) {
    persist(next === cur.secondaryLang
      ? { ...cur, primaryLang: next, secondaryLang: cur.primaryLang }
      : { ...cur, primaryLang: next });
  }

  function changeSecondaryLang(next: string) {
    persist(next === cur.primaryLang
      ? { ...cur, primaryLang: cur.secondaryLang, secondaryLang: next }
      : { ...cur, secondaryLang: next });
  }

  // ---- порядок движков ----
  const engines = drag?.order ?? cur.engines;

  function moveEngine(from: number, to: number) {
    if (to < 0 || to >= cur.engines.length) return;
    const next = cur.engines.slice();
    next.splice(to, 0, ...next.splice(from, 1));
    set("engines", next);
  }

  function dragStart(e: React.PointerEvent, id: string) {
    const row = (e.currentTarget as HTMLElement).closest<HTMLElement>("[data-engine]");
    if (!row) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    // Шаг = высота строки плюс просвет между строками: на него меняется место при сдвиге.
    setDrag({ id, y0: e.clientY, step: row.offsetHeight + 2, dy: 0, order: cur.engines });
  }

  function dragMove(e: React.PointerEvent) {
    const y = e.clientY;
    setDrag((d) => {
      if (!d) return d;
      const dy = y - d.y0;
      const i = d.order.indexOf(d.id);
      const j = Math.max(0, Math.min(d.order.length - 1, i + Math.round(dy / d.step)));
      if (j === i) return { ...d, dy };
      const order = d.order.slice();
      order.splice(j, 0, ...order.splice(i, 1));
      // Строка уже переехала на новое место — точку отсчёта переносим туда же.
      return { ...d, order, y0: d.y0 + (j - i) * d.step, dy: dy - (j - i) * d.step };
    });
  }

  function dragEnd() {
    const d = drag;
    setDrag(null);
    if (d && d.order.join() !== cur.engines.join()) set("engines", d.order);
  }

  // ---- очистка истории ----

  /** Подтверждение как во вкладке «История»: вторая кнопка живёт 3 секунды. */
  async function askClear() {
    setConfirmClear(true);
    window.setTimeout(() => setConfirmClear(false), 3000);
    try {
      const rows = await api.history("", false);
      setClearCount({ n: rows.filter((r) => !r.isFavorite).length, capped: rows.length >= HISTORY_LIMIT });
    } catch { setClearCount(null); }
  }

  async function doClear() {
    setConfirmClear(false);
    try { await api.clearHistory(); flash("История очищена"); }
    catch (e) { flash(errorText(e), "err"); }
    setClearCount(null);
  }

  const clearHint = clearCount === null
    ? "Удалит записи истории, избранное останется."
    : `Удалит ${clearCount.capped ? "не меньше " : ""}${clearCount.n} ${plural(clearCount.n)}, избранное останется.`;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto overflow-x-hidden px-0.5 pb-0.5">

        <Tile icon="keyboard" title="Хоткеи" hint="Глобальные — ловятся, пока приложение живёт в трее">
          <div className="flex flex-col gap-2.5">
            {([
              ["hotkeyPopup", "Перевести в попап", cur.hotkeyPopup],
              ["hotkeyReplace", "Заменить выделенное", cur.hotkeyReplace],
              ["hotkeyWindow", "Открыть окно", cur.hotkeyWindow],
            ] as const).map(([field, label, value]) => (
              <div key={field} className="flex items-center gap-3">
                <span className="w-46 shrink-0 text-[13px] text-ink-2">{label}</span>
                <HotkeyField
                  label={label}
                  value={value}
                  status={statusOf(field)}
                  coordinator={hotkeyCoordinator}
                  onCommit={(v) => set(field, v)}
                />
              </div>
            ))}
          </div>
        </Tile>

        <div className="grid gap-4 md:grid-cols-2">
          <Tile icon="translate" title="Языки" hint="Куда переводить выделенный текст">
            <div className="flex flex-col gap-2.5">
              <div className="flex items-center gap-2.5">
                <label className="pill pl-3!">
                  <span className="text-[12px] font-semibold tracking-[0.07em] text-water">{cur.primaryLang.toUpperCase()}</span>
                  <select
                    className="appearance-none bg-transparent text-[13px] text-ink outline-none"
                    aria-label="Основной язык перевода"
                    value={cur.primaryLang}
                    onChange={(e) => changePrimaryLang(e.target.value)}
                  >
                    {langs.map((l) => <option key={l} value={l}>{langName(l)}</option>)}
                  </select>
                  <Icon name="chevron" size={12} className="text-ink-3" />
                </label>
                <span className="text-[11px] text-ink-3">основной</span>
              </div>
              <span className="text-[12px] leading-[1.45] text-ink-3">
                Если текст уже на этом языке, перевод идёт на второй.
              </span>
              <div className="flex items-center gap-2.5">
                <label className="pill pl-3!">
                  <span className="text-[12px] font-semibold tracking-[0.07em] text-mist">{cur.secondaryLang.toUpperCase()}</span>
                  <select
                    className="appearance-none bg-transparent text-[13px] text-ink outline-none"
                    aria-label="Второй язык перевода"
                    value={cur.secondaryLang}
                    onChange={(e) => changeSecondaryLang(e.target.value)}
                  >
                    {langs.map((l) => <option key={l} value={l}>{langName(l)}</option>)}
                  </select>
                  <Icon name="chevron" size={12} className="text-ink-3" />
                </label>
                <span className="text-[11px] text-ink-3">второй</span>
              </div>
            </div>
          </Tile>

          <Tile icon="theme" title="Тема" hint="Применяется сразу, попап узнаёт событием">
            {/* gap и padding оправы перебиваем важным: в Segment они заданы утилитами. */}
            <Segment items={THEMES} value={cur.theme} onChange={(id) => set("theme", id)} layoutId="theme-pill" className="cards gap-2! p-0!" />
          </Tile>
        </div>

        <Card className="grid gap-4 p-[18px] sm:grid-cols-2">
          <div className="flex items-center gap-3">
            <Toggle on={autostart} label="Запускать вместе с Windows" onClick={toggleAutostart} />
            <div className="flex min-w-0 flex-col gap-px">
              <span className="text-[13px] font-medium">Запускать вместе с Windows</span>
              <span className="text-[11px] text-ink-3">Запись в автозапуске обновляется при каждом старте</span>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <Toggle on={cur.historyEnabled} label="Вести историю" onClick={() => set("historyEnabled", !cur.historyEnabled)} />
            <div className="flex min-w-0 flex-col gap-px">
              <span className="text-[13px] font-medium">Вести историю</span>
              <span className="text-[11px] text-ink-3">SQLite рядом с настройками, только на этом компьютере</span>
            </div>
          </div>
        </Card>

        <div className="grid gap-4 md:grid-cols-[1.35fr_1fr]">
          <Tile icon="layers" title="Движки" hint="Порядок цепочки: первый отвечает, остальные — резерв">
            <div className="flex flex-col gap-0.5">
              {engines.map((e, i) => (
                <div
                  key={e}
                  data-engine
                  className="row flex items-center gap-2.5 px-2.5 py-2"
                  style={drag?.id === e
                    ? { transform: `translateY(${drag.dy}px)`, background: "var(--tile)", boxShadow: "var(--sh-card)", position: "relative", zIndex: 1 }
                    : undefined}
                >
                  <button
                    type="button"
                    className="grip"
                    aria-label={`Перетащить ${engineName(e)}`}
                    onPointerDown={(ev) => dragStart(ev, e)}
                    onPointerMove={drag ? dragMove : undefined}
                    onPointerUp={dragEnd}
                    onPointerCancel={dragEnd}
                  >
                    <Icon name="grip" size={14} className="[stroke-width:2]" />
                  </button>
                  <span className="flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-full border border-line-2 bg-tile-2 text-[11px] font-semibold text-ink-2">
                    {i + 1}
                  </span>
                  <span className="w-24 shrink-0 text-[13px] font-medium">{engineName(e)}</span>
                  {e === "mymemory" && <span className="chip h-[22px] text-[11px]">без ключа</span>}
                  <div className="flex-1" />
                  <IconButton icon="up" size={28} className="ghost" label={`Поднять ${engineName(e)}`} disabled={i === 0} onClick={() => moveEngine(i, i - 1)} />
                  <IconButton icon="down" size={28} className="ghost" label={`Опустить ${engineName(e)}`} disabled={i === engines.length - 1} onClick={() => moveEngine(i, i + 1)} />
                </div>
              ))}
            </div>
            <span className="text-[11px] text-ink-3">Перетащите строку за ручку или переставьте стрелками.</span>
          </Tile>

          <Tile icon="sliders" title="Размер текста в попапе" hint={`${FONT_MIN}–${FONT_MAX} px, применяется сразу`}>
            <div className="flex items-center gap-3">
              <div className="tile flex shrink-0 items-center gap-0.5 rounded-full p-[3px]" role="group" aria-label="Размер текста в попапе">
                {/* «minimize» — это горизонтальная черта: та же форма, что и минус. */}
                <IconButton icon="minimize" size={28} className="ghost" label="Меньше" disabled={cur.fontSize <= FONT_MIN} onClick={() => set("fontSize", cur.fontSize - 1)} />
                <span className="w-[34px] text-center text-sm font-semibold" aria-live="polite">{cur.fontSize}</span>
                <IconButton icon="plus" size={28} className="ghost" label="Больше" disabled={cur.fontSize >= FONT_MAX} onClick={() => set("fontSize", cur.fontSize + 1)} />
              </div>
              <div className="tile min-w-0 flex-1 truncate px-3 py-2" style={{ fontSize: cur.fontSize, lineHeight: 1.4 }}>
                Так выглядит перевод
              </div>
            </div>
          </Tile>
        </div>

        <div className="grid gap-4 md:grid-cols-2">
          <About />

          <Tile icon="trash" title="Опасная зона" hint="Действие необратимо, отмены нет" className="border-[color-mix(in_srgb,var(--err)_22%,transparent)]!">
            <div className="flex items-center gap-2.5">
              {confirmClear ? (
                <>
                  <Pill onClick={() => setConfirmClear(false)}>Отмена</Pill>
                  <Pill className="danger solid" onClick={doClear}>Точно очистить</Pill>
                </>
              ) : (
                <Pill icon="trash" className="danger" onClick={askClear}>Очистить историю</Pill>
              )}
            </div>
            <span className="text-[11px] text-ink-3">
              {confirmClear ? clearHint : "Удаляет историю целиком, кроме избранного. То же действие есть на вкладке «История»."}
            </span>
          </Tile>
        </div>
      </div>

      {/* Автосохранение молчит, пока всё хорошо, поэтому подтверждение показываем внизу — оно видно с любой прокрутки. */}
      <AnimatePresence>
        {note && (
          <motion.div
            key="note"
            role="status"
            className="pop absolute bottom-2 left-1/2 flex max-w-[90%] -translate-x-1/2 items-center gap-2 rounded-full px-4 py-2"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: reduce ? 0.1 : 0.18, ease: EASE_OUT }}
          >
            <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${DOT[note.tone]}`} />
            <span className={`truncate text-[13px] ${note.tone === "err" ? "text-err" : ""}`}>{note.text}</span>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
