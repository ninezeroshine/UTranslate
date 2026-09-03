import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { LogicalSize } from "@tauri-apps/api/window";
import { listen, win } from "../lib/tauri";
import { AnimatePresence, motion, useReducedMotion, type Transition, type Variants } from "motion/react";
import { api, engineLabel, errorText, speak, type Settings, type TranslateResult } from "../lib/api";
import { Icon } from "../lib/icons";

type Status = "loading" | "result" | "error" | "input";
type Show = { text: string; target: string; detected: string | null; clipboardReplaced: boolean };

// Геометрия — см. docs/motion.md. MARGIN совпадает с src-tauri/src/popup.rs.
const CARD_W = 430;
const CARD_H_DEFAULT = 260;
const MARGIN = 64;

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
  const [error, setError] = useState("");
  const [pinned, setPinned] = useState(false);
  const [showOriginal, setShowOriginal] = useState(false);
  const [favorite, setFavorite] = useState(false);
  const [starPop, setStarPop] = useState(false);
  const [copied, setCopied] = useState(false);
  const [clipNote, setClipNote] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [box, setBox] = useState({ w: 160, h: 46 });
  const [hidden, setHidden] = useState(true);
  const [closing, setClosing] = useState(false);
  const [session, setSession] = useState(0);

  const pinnedRef = useRef(false);
  pinnedRef.current = pinned;
  const cardRef = useRef<HTMLDivElement>(null);
  const pillRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const debounce = useRef<number | undefined>(undefined);
  // true, если в этой сессии капсула уже была развёрнута — отличает морфинг пилюли от обычного resize.
  const wasExpandedRef = useRef(false);
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

  function handleShow(payload: Show) {
    setSession((n) => n + 1);
    setHidden(false);
    setClosing(false);
    wasExpandedRef.current = false;
    winSizeRef.current = { w: CARD_W + MARGIN * 2, h: CARD_H_DEFAULT + MARGIN * 2 };
    pendingShrinkRef.current = null;
    setResult(null); setError(""); setFavorite(false); setShowOriginal(false); setPinned(false); setCopied(false);
    setSource(payload.text); setTarget(payload.target); setDetected(payload.detected); setClipNote(payload.clipboardReplaced);
    if (payload.text) {
      setInputMode(false); setStatus("loading"); setExpanded(false);
      window.setTimeout(() => setExpanded(true), 180);
    } else {
      setInputMode(true); setInput(""); setStatus("input"); setExpanded(true);
      window.setTimeout(() => inputRef.current?.focus(), 50);
    }
  }

  function handleError(message: string) {
    setError(errorText(message)); setStatus("error"); setExpanded(true);
  }

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => undefined);
    const subs = [
      listen<Show>("popup:show", ({ payload }) => handleShow(payload)),
      listen<TranslateResult>("popup:result", ({ payload }) => applyResult(payload)),
      listen<{ message: string }>("popup:error", ({ payload }) => handleError(payload.message)),
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
    setStatus("loading"); setError("");
    try { applyResult(await api.translate(text, to)); }
    catch (e) { setError(errorText(e)); setStatus("error"); }
  }

  // Размер капсулы: пилюля или карточка по фактической высоте содержимого.
  const expandedRef = useRef(false);
  expandedRef.current = expanded;
  const measure = () => {
    if (!expandedRef.current) { setBox({ w: (pillRef.current?.offsetWidth ?? 136) + 24, h: 46 }); return; }
    const el = cardRef.current;
    if (el) setBox({ w: CARD_W, h: el.offsetHeight });
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
  useEffect(() => { if (!hidden) fitWindow(box.w, box.h); }, [box.w, box.h, hidden]);

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

  const fontSize = settings?.fontSize ?? 16;

  // Морфинг пилюля→карточка — только на первом раскрытии сессии, дальше это обычный resize.
  const capsuleTransition = expanded && !wasExpandedRef.current ? morphT : resizeT;
  useLayoutEffect(() => { wasExpandedRef.current = expanded; }, [expanded]);

  if (hidden) return null;

  return (
    <div
      style={{ padding: MARGIN }}
      className="h-full w-full"
      onMouseDown={(e) => { if (e.target === e.currentTarget) hideNow(); }}
    >
      <motion.div
        key={session}
        className="glass overflow-hidden"
        initial={false}
        animate={{ width: box.w, height: box.h, borderRadius: expanded ? 22 : 999, opacity: closing ? 0 : 1, scale: closing ? 0.97 : 1 }}
        transition={{ width: capsuleTransition, height: capsuleTransition, borderRadius: capsuleTransition, opacity: exitT, scale: exitT }}
        onAnimationComplete={onCapsuleAnimationComplete}
      >
        <div ref={cardRef} style={{ width: CARD_W }} className="flex flex-col gap-2.5 p-3">
          <div className="flex items-center gap-2.5">
            <div ref={pillRef} className="pill pl-2.5! pr-3!" style={{ width: "fit-content" }}>
              <span className={`dot ${status === "loading" ? "pulse" : ""}`} />
              <span className="tracking-wide">{(detected ?? "auto").toUpperCase()}</span>
              <Icon name="arrow" size={14} className="opacity-45" />
              <button className="tracking-wide text-[var(--accent)]" onClick={switchTarget} title="Сменить целевой язык">
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
                  className="flex flex-1 items-center gap-2.5"
                >
                  <span className="max-w-[150px] truncate text-xs text-white/45" title={result ? engineLabel(result) : undefined}>
                    {status === "loading" ? "переводим…" : result ? engineLabel(result) : ""}
                  </span>
                  <div className="flex-1" />
                  <button className={`rb ${pinned ? "active" : ""}`} onClick={() => setPinned((p) => !p)} title="Закрепить"><Icon name="pin" /></button>
                  <button className="rb" onClick={() => api.openMain(currentText || undefined)} title="Открыть в окне"><Icon name="expand" /></button>
                  <button className="rb" onClick={closeAnimated} title="Закрыть (Esc)"><Icon name="close" /></button>
                </motion.div>
              )}
            </AnimatePresence>
          </div>

          {expanded && (
            <div className="flex flex-col gap-2.5">
              {inputMode && (
                <textarea
                  ref={inputRef}
                  value={input}
                  onChange={(e) => onInput(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); window.clearTimeout(debounce.current); translateNow(input); } }}
                  placeholder="Введите текст для перевода…"
                  rows={3}
                  className="field w-full resize-none py-2 leading-relaxed"
                  style={{ height: "auto", minHeight: 76, fontSize: 15 }}
                />
              )}

              {clipNote && <div className="px-2 text-xs text-amber-200/80">Буфер обмена содержал не текст и был заменён.</div>}

              <div style={{ position: "relative" }}>
                <AnimatePresence mode="popLayout" initial={false}>
                  {status === "loading" && (
                    <motion.div key="skeleton" variants={fx()} initial="hidden" animate="visible" exit="exit" className="flex flex-col gap-2.5 px-2 py-1">
                      <div className="sk w-[92%]" /><div className="sk w-[78%]" /><div className="sk w-[40%]" />
                    </motion.div>
                  )}

                  {status === "error" && (
                    <motion.div key="error" variants={fx()} initial="hidden" animate="visible" exit="exit" className="flex flex-col gap-3 px-2 py-1">
                      <div className="text-[15px] leading-relaxed text-white/85">{error}</div>
                      <div className="flex gap-2">
                        <button className="pill" onClick={() => translateNow(currentText, target)}><Icon name="refresh" size={15} className="opacity-70" />Повторить</button>
                        <button className="pill" onClick={() => api.openMain(currentText || undefined)}>Открыть окно</button>
                      </div>
                    </motion.div>
                  )}

                  {status === "result" && result && (
                    <motion.div key="result" variants={fx()} initial="hidden" animate="visible" exit="exit" className="flex flex-col gap-2">
                      <div className="flex flex-col gap-2 px-2">
                        <AnimatePresence initial={false}>
                          {showOriginal && (
                            <motion.div
                              key="orig"
                              variants={fx()}
                              initial="hidden"
                              animate="visible"
                              exit="exit"
                              className="select-text border-b border-white/10 pb-2 text-[13px] leading-relaxed text-white/50"
                              style={{ maxHeight: 120, overflow: "auto" }}
                            >
                              {source}
                            </motion.div>
                          )}
                        </AnimatePresence>
                        {result.wordMode ? (
                          <div className="flex flex-col gap-2">
                            <div className="flex items-center gap-2.5">
                              <span className="select-text text-2xl font-medium tracking-tight">{currentText.trim()}</span>
                              <button className="rb h-7! w-7!" onClick={() => speak(currentText, result.detected)} title="Озвучить оригинал"><Icon name="speaker" size={14} /></button>
                            </div>
                            <div className="select-text text-lg font-medium text-[var(--accent)]">{result.text}</div>
                            {result.alternatives.length > 0 && (
                              <div className="flex flex-col gap-1.5 border-t border-white/10 pt-2">
                                {result.alternatives.slice(0, 4).map((a) => (
                                  <div key={a.pos} className="flex flex-wrap items-center gap-1.5">
                                    <span className="px-1 text-[11px] font-medium uppercase tracking-wider text-white/40">{a.pos}</span>
                                    {a.terms.slice(0, 6).map((t) => (
                                      <button key={t} className="chip" onClick={() => api.copy(t)} title="Скопировать">{t}</button>
                                    ))}
                                  </div>
                                ))}
                              </div>
                            )}
                          </div>
                        ) : (
                          <div className="select-text leading-relaxed" style={{ fontSize, maxHeight: 300, overflow: "auto", textWrap: "pretty" }}>{result.text}</div>
                        )}
                      </div>

                      <div className="flex items-center gap-2">
                        <button className="pill pl-2.5!" onClick={copy}>
                          <AnimatePresence mode="popLayout" initial={false}>
                            <motion.span
                              key={copied ? "done" : "copy"}
                              initial={{ opacity: 0, filter: "blur(2px)" }}
                              animate={{ opacity: 1, filter: "blur(0px)" }}
                              exit={{ opacity: 0, filter: "blur(2px)" }}
                              transition={{ duration: reduce ? 0.15 : 0.18 }}
                              className="flex items-center gap-1.5"
                            >
                              <Icon name={copied ? "check" : "copy"} size={15} className="opacity-70" />{copied ? "Скопировано" : "Копировать"}
                            </motion.span>
                          </AnimatePresence>
                        </button>
                        <button className="pill pl-2.5!" onClick={() => speak(result.text, result.target)}><Icon name="speaker" size={15} className="opacity-70" />Озвучить</button>
                        <button className={`rb ${favorite ? "active" : ""} ${starPop ? "star-pop" : ""}`} onClick={toggleFavorite} title="В избранное" disabled={!result.historyId}><Icon name="star" /></button>
                        <div className="flex-1" />
                        {!inputMode && (
                          <button className="flex items-center gap-1.5 pr-2 text-xs text-white/50 hover:text-white/80" onClick={() => setShowOriginal((s) => !s)}>
                            Оригинал <Icon name="chevron" size={12} className={`chevron ${showOriginal ? "rotate-180" : ""}`} />
                          </button>
                        )}
                      </div>
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
            </div>
          )}
        </div>
      </motion.div>
    </div>
  );
}
