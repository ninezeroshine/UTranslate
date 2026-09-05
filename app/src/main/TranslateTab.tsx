import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  api, engineLabel, engineName, errorHint, errorText, langName, posName, speak,
  type Settings, type TranslateResult,
} from "../lib/api";
import { Icon } from "../lib/icons";
import { Badge, Card, IconButton, Pill } from "../ui";
import { FavoriteController, TranslationController } from "./latestRequest";

const EASE_OUT = [0.23, 1, 0.32, 1] as const;
const COPIED_MS = 1500;
/** Перевод идёт сам через 600 мс после последней правки: явной кнопки «Перевести» нет
 *  (в макете она с бейджем «план» — design/bento/Main.dc.html). */
const DEBOUNCE_MS = 600;

type Status = "idle" | "loading" | "error";

export function TranslateTab({ prefill, settings }: { prefill: { text: string; n: number }; settings: Settings | null }) {
  const reduce = useReducedMotion();
  const [source, setSource] = useState(prefill.text);
  const [target, setTarget] = useState<string | undefined>(undefined);
  const [result, setResult] = useState<TranslateResult | null>(null);
  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState<{ title: string; hint: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const [favorite, setFavorite] = useState(false);
  const [starPop, setStarPop] = useState(false);
  const [screenPending, setScreenPending] = useState(false);
  const [screenError, setScreenError] = useState<string | null>(null);
  const controller = useRef(new TranslationController());
  const favorites = useRef(new FavoriteController());

  useEffect(() => {
    if (prefill.n > 0) {
      controller.current.change("prefill");
      setSource(prefill.text);
      setResult(null);
      favorites.current.clear();
      setFavorite(false);
      setStarPop(false);
      setCopied(false);
      setError(null);
      setStatus(prefill.text.trim() ? "loading" : "idle");
    }
  }, [prefill]);

  useEffect(() => () => {
    controller.current.change("unmount");
    favorites.current.clear();
  }, []);

  useEffect(() => {
    controller.current.clearPending();
    if (!source.trim()) { setResult(null); setError(null); setStatus("idle"); return; }
    controller.current.schedule(run, DEBOUNCE_MS);
    return () => controller.current.clearPending();
  }, [source, target]);

  async function run() {
    const token = controller.current.begin();
    setStatus("loading"); setError(null);
    try {
      const next = await api.translate(source, target);
      if (!controller.current.isCurrent(token)) return;
      setResult(next);
      favorites.current.accept(next.historyId, next.isFavorite);
      setFavorite(next.isFavorite);
      setStarPop(false);
      setCopied(false);
      setStatus("idle");
    } catch (e) {
      if (!controller.current.isCurrent(token)) return;
      setError({ title: errorText(e), hint: errorHint(e) });
      setStatus("error");
    }
  }

  function changeSource(next: string) {
    controller.current.change("edit");
    setSource(next);
    setResult(null);
    favorites.current.clear();
    setFavorite(false);
    setStarPop(false);
    setCopied(false);
    setError(null);
    setStatus(next.trim() ? "loading" : "idle");
  }

  /** Поменять местами: перевод уходит в исходник, найденный язык становится целевым. */
  function swap() {
    if (!result) return;
    controller.current.change("swap");
    setTarget(result.detected);
    setSource(result.text);
    setResult(null);
    favorites.current.clear();
    setFavorite(false);
    setStarPop(false);
    setCopied(false);
    setStatus("loading");
  }

  function switchTarget() {
    if (!settings) return;
    controller.current.change("target");
    setTarget(targetLabel === settings.primaryLang ? settings.secondaryLang : settings.primaryLang);
    setResult(null);
    favorites.current.clear();
    setFavorite(false);
    setStarPop(false);
    setCopied(false);
    setError(null);
    setStatus("loading");
  }

  function copy() {
    if (!result) return;
    void api.copy(result.text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), COPIED_MS);
    });
  }

  async function toggleFavorite() {
    if (!result?.historyId) return;
    const historyId = result.historyId;
    const next = !favorite;
    setFavorite(next);
    if (next) { setStarPop(true); window.setTimeout(() => setStarPop(false), 240); }
    const rollback = await favorites.current.mutate(historyId, next, () => api.setFavorite(historyId, next));
    if (rollback !== null) setFavorite(rollback);
  }

  async function translateScreen() {
    if (screenPending) return;
    setScreenPending(true);
    setScreenError(null);
    try {
      await api.translateScreen();
    } catch (e) {
      setScreenError(errorText(e));
    } finally {
      setScreenPending(false);
    }
  }

  const engines = settings?.engines ?? ["google", "bing", "mymemory"];
  const targetLabel = target ?? result?.target ?? settings?.primaryLang ?? "ru";
  const detected = result?.detected ?? null;
  const bodyFx = {
    initial: { opacity: 0, filter: reduce ? "none" : "blur(3px)" },
    animate: { opacity: 1, filter: "blur(0px)" },
    exit: { opacity: 0 },
    transition: { duration: reduce ? 0.15 : 0.18, ease: EASE_OUT },
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3.5">
      <div className="relative grid min-h-0 flex-1 grid-cols-2 gap-4">

        <Card className="flex min-h-0 flex-col gap-3 p-4">
          <div className="flex items-center gap-2">
            <span className="pill pointer-events-none pl-3!">
              <span className="text-[12px] font-semibold tracking-[0.07em] text-mist">{(detected ?? "auto").toUpperCase()}</span>
              <span>{detected ? langName(detected) : "Определить язык"}</span>
            </span>
            {detected && <Badge tone="mist">определён</Badge>}
            <div className="flex-1" />
            {source && <IconButton icon="close" label="Очистить" onClick={() => changeSource("")} />}
          </div>
          <textarea
            value={source}
            onChange={(e) => changeSource(e.target.value)}
            placeholder="Введите или вставьте текст…"
            aria-label="Исходный текст"
            spellCheck={false}
            className="min-h-0 flex-1 resize-none bg-transparent px-1.5 py-0.5 text-base leading-[1.6] text-ink-2 outline-none placeholder:text-ink-3"
          />
          <div className="flex items-center gap-2.5">
            <span className="pl-1.5 text-xs text-ink-3">{source.length} / 5000</span>
            <span className="text-[11px] text-ink-3">{settings?.hotkeyPopup ?? "Ctrl+Alt+T"} — перевести выделенное</span>
            <div className="flex-1" />
            {screenError && <span className="max-w-42 truncate text-[11px] text-err" role="alert">{screenError}</span>}
            <Pill
              icon="screen"
              disabled={screenPending}
              onClick={() => { void translateScreen(); }}
              title={`${settings?.hotkeyScreen ?? "Ctrl+Alt+S"} — перевести область экрана`}
            >
              {screenPending ? "Открываем…" : "С экрана"}
            </Pill>
            <IconButton icon="speaker" label="Озвучить исходник" disabled={!source} onClick={() => speak(source, detected ?? "en")} />
          </div>
        </Card>

        <Card className="flex min-h-0 flex-col gap-3 p-4">
          <div className="flex items-center gap-2.5">
            <Pill variant="active" className="pl-3!" onClick={switchTarget} title="Сменить целевой язык">
              <span className={`dot ${status === "loading" ? "hollow pulse" : ""}`} />
              <span className="text-[12px] font-semibold tracking-[0.07em] text-water">{targetLabel.toUpperCase()}</span>
              <span>{langName(targetLabel)}</span>
              <Icon name="chevron" size={12} className="text-ink-3" />
            </Pill>
            {status === "loading" && <span className="text-[12px] text-ink-3">переводим…</span>}
            {status === "error" && <span className="text-[12px] text-err">не удалось</span>}
            {status === "idle" && result && (
              <Badge tone={result.fallbackFrom ? "sand" : "neutral"} title={result.fallbackFrom ?? undefined}>
                {engineLabel(result)}
              </Badge>
            )}
          </div>

          <div className="relative min-h-0 flex-1 overflow-auto px-1.5 py-0.5" aria-live="polite">
            <AnimatePresence mode="popLayout" initial={false}>
              {status === "error" && error ? (
                <motion.div key="error" {...bodyFx} className="flex flex-col gap-3.5">
                  <div className="flex flex-col gap-1.5">
                    <span className="text-[18px] font-semibold tracking-[-0.02em]">{error.title}</span>
                    <span className="text-[13px] leading-[1.5] text-ink-2">{error.hint}</span>
                  </div>
                  <Pill variant="water" size="md" icon="refresh" className="w-fit" onClick={run}>Повторить</Pill>
                </motion.div>
              ) : status === "loading" && !result ? (
                <motion.div key="skeleton" {...bodyFx} className="flex flex-col gap-2.5 py-1.5">
                  <div className="sk w-[92%]" /><div className="sk w-[78%]" /><div className="sk w-[40%]" />
                </motion.div>
              ) : result ? (
                <motion.div key="result" {...bodyFx} className="flex flex-col gap-3">
                  <div className="select-text text-[20px] leading-[1.55] tracking-[-0.005em]" style={{ textWrap: "pretty" }}>
                    {result.text}
                  </div>
                  {result.wordMode && result.alternatives.length > 0 && (
                    <div className="flex flex-col gap-2 border-t border-line pt-3">
                      {result.alternatives.slice(0, 4).map((a) => (
                        <div key={a.pos} className="flex items-start gap-2">
                          <span className="min-w-[52px] shrink-0 pt-1 text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">{posName(a.pos)}</span>
                          <div className="flex flex-wrap gap-1.5">
                            {a.terms.slice(0, 6).map((t) => (
                              <button key={t} type="button" className="chip" onClick={() => api.copy(t)} title="Скопировать">{t}</button>
                            ))}
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </motion.div>
              ) : (
                <motion.div key="empty" {...bodyFx} className="text-base text-ink-3">Перевод появится здесь</motion.div>
              )}
            </AnimatePresence>
          </div>

          <div className="flex items-center gap-2">
            <Pill variant="water" size="md" disabled={!result} onClick={copy}>
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={copied ? "done" : "copy"}
                  initial={{ opacity: 0, filter: reduce ? "none" : "blur(2px)" }}
                  animate={{ opacity: 1, filter: "blur(0px)" }}
                  exit={{ opacity: 0, filter: reduce ? "none" : "blur(2px)" }}
                  transition={{ duration: reduce ? 0.15 : 0.18 }}
                  className="flex items-center gap-1.5"
                >
                  <Icon name={copied ? "check" : "copy"} size={15} />
                  {copied ? "Скопировано" : "Копировать"}
                </motion.span>
              </AnimatePresence>
            </Pill>
            <Pill size="md" icon="speaker" disabled={!result} onClick={() => result && speak(result.text, result.target)}>Озвучить</Pill>
            <div className="flex-1" />
            <IconButton
              icon="star"
              label="В избранное"
              size={36}
              tone="sand"
              active={favorite}
              className={starPop ? "star-pop" : ""}
              disabled={!result?.historyId}
              onClick={toggleFavorite}
            />
          </div>
        </Card>

        <button
          type="button"
          className="swap absolute left-1/2 top-3.5 flex h-11 w-11 -translate-x-1/2 items-center justify-center rounded-full"
          onClick={swap}
          disabled={!result}
          title="Поменять местами"
          aria-label="Поменять языки местами"
        >
          <Icon name="swap" size={17} />
        </button>
      </div>

      <Card className="flex h-15 shrink-0 items-center gap-2.5 rounded-tile px-4">
        <span className="text-[11px] font-semibold uppercase tracking-[0.09em] text-ink-3">Цепочка</span>
        <div className="flex gap-1.5">
          {engines.map((e, i) => {
            const on = result ? result.engine === e : i === 0;
            return (
              <span key={e} className={`pill pointer-events-none h-8! px-[13px]! ${on ? "active" : "text-ink-2"}`}>
                {on && <span className="dot h-1.5! w-1.5!" />}
                {engineName(e)}
              </span>
            );
          })}
        </div>
        <div className="flex-1" />
        <span className="text-[11px] text-ink-3">Порядок задаётся в настройках</span>
      </Card>
    </div>
  );
}
