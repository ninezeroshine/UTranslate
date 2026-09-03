import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { LogicalSize } from "@tauri-apps/api/window";
import { listen, win } from "../lib/tauri";
import { AnimatePresence, motion, useReducedMotion, type Transition, type Variants } from "motion/react";
import { api, engineLabel, errorHint, errorText, posName, speak, type Settings, type TranslateResult } from "../lib/api";
import { Icon } from "../lib/icons";
import { applyTheme } from "../lib/theme";
import { Badge, IconButton, Pill } from "../ui";

type Status = "loading" | "result" | "error" | "input";
type Show = { text: string; target: string; detected: string | null; clipboardReplaced: boolean };
/** Подтверждение замены текста: пилюля на 2 секунды. overlay — попап уже на экране, окно не наше. */
type Toast = { text: string; overlay: boolean };
/** Ошибка на карточке: заголовок и строка «что делать». */
type ErrorState = { title: string; hint: string };

// Геометрия — см. docs/motion.md. MARGIN совпадает с src-tauri/src/popup.rs.
const CARD_W = 430;
const CARD_H_DEFAULT = 260;
const MARGIN = 64;
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
  const [hidden, setHidden] = useState(true);
  const [closing, setClosing] = useState(false);
  const [session, setSession] = useState(0);
  const [toast, setToast] = useState<Toast | null>(null);
  const [toastOut, setToastOut] = useState(false);
  const toastTimer = useRef<number | undefined>(undefined);

  const pinnedRef = useRef(false);
  pinnedRef.current = pinned;
  const cardRef = useRef<HTMLDivElement>(null);
  const pillRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const debounce = useRef<number | undefined>(undefined);
  // true, если в этой сессии капсула уже была развёрнута — отличает морфинг пилюли от обычного resize.
  const wasExpandedRef = useRef(false);
  // Размеры прошлого показа: с ними новая сессия монтируется сразу в нужной форме.
  // Без них капсула появлялась в размере прошлой карточки и съезжала в пилюлю на глазах.
  const pillWRef = useRef(160);
  const inputHRef = useRef(190);
  const inputModeRef = useRef(false);
  inputModeRef.current = inputMode;
  // Текущий размер окна на стороне Rust — чтобы не дёргать innerSize() на каждое изменение.
  const winSizeRef = useRef({ w: CARD_W + MARGIN * 2, h: CARD_H_DEFAULT + MARGIN * 2 });
  const pendingShrinkRef = useRef<{ w: number; h: number } | null>(null);

  function hideNow() {
    setHidden(true);
    win?.hide();
  }

  function closeAnimated() {
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
    setToast(null);
    setToastOut(false);
    setSession((n) => n + 1);
    setHidden(false);
    setClosing(false);
    wasExpandedRef.current = false;
    winSizeRef.current = { w: CARD_W + MARGIN * 2, h: CARD_H_DEFAULT + MARGIN * 2 };
    pendingShrinkRef.current = null;
    setResult(null); setError(null); setFavorite(false); setShowOriginal(false); setPinned(false); setCopied(false);
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
      listen<TranslateResult>("popup:result", ({ payload }) => applyResult(payload)),
      listen<{ message: string }>("popup:error", ({ payload }) => handleError(payload.message)),
      listen<Toast>("popup:toast", ({ payload }) => handleToast(payload)),
      // Настройки правятся в главном окне — попап узнаёт о смене темы и шрифта отсюда.
      listen<Settings>("settings:changed", ({ payload }) => { setSettings(payload); applyTheme(payload.theme); }),
      win?.onFocusChanged(({ payload: focused }) => { if (!focused && !pinnedRef.current) hideNow(); }),
    ];
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") hideNow(); };
    window.addEventListener("keydown", onKey);
    // Отладка вёрстки в обычном браузере: window.__utDemo.show({...}) / .result({...}) / .error("…")
    (window as unknown as { __utDemo: unknown }).__utDemo = { show: handleShow, result: applyResult, error: handleError };
    return () => {
      subs.forEach((p) => p?.then((un) => un()));
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  function applyResult(r: TranslateResult) {
    setResult(r); setDetected(r.detected); setTarget(r.target); setFavorite(false); setStatus("result"); setExpanded(true);
  }

  async function translateNow(text: string, to?: string) {
    if (!text.trim()) return;
    setStatus("loading"); setError(null);
    try { applyResult(await api.translate(text, to)); }
    catch (e) { setError({ title: errorText(e), hint: errorHint(e) }); setStatus("error"); }
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
    if (inputModeRef.current) inputHRef.current = el.offsetHeight;
    setBox({ w: CARD_W, h: el.offsetHeight });
  };
  useLayoutEffect(measure, [expanded, status, result, showOriginal, input, error, clipNote, session]);
  // Шрифты и перенос строк доезжают позже коммита — следим за реальной высотой.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [session]);

  // Рост окна — сразу вместе с box; ужатие — только в onAnimationComplete внешней капсулы.
  function fitWindow(cardW: number, cardH: number) {
    const targetW = cardW + MARGIN * 2;
    const targetH = cardH + MARGIN * 2;
    const cur = winSizeRef.current;
    if (targetW > cur.w || targetH > cur.h) {
      const next = { w: Math.max(targetW, cur.w), h: Math.max(targetH, cur.h) };
      pendingShrinkRef.current = null;
      winSizeRef.current = next;
      void win?.setSize(new LogicalSize(next.w, next.h));
    } else if (targetW < cur.w || targetH < cur.h) {
      pendingShrinkRef.current = { w: targetW, h: targetH };
    }
  }
  // Тост в своём окне живёт в размере, который выставил Rust, — карточку тут не меряем.
  const soloToast = toast !== null && !toast.overlay;
  useEffect(() => { if (!hidden && !soloToast) fitWindow(box.w, box.h); }, [box.w, box.h, hidden, soloToast]);

  function onCapsuleAnimationComplete() {
    if (closing) { setClosing(false); hideNow(); return; }
    const p = pendingShrinkRef.current;
    if (p) {
      pendingShrinkRef.current = null;
      winSizeRef.current = p;
      void win?.setSize(new LogicalSize(p.w, p.h));
    }
  }

  const currentText = inputMode ? input : source;

  function onInput(v: string) {
    setInput(v);
    window.clearTimeout(debounce.current);
    if (!v.trim()) { setResult(null); setStatus("input"); return; }
    debounce.current = window.setTimeout(() => translateNow(v), 600);
  }

  function switchTarget() {
    if (!settings) return;
    const next = target === settings.primaryLang ? settings.secondaryLang : settings.primaryLang;
    setTarget(next);
    translateNow(currentText, next);
  }

  async function toggleFavorite() {
    if (!result?.historyId) return;
    const next = !favorite;
    setFavorite(next);
    if (next) { setStarPop(true); window.setTimeout(() => setStarPop(false), 240); }
    try { await api.setFavorite(result.historyId, next); } catch { setFavorite(!next); }
  }

  function copy() {
    if (!result) return;
    api.copy(result.text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1200); });
  }

  const fontSize = settings?.fontSize ?? 21;

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
        ...(toast.overlay ? { position: "absolute" as const, left: MARGIN + 12, top: MARGIN + 12, zIndex: 2, pointerEvents: "none" as const } : {}),
      }}
      initial={false}
      animate={{ opacity: toastOut ? 0 : 1 }}
      transition={{ duration: 0.18, ease: EASE_OUT }}
    >
      <Icon name="check" size={15} className="text-water" />
      Заменено: {toast.text}
    </motion.div>
  );

  if (soloToast) return <div style={{ padding: MARGIN }} className="h-full w-full">{toastNode}</div>;

  // Ряд действий под переводом: копировать, озвучить, избранное, раскрытие оригинала.
  const actions = result && (
    <div className="flex items-center gap-2">
      <Pill variant="water" size="md" onClick={copy}>
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={copied ? "done" : "copy"}
            initial={{ opacity: 0, filter: "blur(2px)" }}
            animate={{ opacity: 1, filter: "blur(0px)" }}
            exit={{ opacity: 0, filter: "blur(2px)" }}
            transition={{ duration: reduce ? 0.15 : 0.18 }}
            className="flex items-center gap-1.5"
          >
            <Icon name={copied ? "check" : "copy"} size={15} />
            {copied ? "Скопировано" : "Копировать"}
          </motion.span>
        </AnimatePresence>
      </Pill>
      {!result.wordMode && (
        <Pill size="md" icon="speaker" onClick={() => speak(result.text, result.target)}>Озвучить</Pill>
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
      <div className="flex-1" />
      {!inputMode && !result.wordMode && (
        <button
          className="flex items-center gap-1.5 pr-1.5 text-[12px] transition-colors"
          style={{ color: showOriginal ? "var(--water)" : "var(--ink-3)" }}
          aria-expanded={showOriginal}
          onClick={() => setShowOriginal((s) => !s)}
        >
          Оригинал
          <Icon name="chevron" size={12} className={`chevron ${showOriginal ? "rotate-180" : ""}`} />
        </button>
      )}
    </div>
  );

  return (
    <div
      style={{ padding: MARGIN, position: "relative" }}
      className="h-full w-full"
      onMouseDown={(e) => { if (e.target === e.currentTarget) hideNow(); }}
    >
      <motion.div
        key={session}
        className="pop overflow-hidden"
        role="dialog"
        aria-label="UTranslate — перевод"
        initial={false}
        animate={{ width: box.w, height: box.h, borderRadius: expanded ? 26 : 999, opacity: closing ? 0 : 1, scale: closing ? 0.97 : 1 }}
        transition={{ width: capsuleTransition, height: capsuleTransition, borderRadius: capsuleTransition, opacity: exitT, scale: exitT }}
        onAnimationComplete={onCapsuleAnimationComplete}
      >
        <div
          ref={cardRef}
          style={{ width: CARD_W, padding: expanded ? PAD_CARD : PAD_PILL }}
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
            <div className="flex flex-col gap-[13px]" aria-live="polite">
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
                        <Pill variant="water" size="md" icon="refresh" onClick={() => translateNow(currentText, target)}>Повторить</Pill>
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
                      {actions}
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
            </div>
          )}
        </div>
      </motion.div>
      {toast?.overlay && toastNode}
    </div>
  );
}
