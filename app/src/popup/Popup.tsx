import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { currentMonitor, LogicalSize, PhysicalPosition } from "@tauri-apps/api/window";
import { listen, win } from "../lib/tauri";
import { AnimatePresence, motion, useReducedMotion, type Transition, type Variants } from "motion/react";
import { api, engineName, errorHint, errorText, posName, speak, type Settings, type TranslateResult } from "../lib/api";
import { Icon } from "../lib/icons";
import { applyTheme } from "../lib/theme";
import { FavoriteController, LatestRequest } from "../main/latestRequest";
import { IconButton, Pill } from "../ui";
import { EngineMenu } from "./EngineMenu";
import { PopupFooter } from "./PopupFooter";
import { PopupHeader } from "./PopupHeader";
import { SourceBlock } from "./SourceBlock";
import { TranslationEditor } from "./TranslationEditor";
import type { PopupOrigin, PopupStatus as Status, ReplaceState } from "./types";

type Show = {
  text: string;
  target: string;
  detected: string | null;
  clipboardReplaced: boolean;
  requestId: number;
  canReplace: boolean;
  /** Optional while older backends/tests are still emitting the legacy payload. */
  origin?: PopupOrigin;
  phase?: "recognizing" | "translating";
};
type PopupError = { message: string; requestId: number };
type ScreenRecognized = { requestId: number; text: string; target: string; detected: string | null };
type CaptureLifecycle = { requestId: number };
type ReplaceContext = { requestId: number; sourceText: string };
type ReplaceFocusGuard = {
  attempt: number;
  requestId: number;
  phase: "pending" | "recovery";
};
/** Подтверждение замены текста: пилюля на 2 секунды. overlay — попап уже на экране, окно не наше. */
type Toast = { text: string; overlay: boolean };
/** Ошибка на карточке: заголовок и строка «что делать». */
type ErrorState = { title: string; hint: string };
type EditSaveState = "idle" | "saving" | "error";

// Геометрия — см. docs/motion.md. MARGIN совпадает с src-tauri/src/popup.rs.
const CARD_W = 430;
const CARD_H_DEFAULT = 260;
const MARGIN = 64;
const COMPACT_MARGIN = 16;
const COMPACT_WORK_AREA_H = 520;
const TOAST_MS = 2000;
// Поле карточки 14 (design/bento). В свёрнутой капсуле оно 4 по горизонтали и 6 по вертикали:
// пилюля языков сама даёт свои 14, поэтому содержимое отступает на 18, а капсула выходит 46 высотой.
const PAD_CARD = 14;
const PAD_PILL = "6px 4px";
const PILL_H = 46;
const KNOWN_ENGINES = new Set(["google", "bing", "mymemory"]);

const EASE_OUT = [0.23, 1, 0.32, 1] as const;
const T_ENTER: Transition = { duration: 0.22, ease: EASE_OUT };
const T_EXIT: Transition = { duration: 0.12, ease: EASE_OUT };
const S_MORPH: Transition = { type: "spring", duration: 0.42, bounce: 0.12 };
const S_RESIZE: Transition = { type: "spring", duration: 0.3, bounce: 0 };
const T_REDUCED: Transition = { duration: 0.15 };

export default function Popup() {
  const reduce = useReducedMotion();
  const enterT = reduce ? T_REDUCED : T_ENTER;
  const exitT = reduce ? T_REDUCED : T_EXIT;
  const morphT = reduce ? T_REDUCED : S_MORPH;
  const resizeT = reduce ? T_REDUCED : S_RESIZE;

  // Блоки тела: opacity + blur + сдвиг на вход, только opacity + blur на выход.
  const fx = (offset = 6, blur = 4): Variants => ({
    hidden: { opacity: 0, y: reduce ? 0 : offset, filter: reduce ? "none" : `blur(${blur}px)` },
    visible: { opacity: 1, y: 0, filter: "blur(0px)", transition: enterT },
    exit: { opacity: 0, filter: reduce ? "none" : `blur(${blur}px)`, transition: exitT },
  });
  // Шапка выезжает от правого края, как у Dynamic Island.
  const headerFx: Variants = {
    hidden: { opacity: 0, scale: reduce ? 1 : 0.9 },
    visible: { opacity: 1, scale: 1, transition: { ...enterT, delay: reduce ? 0 : 0.06 } },
    exit: { opacity: 0, scale: reduce ? 1 : 0.9, transition: exitT },
  };

  const [status, setStatus] = useState<Status>("input");
  const [origin, setOrigin] = useState<PopupOrigin>("selection");
  const [expanded, setExpanded] = useState(false);
  const [inputMode, setInputMode] = useState(false);
  const [source, setSource] = useState("");
  const [input, setInput] = useState("");
  const [target, setTarget] = useState("ru");
  const [detected, setDetected] = useState<string | null>(null);
  const [result, setResult] = useState<TranslateResult | null>(null);
  const [error, setError] = useState<ErrorState | null>(null);
  const [pinned, setPinned] = useState(false);
  const [showOriginal, setShowOriginal] = useState(false);
  const [favorite, setFavorite] = useState(false);
  const [starPop, setStarPop] = useState(false);
  const [copied, setCopied] = useState(false);
  const [clipNote, setClipNote] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [box, setBox] = useState({ w: 160, h: PILL_H });
  const [maxCardHeight, setMaxCardHeight] = useState<number | null>(null);
  const [frameMargin, setFrameMargin] = useState(MARGIN);
  const [hidden, setHidden] = useState(true);
  const [closing, setClosing] = useState(false);
  const [session, setSession] = useState(0);
  const [toast, setToast] = useState<Toast | null>(null);
  const [toastOut, setToastOut] = useState(false);
  const [replaceRequestId, setReplaceRequestId] = useState<number | null>(null);
  const [replaceState, setReplaceState] = useState<ReplaceState>("idle");
  const [replaceError, setReplaceError] = useState<string | null>(null);
  const [editMode, setEditMode] = useState(false);
  const [draft, setDraft] = useState("");
  const [editSaveState, setEditSaveState] = useState<EditSaveState>("idle");
  const [editError, setEditError] = useState<string | null>(null);
  const [engineMenuOpen, setEngineMenuOpen] = useState(false);
  const [selectedEngine, setSelectedEngine] = useState<string | undefined>(undefined);
  const [enginePending, setEnginePending] = useState(false);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [screenActionPending, setScreenActionPending] = useState(false);
  const [screenActionError, setScreenActionError] = useState<string | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);

  const pinnedRef = useRef(false);
  pinnedRef.current = pinned;
  const cardRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pillRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const editRef = useRef<HTMLTextAreaElement>(null);
  const engineButtonRef = useRef<HTMLButtonElement>(null);
  const debounce = useRef<number | undefined>(undefined);
  // Четыре независимых «побеждает последний»: перевод из попапа, сохранение правки,
  // попытка замены выделения и запуск выбора области экрана.
  const requests = useRef({
    local: new LatestRequest(),
    edit: new LatestRequest(),
    replace: new LatestRequest(),
    screen: new LatestRequest(),
  }).current;
  const selectedEngineRef = useRef<string | undefined>(undefined);
  const backendRequestRef = useRef<number | null>(null);
  const statusRef = useRef<Status>(status);
  statusRef.current = status;
  const originRef = useRef<PopupOrigin>(origin);
  originRef.current = origin;
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;
  const captureSuspendRef = useRef<number | null>(null);
  const captureWasVisibleRef = useRef(false);
  const capturePreviousStatusRef = useRef<Status>("input");
  const replaceContextRef = useRef<ReplaceContext | null>(null);
  const replacePendingRef = useRef(false);
  const replaceFocusGuardRef = useRef<ReplaceFocusGuard | null>(null);
  const favoriteControllerRef = useRef(new FavoriteController());
  // true, если в этой сессии капсула уже была развёрнута — отличает морфинг пилюли от обычного resize.
  const wasExpandedRef = useRef(false);
  // Размеры прошлого показа: с ними новая сессия монтируется сразу в нужной форме.
  // Без них капсула появлялась в размере прошлой карточки и съезжала в пилюлю на глазах.
  const pillWRef = useRef(160);
  const inputHRef = useRef(190);
  const inputModeRef = useRef(false);
  inputModeRef.current = inputMode;
  const winSizeRef = useRef({ w: CARD_W + MARGIN * 2, h: CARD_H_DEFAULT + MARGIN * 2 });
  const pendingFitRef = useRef<{ w: number; h: number } | null>(null);
  const fittingRef = useRef(false);
  const editModeRef = useRef(false);
  editModeRef.current = editMode;
  const engineMenuOpenRef = useRef(false);
  engineMenuOpenRef.current = engineMenuOpen;
  const editSaveStateRef = useRef<EditSaveState>("idle");
  editSaveStateRef.current = editSaveState;
  const committedTextRef = useRef("");
  committedTextRef.current = result?.text ?? "";

  function hideNow() {
    window.clearTimeout(debounce.current);
    requests.local.invalidate();
    requests.edit.invalidate();
    backendRequestRef.current = null;
    requests.replace.invalidate();
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    hiddenRef.current = true;
    requests.screen.invalidate();
    setScreenActionPending(false);
    setHidden(true);
    win?.hide();
  }

  function closeAnimated() {
    window.clearTimeout(debounce.current);
    requests.local.invalidate();
    requests.edit.invalidate();
    backendRequestRef.current = null;
    requests.replace.invalidate();
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    requests.screen.invalidate();
    setScreenActionPending(false);
    setClosing(true);
  }

  /** Тост исчезает opacity за --dur-fast, потом прячем окно. Своё окно тоста — прячем целиком,
   *  пилюлю поверх открытого попапа — только её. */
  function handleToast(t: Toast) {
    window.clearTimeout(toastTimer.current);
    setToast(t);
    setToastOut(false);
    if (!t.overlay) setHidden(false);
    toastTimer.current = window.setTimeout(() => {
      setToastOut(true);
      toastTimer.current = window.setTimeout(() => {
        setToast(null);
        setToastOut(false);
        if (!t.overlay) hideNow();
      }, 180);
    }, TOAST_MS);
  }

  function handleShow(payload: Show) {
    const nextOrigin = payload.origin ?? "selection";
    const recognizing = nextOrigin === "screen" && payload.phase === "recognizing";
    originRef.current = nextOrigin;
    // Обычный поток сильнее тоста: таймер отменяется, пилюля пропадает сразу.
    window.clearTimeout(toastTimer.current);
    window.clearTimeout(debounce.current);
    requests.local.invalidate();
    requests.edit.invalidate();
    backendRequestRef.current = payload.requestId;
    captureSuspendRef.current = null;
    requests.screen.invalidate();
    requests.replace.invalidate();
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    replaceContextRef.current = nextOrigin !== "screen" && payload.canReplace && payload.text
      ? { requestId: payload.requestId, sourceText: payload.text }
      : null;
    favoriteControllerRef.current.clear();
    setToast(null);
    setToastOut(false);
    setSession((n) => n + 1);
    hiddenRef.current = false;
    setHidden(false);
    setClosing(false);
    wasExpandedRef.current = false;
    winSizeRef.current = { w: CARD_W + MARGIN * 2, h: CARD_H_DEFAULT + MARGIN * 2 };
    pendingFitRef.current = null;
    setResult(null); setError(null); setFavorite(false); setShowOriginal(false); setCopied(false);
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    setEditMode(false); setDraft(""); setEditSaveState("idle"); setEditError(null);
    setEngineMenuOpen(false); setSelectedEngine(undefined); setEnginePending(false); setEngineError(null);
    setScreenActionPending(false); setScreenActionError(null); setOrigin(nextOrigin);
    selectedEngineRef.current = undefined;
    setSource(payload.text); setTarget(payload.target); setDetected(payload.detected); setClipNote(payload.clipboardReplaced);
    if (recognizing) {
      statusRef.current = "recognizing";
      setInputMode(false); setInput(""); setStatus("recognizing"); setExpanded(false);
      setBox({ w: pillWRef.current, h: PILL_H });
    } else if (payload.text) {
      statusRef.current = "loading";
      setInputMode(false); setStatus("loading"); setExpanded(false);
      setBox({ w: pillWRef.current, h: PILL_H });
      window.setTimeout(() => setExpanded(true), 180);
    } else {
      statusRef.current = "input";
      setInputMode(true); setInput(""); setStatus("input"); setExpanded(true);
      setBox({ w: CARD_W, h: inputHRef.current });
      window.setTimeout(() => inputRef.current?.focus(), 50);
    }
  }

  function handleRecognized(payload: ScreenRecognized) {
    if (
      hiddenRef.current
      || originRef.current !== "screen"
      || statusRef.current !== "recognizing"
      || payload.requestId !== backendRequestRef.current
    ) return;
    setSource(payload.text);
    setTarget(payload.target);
    setDetected(payload.detected);
    setInputMode(false);
    statusRef.current = "loading";
    setStatus("loading");
    setExpanded(true);
  }

  function invalidateForScreenCapture() {
    window.clearTimeout(debounce.current);
    debounce.current = undefined;
    backendRequestRef.current = null;
    requests.local.invalidate();
    requests.edit.invalidate();
    requests.replace.invalidate();
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    replaceContextRef.current = null;
    favoriteControllerRef.current.clear();
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    // Keep the visible unsaved translation draft; only invalidate its in-flight save generation.
    setEditSaveState("idle"); setEditError(null);
    setEngineMenuOpen(false); setEnginePending(false); setEngineError(null);
    setScreenActionError(null);
  }

  function handleCaptureSuspend(payload: CaptureLifecycle) {
    // This ref must change before the ACK can move focus from the webview to native overlays.
    captureSuspendRef.current = payload.requestId;
    captureWasVisibleRef.current = !hiddenRef.current;
    capturePreviousStatusRef.current = statusRef.current;
    requests.screen.invalidate();
    setScreenActionPending(false);
    setScreenActionError(null);
    invalidateForScreenCapture();
    void api.ackScreenCapture(payload.requestId).catch(() => undefined);
  }

  function handleCaptureResume(payload: CaptureLifecycle) {
    if (captureSuspendRef.current !== payload.requestId) return;
    captureSuspendRef.current = null;
    const wasVisible = captureWasVisibleRef.current;
    captureWasVisibleRef.current = false;
    if (!wasVisible) return;
    hiddenRef.current = false;
    setHidden(false);
    if (["loading", "recognizing"].includes(capturePreviousStatusRef.current)) {
      setError({
        title: "Выбор области отменён",
        hint: "Предыдущий запрос остановлен. Повторите его или выберите другую область экрана.",
      });
      statusRef.current = "error";
      setStatus("error");
      setExpanded(true);
    }
  }

  async function requestScreenCapture() {
    if (screenActionPending) return;
    const request = requests.screen.begin();
    setScreenActionPending(true);
    setScreenActionError(null);
    try {
      await api.translateScreen();
    } catch (e) {
      if (requests.screen.isCurrent(request)) setScreenActionError(errorText(e));
    } finally {
      if (requests.screen.isCurrent(request)) setScreenActionPending(false);
    }
  }

  function handleError(message: string) {
    setError({ title: errorText(message), hint: errorHint(message) });
    statusRef.current = "error";
    setStatus("error");
    setExpanded(true);
  }

  useEffect(() => {
    api.getSettings().then((s) => { setSettings(s); applyTheme(s.theme); }).catch(() => undefined);
    const subs = [
      listen<Show>("popup:show", ({ payload }) => handleShow(payload)),
      listen<ScreenRecognized>("popup:recognized", ({ payload }) => handleRecognized(payload)),
      listen<TranslateResult>("popup:result", ({ payload }) => {
        if (!hiddenRef.current && payload.requestId === backendRequestRef.current) applyResult(payload, payload.requestId);
      }),
      listen<PopupError>("popup:error", ({ payload }) => {
        if (!hiddenRef.current && payload.requestId === backendRequestRef.current) handleError(payload.message);
      }),
      listen<CaptureLifecycle>("popup:capture-suspend", ({ payload }) => handleCaptureSuspend(payload)),
      listen<CaptureLifecycle>("popup:capture-resume", ({ payload }) => handleCaptureResume(payload)),
      listen<Toast>("popup:toast", ({ payload }) => handleToast(payload)),
      // Настройки правятся в главном окне — попап узнаёт о смене темы и шрифта отсюда.
      listen<Settings>("settings:changed", ({ payload }) => { setSettings(payload); applyTheme(payload.theme); }),
      win?.onFocusChanged(({ payload: focused }) => {
        if (captureSuspendRef.current !== null) return;
        const guard = replaceFocusGuardRef.current;
        if (focused) {
          if (guard?.phase === "recovery") replaceFocusGuardRef.current = null;
          return;
        }
        if (pinnedRef.current) return;
        if (
          guard
          && requests.replace.isCurrent(guard.attempt)
          && guard.requestId === replaceContextRef.current?.requestId
        ) return;

        const attempt = requests.replace.token();
        const requestId = replaceContextRef.current?.requestId ?? null;
        void win?.isFocused().then((isFocused) => {
          if (
            isFocused
            || pinnedRef.current
            || replacePendingRef.current
            || !requests.replace.isCurrent(attempt)
            || requestId !== (replaceContextRef.current?.requestId ?? null)
          ) return;
          hideNow();
        });
      }),
    ];
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (engineMenuOpenRef.current) {
        e.preventDefault();
        setEngineMenuOpen(false);
        window.requestAnimationFrame(() => engineButtonRef.current?.focus());
        return;
      }
      if (editModeRef.current) {
        e.preventDefault();
        if (editSaveStateRef.current === "saving") return;
        requests.edit.invalidate();
        setDraft(committedTextRef.current);
        setEditMode(false);
        setEditSaveState("idle");
        setEditError(null);
        return;
      }
      hideNow();
    };
    window.addEventListener("keydown", onKey);
    // Отладка вёрстки в обычном браузере: window.__utDemo.show({...}) / .result({...}) / .error("…")
    (window as unknown as { __utDemo: unknown }).__utDemo = {
      show: handleShow,
      result: (value: TranslateResult) => applyResult(value, value.requestId),
      error: handleError,
    };
    return () => {
      favoriteControllerRef.current.clear();
      subs.forEach((p) => p?.then((un) => un()));
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  function applyResult(r: TranslateResult, replacementRequestId: number | null = null) {
    requests.edit.invalidate();
    favoriteControllerRef.current.accept(r.historyId, r.isFavorite);
    const context = replaceContextRef.current;
    replacePendingRef.current = false;
    setReplaceRequestId(context?.requestId === replacementRequestId ? replacementRequestId : null);
    setReplaceState("idle");
    setReplaceError(null);
    setDraft(r.text); setEditMode(false); setEditSaveState("idle"); setEditError(null);
    setEnginePending(false); setEngineError(null); setEngineMenuOpen(false);
    statusRef.current = "result";
    setResult(r); setDetected(r.detected); setTarget(r.target); setFavorite(r.isFavorite); setStatus("result"); setExpanded(true);
  }

  async function translateNow(text: string, to?: string, replacementRequestId: number | null = null) {
    window.clearTimeout(debounce.current);
    debounce.current = undefined;
    if (!text.trim()) return;
    favoriteControllerRef.current.clear();
    backendRequestRef.current = null;
    requests.replace.invalidate();
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    requests.edit.invalidate();
    const request = requests.local.begin();
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    setEditMode(false); setEditSaveState("idle"); setEditError(null);
    setEngineMenuOpen(false); setEnginePending(false); setEngineError(null);
    statusRef.current = "loading";
    setStatus("loading"); setError(null);
    try {
      const translated = await api.translate(text, to, selectedEngineRef.current);
      if (requests.local.isCurrent(request)) applyResult(translated, replacementRequestId);
    } catch (e) {
      if (requests.local.isCurrent(request)) {
        statusRef.current = "error";
        setError({ title: errorText(e), hint: errorHint(e) }); setStatus("error");
      }
    }
  }

  // Размер капсулы: пилюля или карточка по фактической высоте содержимого.
  const expandedRef = useRef(false);
  expandedRef.current = expanded;
  const measure = () => {
    if (!expandedRef.current) {
      const w = (pillRef.current?.offsetWidth ?? 152) + 8;
      pillWRef.current = w;
      setBox({ w, h: PILL_H });
      return;
    }
    const el = cardRef.current;
    if (!el) return;
    const scroll = scrollRef.current;
    const naturalHeight = scroll ? el.offsetHeight - scroll.clientHeight + scroll.scrollHeight : el.offsetHeight;
    if (inputModeRef.current) inputHRef.current = naturalHeight;
    setBox({ w: CARD_W, h: naturalHeight });
  };
  useLayoutEffect(measure, [
    expanded, status, result, showOriginal, input, error, clipNote, session,
    replaceState, replaceError, editMode, draft, editSaveState, editError,
    engineMenuOpen, enginePending, engineError,
  ]);
  // Шрифты и перенос строк доезжают позже коммита — следим за реальной высотой.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [session]);

  // Все resize/reposition идут через одну очередь. Поэтому быстрый result→loading→result
  // не может завершиться устаревшим сжатием от loading.
  function fitWindow(cardW: number, cardH: number) {
    pendingFitRef.current = { w: cardW, h: cardH };
    if (fittingRef.current) return;
    fittingRef.current = true;
    void (async () => {
      try {
        while (pendingFitRef.current) {
          const requested = pendingFitRef.current;
          pendingFitRef.current = null;
          if (!win) {
            const margin = window.innerHeight < COMPACT_WORK_AREA_H ? COMPACT_MARGIN : MARGIN;
            setFrameMargin(margin);
            setMaxCardHeight(Math.max(PILL_H, window.innerHeight - margin * 2));
            continue;
          }
          const [monitor, position] = await Promise.all([currentMonitor(), win.outerPosition()]);
          if (pendingFitRef.current) continue;
          const scale = monitor?.scaleFactor ?? await win.scaleFactor();
          const work = monitor?.workArea;
          const maxWindowH = work ? work.size.height / scale : requested.h + MARGIN * 2;
          const margin = maxWindowH < COMPACT_WORK_AREA_H ? COMPACT_MARGIN : MARGIN;
          const maxCardH = Math.max(PILL_H, maxWindowH - margin * 2);
          setFrameMargin(margin);
          setMaxCardHeight(maxCardH);
          const next = {
            w: requested.w + margin * 2,
            h: Math.min(requested.h, maxCardH) + margin * 2,
          };
          await win.setSize(new LogicalSize(next.w, next.h));
          winSizeRef.current = next;
          if (work) {
            const widthPx = next.w * scale;
            const heightPx = next.h * scale;
            const left = work.position.x;
            const top = work.position.y;
            const right = left + work.size.width;
            const bottom = top + work.size.height;
            const x = Math.round(Math.min(Math.max(position.x, left), Math.max(left, right - widthPx)));
            const y = Math.round(Math.min(Math.max(position.y, top), Math.max(top, bottom - heightPx)));
            await win.setPosition(new PhysicalPosition(x, y));
          }
        }
      } finally {
        fittingRef.current = false;
        if (pendingFitRef.current) fitWindow(pendingFitRef.current.w, pendingFitRef.current.h);
      }
    })();
  }
  // Тост в своём окне живёт в размере, который выставил Rust, — карточку тут не меряем.
  const soloToast = toast !== null && !toast.overlay;
  useEffect(() => { if (!hidden && !soloToast) fitWindow(box.w, box.h); }, [box.w, box.h, hidden, soloToast]);

  function onCapsuleAnimationComplete() {
    if (closing) { setClosing(false); hideNow(); }
  }

  const currentText = inputMode ? input : source;
  const visibleTranslation = editMode ? draft : (result?.text ?? "");
  const hasVisibleTranslation = visibleTranslation.trim().length > 0;
  const editDirty = result !== null && draft !== result.text;
  const controlsBusy = editSaveState === "saving" || enginePending || status === "recognizing";

  function beginEdit() {
    if (!result || enginePending || replaceState === "pending") return;
    setDraft(result.text);
    setEditError(null);
    setEditSaveState("idle");
    setEngineMenuOpen(false);
    setEditMode(true);
    const focusEditor = () => window.requestAnimationFrame(() => {
      const field = editRef.current;
      if (!field) return;
      field.focus();
      field.setSelectionRange(field.value.length, field.value.length);
    });
    focusEditor();
    if (win) void win.setFocus().then(focusEditor).catch(() => undefined);
  }

  function cancelEdit() {
    if (editSaveState === "saving") return;
    requests.edit.invalidate();
    setDraft(result?.text ?? "");
    setEditMode(false);
    setEditSaveState("idle");
    setEditError(null);
  }

  async function saveEditedTranslation(): Promise<TranslateResult | null> {
    if (!result || !hasVisibleTranslation || editSaveState === "saving") return null;
    const nextText = draft;
    if (nextText === result.text) {
      setEditMode(false);
      setEditError(null);
      return result;
    }

    const request = requests.edit.begin();
    const sourceText = currentText;
    setEditSaveState("saving");
    setEditError(null);
    setEngineMenuOpen(false);
    try {
      const actualFavorite = result.historyId === null
        ? result.isFavorite
        : await api.updateTranslationText(result.historyId, sourceText, result.text, nextText);
      if (!requests.edit.isCurrent(request)) return null;
      const updated = { ...result, text: nextText, isFavorite: actualFavorite };
      favoriteControllerRef.current.accept(updated.historyId, actualFavorite);
      setResult(updated);
      setDraft(nextText);
      setFavorite(actualFavorite);
      setEditMode(false);
      setEditSaveState("idle");
      setEditError(null);
      return updated;
    } catch (e) {
      if (!requests.edit.isCurrent(request)) return null;
      setEditSaveState("error");
      setEditError(`Не удалось сохранить перевод: ${errorText(e)}. Исправленный текст не потерян.`);
      return null;
    }
  }

  function toggleEngineMenu() {
    if (!result || controlsBusy || editMode || replaceState === "pending") return;
    const opening = !engineMenuOpen;
    setEngineMenuOpen(opening);
    if (!opening) return;
    const request = requests.local.token();
    void api.getSettings().then((fresh) => {
      if (requests.local.isCurrent(request)) setSettings(fresh);
    }).catch(() => undefined);
  }

  async function chooseEngine(engine: string | undefined) {
    if (!result || controlsBusy || editMode || replaceState === "pending" || !currentText.trim()) return;
    setSelectedEngine(engine);
    selectedEngineRef.current = engine;
    setEngineMenuOpen(false);
    setEnginePending(true);
    setEngineError(null);
    backendRequestRef.current = null;
    const request = requests.local.begin();
    try {
      const translated = await api.translate(currentText, target, engine);
      if (requests.local.isCurrent(request)) applyResult(translated, replacementRequestFor(currentText));
    } catch (e) {
      if (!requests.local.isCurrent(request)) return;
      setEnginePending(false);
      setEngineError(`Не удалось перевести через ${engine ? engineName(engine) : "автоматический выбор"}: ${errorText(e)}.`);
    }
  }

  function replacementRequestFor(text: string) {
    const context = replaceContextRef.current;
    return !inputMode && context?.sourceText === text ? context.requestId : null;
  }

  function onInput(v: string) {
    if (originRef.current === "screen") setSource(v);
    else setInput(v);
    setSelectedEngine(undefined);
    selectedEngineRef.current = undefined;
    window.clearTimeout(debounce.current);
    favoriteControllerRef.current.clear();
    backendRequestRef.current = null;
    requests.local.invalidate();
    requests.edit.invalidate();
    requests.replace.invalidate();
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    replaceContextRef.current = null;
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    setEditMode(false); setDraft(""); setEditSaveState("idle"); setEditError(null);
    setEngineMenuOpen(false); setEnginePending(false); setEngineError(null);
    setScreenActionError(null);
    statusRef.current = "input";
    setResult(null); setFavorite(false); setStatus("input");
    if (!v.trim()) return;
    debounce.current = window.setTimeout(() => translateNow(v), 600);
  }

  function switchTarget() {
    if (!settings || controlsBusy || editMode || replaceState === "pending") return;
    const next = target === settings.primaryLang ? settings.secondaryLang : settings.primaryLang;
    setTarget(next);
    translateNow(currentText, next, replacementRequestFor(currentText));
  }

  async function toggleFavorite() {
    if (!result?.historyId || !hasVisibleTranslation || controlsBusy) return;
    const savesDirtyDraft = editMode && editDirty;
    const activeResult = savesDirtyDraft ? await saveEditedTranslation() : result;
    if (!activeResult?.historyId) return;
    const historyId = activeResult.historyId;
    const next = !(savesDirtyDraft ? activeResult.isFavorite : favorite);
    setFavorite(next);
    if (next) { setStarPop(true); window.setTimeout(() => setStarPop(false), 240); }
    const rollback = await favoriteControllerRef.current.mutate(
      historyId,
      next,
      () => api.setFavorite(historyId, next),
    );
    if (rollback !== null) setFavorite(rollback);
  }

  function copy() {
    if (!result || !hasVisibleTranslation || controlsBusy) return;
    api.copy(visibleTranslation).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1200); });
  }

  const canReplace = status === "result"
    && result !== null
    && origin !== "screen"
    && replaceContextRef.current !== null
    && replaceRequestId === replaceContextRef.current.requestId
    && source === replaceContextRef.current.sourceText
    && replaceState === "idle"
    && hasVisibleTranslation
    && !controlsBusy;

  async function replaceSelection() {
    const context = replaceContextRef.current;
    if (!canReplace || !context || !result || replaceState !== "idle" || replacePendingRef.current) return;
    replacePendingRef.current = true;
    const attempt = requests.replace.begin();
    replaceFocusGuardRef.current = { attempt, requestId: context.requestId, phase: "pending" };
    setReplaceState("pending");
    setReplaceError(null);
    try {
      await api.replacePopupTranslation(context.requestId, context.sourceText, visibleTranslation);
      if (requests.replace.isCurrent(attempt)) setReplaceState("done");
    } catch (e) {
      if (!requests.replace.isCurrent(attempt)) return;
      replacePendingRef.current = false;
      const guard = replaceFocusGuardRef.current;
      if (guard?.attempt === attempt && guard.requestId === context.requestId) {
        guard.phase = "recovery";
      }
      setReplaceState("failed");
      setReplaceError(`Не удалось заменить текст: ${errorText(e)}. Выделите текст заново и вызовите перевод своим хоткеем.`);
    }
  }

  const fontSize = settings?.fontSize ?? 21;
  const availableEngines = (settings?.engines ?? []).filter((id) => KNOWN_ENGINES.has(id));
  const visibleBoxHeight = expanded && maxCardHeight !== null ? Math.min(box.h, maxCardHeight) : box.h;

  // Морфинг пилюля→карточка — только на первом раскрытии сессии, дальше это обычный resize.
  const capsuleTransition = expanded && !wasExpandedRef.current ? morphT : resizeT;
  useLayoutEffect(() => { wasExpandedRef.current = expanded; }, [expanded]);

  if (hidden) return null;

  // Пилюля тоста — та же капсула .pop, что и состояние загрузки: одно существо, разное содержимое.
  // Появление мгновенное (действие с клавиатуры), уход — только opacity, поэтому reduced-motion не трогаем.
  const toastNode = toast && (
    <motion.div
      className="pop flex items-center gap-2.5"
      role="status"
      style={{
        width: "fit-content", height: PILL_H, borderRadius: 999, padding: "0 18px",
        fontSize: 13, fontWeight: 500, whiteSpace: "nowrap",
        ...(toast.overlay ? { position: "absolute" as const, left: frameMargin + 12, top: frameMargin + 12, zIndex: 2, pointerEvents: "none" as const } : {}),
      }}
      initial={false}
      animate={{ opacity: toastOut ? 0 : 1 }}
      transition={{ duration: 0.18, ease: EASE_OUT }}
    >
      <Icon name="check" size={15} className="text-water" />
      Заменено: {toast.text}
    </motion.div>
  );

  if (soloToast) return <div style={{ padding: frameMargin }} className="h-full w-full">{toastNode}</div>;

  // «Оригинал» есть, когда есть что показать: для экрана это правка распознанного текста,
  // для выделения — исходник (в словарном режиме слово и так на карточке).
  const canShowOriginal = status === "result" && result !== null && !inputMode
    && (showOriginal || source.trim().length > 0)
    && (origin === "screen" || !result.wordMode);
  // Распознанный текст остаётся на экране и во время перевода правки: иначе поле теряет фокус.
  const sourceBlockShown = origin === "screen"
    ? showOriginal && status !== "recognizing"
    : showOriginal && status === "result" && result !== null && !result.wordMode;

  const translationEditor = editMode && result && (
    <TranslationEditor
      editorRef={editRef}
      value={draft}
      saving={editSaveState === "saving"}
      disabled={controlsBusy || replaceState === "pending"}
      canSave={hasVisibleTranslation}
      error={editError}
      onChange={(value) => {
        setDraft(value);
        setCopied(false);
        setEditError(null);
        if (editSaveState === "error") setEditSaveState("idle");
      }}
      onCancel={cancelEdit}
      onSave={() => { void saveEditedTranslation(); }}
    />
  );

  const actions = (
    <PopupFooter
      origin={origin}
      status={status}
      result={result}
      hasVisibleTranslation={hasVisibleTranslation}
      visibleTranslation={visibleTranslation}
      controlsBusy={controlsBusy}
      canReplace={canReplace}
      replaceState={replaceState}
      replaceError={replaceError}
      screenActionPending={screenActionPending}
      screenActionError={screenActionError}
      screenHotkeyHint={settings?.hotkeyScreen ?? "Ctrl+Alt+S"}
      favorite={favorite}
      starPop={starPop}
      copied={copied}
      editMode={editMode}
      showOriginal={showOriginal}
      canShowOriginal={canShowOriginal}
      onReplace={replaceSelection}
      onScreenCapture={() => { void requestScreenCapture(); }}
      onCopy={copy}
      onSpeak={() => { if (result) speak(visibleTranslation, result.target); }}
      onFavorite={() => { void toggleFavorite(); }}
      onToggleEdit={() => { if (editMode) cancelEdit(); else beginEdit(); }}
      onToggleOriginal={() => setShowOriginal((shown) => !shown)}
    />
  );

  return (
    <div
      style={{ padding: frameMargin, position: "relative" }}
      className="h-full w-full"
      onMouseDown={(e) => { if (e.target === e.currentTarget) hideNow(); }}
    >
      <motion.div
        key={session}
        className="pop overflow-hidden"
        role="dialog"
        aria-label="UTranslate — перевод"
        initial={false}
        animate={{ width: box.w, height: visibleBoxHeight, borderRadius: expanded ? 26 : 999, opacity: closing ? 0 : 1, scale: closing ? 0.97 : 1 }}
        transition={{ width: capsuleTransition, height: capsuleTransition, borderRadius: capsuleTransition, opacity: exitT, scale: exitT }}
        onAnimationComplete={onCapsuleAnimationComplete}
      >
        <div
          ref={cardRef}
          style={{
            width: CARD_W,
            height: expanded && maxCardHeight !== null && box.h > maxCardHeight ? maxCardHeight : undefined,
            padding: expanded ? PAD_CARD : PAD_PILL,
          }}
          className="flex flex-col gap-[13px]"
        >
          <PopupHeader
            containerRef={pillRef}
            engineButtonRef={engineButtonRef}
            expanded={expanded}
            status={status}
            origin={origin}
            detected={detected}
            target={target}
            result={result}
            engineMenuOpen={engineMenuOpen}
            locked={controlsBusy || editMode || replaceState === "pending"}
            pinned={pinned}
            headerFx={headerFx}
            onSwitchTarget={switchTarget}
            onToggleEngineMenu={toggleEngineMenu}
            onTogglePin={() => setPinned((p) => !p)}
            onOpenMain={() => api.openMain(currentText || undefined)}
            onClose={closeAnimated}
          />

          <EngineMenu
            open={expanded && engineMenuOpen && status === "result" && result !== null}
            engines={availableEngines}
            selected={selectedEngine}
            variants={fx(3, 2)}
            onChoose={chooseEngine}
          />

          {expanded && (
            <div className="flex min-h-0 flex-1 flex-col gap-[13px]" aria-live="polite">
              <div ref={scrollRef} data-popup-scroll className="popup-scroll min-h-0 flex-1">
                <div className="flex flex-col gap-[13px]">
              {inputMode && (
                <div className="flex flex-col gap-[9px] px-1">
                  <textarea
                    ref={inputRef}
                    value={input}
                    onChange={(e) => onInput(e.target.value)}
                    onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); window.clearTimeout(debounce.current); translateNow(input); } }}
                    disabled={controlsBusy || editMode || replaceState === "pending"}
                    placeholder="Введите текст для перевода…"
                    rows={3}
                    className="field w-full resize-none py-2.5 leading-relaxed"
                    style={{ height: "auto", minHeight: 76, fontSize: 15 }}
                  />
                  <div className="flex items-center gap-2 text-[11px] text-ink-3">
                    <span>Enter — перевести, Shift+Enter — новая строка</span>
                    <div className="flex-1" />
                    <Pill
                      icon="screen"
                      disabled={screenActionPending}
                      onClick={() => { void requestScreenCapture(); }}
                      title={`${settings?.hotkeyScreen ?? "Ctrl+Alt+S"} — перевести область экрана`}
                    >
                      {screenActionPending ? "Открываем…" : "С экрана"}
                    </Pill>
                    <span>{input.length} / 5000</span>
                  </div>
                  {screenActionError && <div className="popup-inline-error" role="alert">{screenActionError}</div>}
                </div>
              )}

              <SourceBlock
                shown={sourceBlockShown}
                origin={origin}
                source={source}
                disabled={controlsBusy || editMode}
                variants={fx()}
                onChange={onInput}
              />

              {clipNote && <div className="px-1 text-[12px] text-warn">Буфер обмена содержал не текст и был заменён.</div>}
              {enginePending && <div className="px-1 text-[12px] text-ink-3" role="status">Переводим через выбранный движок…</div>}
              {engineError && <div className="popup-inline-error" role="alert">{engineError} Выберите другой движок или повторите.</div>}

              <div style={{ position: "relative" }}>
                <AnimatePresence mode="popLayout" initial={false}>
                  {status === "loading" && (
                    <motion.div key="skeleton" variants={fx()} initial="hidden" animate="visible" exit="exit" className="flex flex-col gap-2.5 px-1 pb-2 pt-1.5">
                      <div className="sk w-[92%]" /><div className="sk w-[78%]" /><div className="sk w-[40%]" />
                    </motion.div>
                  )}

                  {status === "error" && error && (
                    <motion.div key="error" variants={fx()} initial="hidden" animate="visible" exit="exit" className="flex flex-col gap-[13px]">
                      <div className="flex flex-col gap-[5px] px-1">
                        <span className="text-[18px] font-semibold tracking-[-0.015em]">{error.title}</span>
                        <span className="text-[13px] leading-[1.5] text-ink-2">{error.hint}</span>
                      </div>
                      <div className="flex flex-wrap items-center gap-2">
                        {currentText.trim() && <Pill
                          variant="water"
                          size="md"
                          icon="refresh"
                          onClick={() => translateNow(currentText, target, replacementRequestFor(currentText))}
                        >
                          Повторить
                        </Pill>}
                        <Pill
                          size="md"
                          icon="screen"
                          disabled={screenActionPending}
                          onClick={() => { void requestScreenCapture(); }}
                        >
                          {screenActionPending
                            ? "Открываем…"
                            : origin === "screen" ? "Выделить заново" : "С экрана"}
                        </Pill>
                        <Pill size="md" onClick={() => api.openMain(currentText || undefined)}>Открыть окно</Pill>
                      </div>
                      {screenActionError && <div className="popup-inline-error" role="alert">{screenActionError}</div>}
                    </motion.div>
                  )}

                  {status === "result" && result && (
                    <motion.div key="result" variants={fx()} initial="hidden" animate="visible" exit="exit" className="flex flex-col gap-[13px]">
                      {result.wordMode ? (
                        <>
                          <div className="flex flex-col gap-[9px] px-1">
                            <div className="flex items-center gap-2.5">
                              <span className="select-text text-[26px] font-semibold tracking-[-0.025em]">{currentText.trim()}</span>
                              <IconButton icon="speaker" label="Озвучить оригинал" size={28} onClick={() => speak(currentText, result.detected)} />
                            </div>
                            {editMode ? translationEditor : (
                              <span data-popup-translation className="select-text text-[20px] font-semibold tracking-[-0.01em] text-water">{result.text}</span>
                            )}
                          </div>
                          {result.alternatives.length > 0 && (
                            <div className="flex flex-col gap-[9px] border-t border-line px-1 pt-3">
                              {result.alternatives.slice(0, 4).map((a) => (
                                <div key={a.pos} className="flex items-start gap-2">
                                  <span className="min-w-[52px] shrink-0 pt-1 text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">{posName(a.pos)}</span>
                                  <div className="flex flex-wrap gap-1.5">
                                    {a.terms.slice(0, 6).map((t) => (
                                      <button key={t} className="chip" onClick={() => api.copy(t)} title="Скопировать">{t}</button>
                                    ))}
                                  </div>
                                </div>
                              ))}
                            </div>
                          )}
                        </>
                      ) : (
                        <div className="flex flex-col gap-[11px] px-1">
                          {editMode ? translationEditor : (
                            <div
                              data-popup-translation
                              className="select-text w-full"
                              style={{ fontSize, lineHeight: 1.5, letterSpacing: "-0.005em", maxHeight: 300, overflow: "auto", textWrap: "pretty" }}
                            >
                              {result.text}
                            </div>
                          )}
                        </div>
                      )}
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
                </div>
              </div>
              {(status === "loading" || status === "result") && actions}
            </div>
          )}
        </div>
      </motion.div>
      {toast?.overlay && toastNode}
    </div>
  );
}
