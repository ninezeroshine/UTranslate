import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { currentMonitor, LogicalSize, PhysicalPosition } from "@tauri-apps/api/window";
import { listen, win } from "../lib/tauri";
import { AnimatePresence, motion, useReducedMotion, type Transition, type Variants } from "motion/react";
import { api, engineLabel, errorHint, errorText, posName, speak, type Settings, type TranslateResult } from "../lib/api";
import { Icon } from "../lib/icons";
import { applyTheme } from "../lib/theme";
import { FavoriteController } from "../main/latestRequest";
import { Badge, IconButton, Pill } from "../ui";

type Status = "loading" | "result" | "error" | "input";
type Show = {
  text: string;
  target: string;
  detected: string | null;
  clipboardReplaced: boolean;
  requestId: number;
  canReplace: boolean;
};
type PopupError = { message: string; requestId: number };
type ReplaceContext = { requestId: number; sourceText: string };
type ReplaceState = "idle" | "pending" | "done" | "failed";
type ReplaceFocusGuard = {
  attempt: number;
  requestId: number;
  phase: "pending" | "recovery";
};
/** Подтверждение замены текста: пилюля на 2 секунды. overlay — попап уже на экране, окно не наше. */
type Toast = { text: string; overlay: boolean };
/** Ошибка на карточке: заголовок и строка «что делать». */
type ErrorState = { title: string; hint: string };

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
  const toastTimer = useRef<number | undefined>(undefined);

  const pinnedRef = useRef(false);
  pinnedRef.current = pinned;
  const cardRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pillRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const debounce = useRef<number | undefined>(undefined);
  const localRequestRef = useRef(0);
  const backendRequestRef = useRef<number | null>(null);
  const replaceContextRef = useRef<ReplaceContext | null>(null);
  const replaceAttemptRef = useRef(0);
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

  function hideNow() {
    window.clearTimeout(debounce.current);
    localRequestRef.current += 1;
    backendRequestRef.current = null;
    replaceAttemptRef.current += 1;
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    setHidden(true);
    win?.hide();
  }

  function closeAnimated() {
    window.clearTimeout(debounce.current);
    localRequestRef.current += 1;
    backendRequestRef.current = null;
    replaceAttemptRef.current += 1;
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
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
    // Обычный поток сильнее тоста: таймер отменяется, пилюля пропадает сразу.
    window.clearTimeout(toastTimer.current);
    window.clearTimeout(debounce.current);
    localRequestRef.current += 1;
    backendRequestRef.current = payload.requestId;
    replaceAttemptRef.current += 1;
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    replaceContextRef.current = payload.canReplace && payload.text
      ? { requestId: payload.requestId, sourceText: payload.text }
      : null;
    favoriteControllerRef.current.clear();
    setToast(null);
    setToastOut(false);
    setSession((n) => n + 1);
    setHidden(false);
    setClosing(false);
    wasExpandedRef.current = false;
    winSizeRef.current = { w: CARD_W + MARGIN * 2, h: CARD_H_DEFAULT + MARGIN * 2 };
    pendingFitRef.current = null;
    setResult(null); setError(null); setFavorite(false); setShowOriginal(false); setCopied(false);
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    setSource(payload.text); setTarget(payload.target); setDetected(payload.detected); setClipNote(payload.clipboardReplaced);
    if (payload.text) {
      setInputMode(false); setStatus("loading"); setExpanded(false);
      setBox({ w: pillWRef.current, h: PILL_H });
      window.setTimeout(() => setExpanded(true), 180);
    } else {
      setInputMode(true); setInput(""); setStatus("input"); setExpanded(true);
      setBox({ w: CARD_W, h: inputHRef.current });
      window.setTimeout(() => inputRef.current?.focus(), 50);
    }
  }

  function handleError(message: string) {
    setError({ title: errorText(message), hint: errorHint(message) });
    setStatus("error");
    setExpanded(true);
  }

  useEffect(() => {
    api.getSettings().then((s) => { setSettings(s); applyTheme(s.theme); }).catch(() => undefined);
    const subs = [
      listen<Show>("popup:show", ({ payload }) => handleShow(payload)),
      listen<TranslateResult>("popup:result", ({ payload }) => {
        if (payload.requestId === backendRequestRef.current) applyResult(payload, payload.requestId);
      }),
      listen<PopupError>("popup:error", ({ payload }) => {
        if (payload.requestId === backendRequestRef.current) handleError(payload.message);
      }),
      listen<Toast>("popup:toast", ({ payload }) => handleToast(payload)),
      // Настройки правятся в главном окне — попап узнаёт о смене темы и шрифта отсюда.
      listen<Settings>("settings:changed", ({ payload }) => { setSettings(payload); applyTheme(payload.theme); }),
      win?.onFocusChanged(({ payload: focused }) => {
        const guard = replaceFocusGuardRef.current;
        if (focused) {
          if (guard?.phase === "recovery") replaceFocusGuardRef.current = null;
          return;
        }
        if (pinnedRef.current) return;
        if (
          guard
          && guard.attempt === replaceAttemptRef.current
          && guard.requestId === replaceContextRef.current?.requestId
        ) return;

        const attempt = replaceAttemptRef.current;
        const requestId = replaceContextRef.current?.requestId ?? null;
        void win?.isFocused().then((isFocused) => {
          if (
            isFocused
            || pinnedRef.current
            || replacePendingRef.current
            || attempt !== replaceAttemptRef.current
            || requestId !== (replaceContextRef.current?.requestId ?? null)
          ) return;
          hideNow();
        });
      }),
    ];
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") hideNow(); };
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
    favoriteControllerRef.current.accept(r.historyId, r.isFavorite);
    const context = replaceContextRef.current;
    replacePendingRef.current = false;
    setReplaceRequestId(context?.requestId === replacementRequestId ? replacementRequestId : null);
    setReplaceState("idle");
    setReplaceError(null);
    setResult(r); setDetected(r.detected); setTarget(r.target); setFavorite(r.isFavorite); setStatus("result"); setExpanded(true);
  }

  async function translateNow(text: string, to?: string, replacementRequestId: number | null = null) {
    window.clearTimeout(debounce.current);
    debounce.current = undefined;
    if (!text.trim()) return;
    favoriteControllerRef.current.clear();
    backendRequestRef.current = null;
    replaceAttemptRef.current += 1;
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    const request = ++localRequestRef.current;
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    setStatus("loading"); setError(null);
    try {
      const translated = await api.translate(text, to);
      if (request === localRequestRef.current) applyResult(translated, replacementRequestId);
    } catch (e) {
      if (request === localRequestRef.current) {
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
  useLayoutEffect(measure, [expanded, status, result, showOriginal, input, error, clipNote, session, replaceState, replaceError]);
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

  function replacementRequestFor(text: string) {
    const context = replaceContextRef.current;
    return !inputMode && context?.sourceText === text ? context.requestId : null;
  }

  function onInput(v: string) {
    setInput(v);
    window.clearTimeout(debounce.current);
    favoriteControllerRef.current.clear();
    backendRequestRef.current = null;
    localRequestRef.current += 1;
    replaceAttemptRef.current += 1;
    replacePendingRef.current = false;
    replaceFocusGuardRef.current = null;
    replaceContextRef.current = null;
    setReplaceRequestId(null); setReplaceState("idle"); setReplaceError(null);
    setResult(null); setFavorite(false); setStatus("input");
    if (!v.trim()) return;
    debounce.current = window.setTimeout(() => translateNow(v), 600);
  }

  function switchTarget() {
    if (!settings) return;
    const next = target === settings.primaryLang ? settings.secondaryLang : settings.primaryLang;
    setTarget(next);
    translateNow(currentText, next, replacementRequestFor(currentText));
  }

  async function toggleFavorite() {
    if (!result?.historyId) return;
    const historyId = result.historyId;
    const next = !favorite;
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
    if (!result) return;
    api.copy(result.text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1200); });
  }

  const canReplace = status === "result"
    && result !== null
    && replaceContextRef.current !== null
    && replaceRequestId === replaceContextRef.current.requestId
    && source === replaceContextRef.current.sourceText
    && replaceState === "idle";

  async function replaceSelection() {
    const context = replaceContextRef.current;
    if (!canReplace || !context || !result || replaceState !== "idle" || replacePendingRef.current) return;
    replacePendingRef.current = true;
    const attempt = ++replaceAttemptRef.current;
    replaceFocusGuardRef.current = { attempt, requestId: context.requestId, phase: "pending" };
    setReplaceState("pending");
    setReplaceError(null);
    try {
      await api.replacePopupTranslation(context.requestId, context.sourceText, result.text);
      if (attempt === replaceAttemptRef.current) setReplaceState("done");
    } catch (e) {
      if (attempt !== replaceAttemptRef.current) return;
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

  // Действия переносятся целыми группами: на узкой рабочей области ни одна кнопка не обрезается.
  const actions = (
    <div className="popup-footer">
      <div className="popup-footer-actions">
      <Pill
        variant="water"
        size="md"
        disabled={!canReplace}
        aria-label="Заменить"
        title={result ? `Заменить на «${result.text}»` : "Дождитесь результата перевода"}
        onClick={replaceSelection}
      >
        {replaceState === "pending"
          ? "Заменяем…"
          : replaceState === "done"
            ? "Заменено"
            : replaceState === "failed"
              ? "Выделите заново"
              : "Заменить"}
      </Pill>
      {status === "result" && result && (<>
      <IconButton
        icon={copied ? "check" : "copy"}
        label={copied ? "Скопировано" : "Копировать"}
        size={36}
        onClick={copy}
      />
      {!result.wordMode && (
        <IconButton icon="speaker" label="Озвучить" size={36} onClick={() => speak(result.text, result.target)} />
      )}
      <IconButton
        icon="star"
        label="В избранное"
        size={36}
        tone="sand"
        active={favorite}
        className={starPop ? "star-pop" : ""}
        onClick={toggleFavorite}
        disabled={!result.historyId}
      />
      </>)}
      </div>
      {status === "result" && result && !inputMode && !result.wordMode && (
        <button
          className="popup-original-toggle flex items-center gap-1.5 pr-1.5 text-[12px] transition-colors"
          style={{ color: showOriginal ? "var(--water)" : "var(--ink-3)" }}
          aria-expanded={showOriginal}
          onClick={() => setShowOriginal((s) => !s)}
        >
          Оригинал
          <Icon name="chevron" size={12} className={`chevron ${showOriginal ? "rotate-180" : ""}`} />
        </button>
      )}
      {replaceError && <div className="popup-replace-error" role="alert">{replaceError}</div>}
    </div>
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
          <div className="flex items-center gap-2.5">
            <div ref={pillRef} className={`langpill ${expanded ? "on" : ""}`}>
              <span className={`dot ${detected ? "" : "hollow"} ${status === "loading" ? "pulse" : ""}`} />
              <span className="text-[12px] font-semibold tracking-[0.07em] text-ink-2">{(detected ?? "auto").toUpperCase()}</span>
              <Icon name="arrow" size={14} className="text-ink-3" />
              <button
                className="text-[12px] font-semibold tracking-[0.07em] text-water"
                onClick={switchTarget}
                title="Сменить целевой язык"
              >
                {target.toUpperCase()}
              </button>
            </div>
            <AnimatePresence>
              {expanded && (
                <motion.div
                  key="hdr"
                  variants={headerFx}
                  initial="hidden"
                  animate="visible"
                  exit="exit"
                  style={{ transformOrigin: "right center" }}
                  className="flex min-w-0 flex-1 items-center gap-2.5"
                >
                  {status === "loading" && <span className="text-[12px] text-ink-3">переводим…</span>}
                  {status === "error" && <span className="text-[12px] text-err">не удалось</span>}
                  {status === "result" && result && (
                    <Badge tone={result.fallbackFrom ? "sand" : "neutral"} title={result.fallbackFrom ?? undefined}>
                      {engineLabel(result)}
                    </Badge>
                  )}
                  <div className="flex-1" />
                  <div className="flex gap-1.5">
                    <IconButton icon="pin" label="Закрепить" active={pinned} onClick={() => setPinned((p) => !p)} />
                    <IconButton icon="expand" label="Открыть в окне" onClick={() => api.openMain(currentText || undefined)} />
                    <IconButton icon="close" label="Закрыть (Esc)" onClick={closeAnimated} />
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
          </div>

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
                    placeholder="Введите текст для перевода…"
                    rows={3}
                    className="field w-full resize-none py-2.5 leading-relaxed"
                    style={{ height: "auto", minHeight: 76, fontSize: 15 }}
                  />
                  <div className="flex items-center gap-2 text-[11px] text-ink-3">
                    <span>Enter — перевести, Shift+Enter — новая строка</span>
                    <div className="flex-1" />
                    <span>{input.length} / 5000</span>
                  </div>
                </div>
              )}

              {clipNote && <div className="px-1 text-[12px] text-warn">Буфер обмена содержал не текст и был заменён.</div>}

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
                      <div className="flex items-center gap-2">
                        <Pill
                          variant="water"
                          size="md"
                          icon="refresh"
                          onClick={() => translateNow(currentText, target, replacementRequestFor(currentText))}
                        >
                          Повторить
                        </Pill>
                        <Pill size="md" onClick={() => api.openMain(currentText || undefined)}>Открыть окно</Pill>
                      </div>
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
                            <span className="select-text text-[20px] font-semibold tracking-[-0.01em] text-water">{result.text}</span>
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
                          <AnimatePresence initial={false}>
                            {showOriginal && (
                              <motion.div
                                key="orig"
                                data-popup-original
                                variants={fx()}
                                initial="hidden"
                                animate="visible"
                                exit="exit"
                                className="select-text border-b border-line pb-[11px] text-[13px] leading-[1.55] text-ink-3"
                                style={{ maxHeight: 120, overflow: "auto" }}
                              >
                                {source}
                              </motion.div>
                            )}
                          </AnimatePresence>
                          <div
                            className="select-text"
                            style={{ fontSize, lineHeight: 1.5, letterSpacing: "-0.005em", maxHeight: 300, overflow: "auto", textWrap: "pretty" }}
                          >
                            {result.text}
                          </div>
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
