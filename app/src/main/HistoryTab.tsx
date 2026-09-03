import { useEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { api, errorText, type Entry, type Settings } from "../lib/api";
import { Icon } from "../lib/icons";
import { Badge, Card, Field, IconButton, Pill } from "../ui";

const EASE_OUT = [0.23, 1, 0.32, 1] as const;
const SEARCH_MS = 150;
const COPIED_MS = 1500;
/** Пока идёт отсчёт, строка уже убрана из списка, но в базе цела: «Отменить» просто гасит таймер.
 *  ponytail: если приложение закрыть за эти 5 секунд, запись останется — промах в безопасную
 *  сторону. Настоящая корзина понадобится, только если попросят восстанавливать удалённое позже. */
const UNDO_MS = 5000;

type Group = "today" | "yesterday" | "older";
const GROUP_LABEL: Record<Group, string> = { today: "Сегодня", yesterday: "Вчера", older: "Раньше" };

/** Границы суток считаем один раз на отрисовку: полночь сегодня и полночь вчера. */
function dayBounds() {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  const today = d.getTime() / 1000;
  return { today, yesterday: today - 86400 };
}

function groupOf(ts: number, b: { today: number; yesterday: number }): Group {
  return ts >= b.today ? "today" : ts >= b.yesterday ? "yesterday" : "older";
}

function fmtTime(ts: number, group: Group) {
  const d = new Date(ts * 1000);
  return group === "older"
    ? d.toLocaleDateString("ru-RU", { day: "numeric", month: "short" })
    : d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
}

/** «Ctrl+Alt+T» кейкапами; Super показываем как Win — так короче для Windows. */
function Keys({ combo }: { combo: string }) {
  const parts = combo.split("+").map((p) => (p === "Super" ? "Win" : p));
  return (
    <div className="flex items-center gap-1.5">
      {parts.map((p, i) => (
        <span key={p} className="flex items-center gap-1.5">
          {i > 0 && <span className="text-xs text-ink-3">+</span>}
          <span className="key">{p}</span>
        </span>
      ))}
    </div>
  );
}

function Empty({ icon, title, hint, children }: { icon: "search" | "star"; title: string; hint: string; children?: ReactNode }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3.5 px-6">
      <div className="flex h-19 w-19 items-center justify-center rounded-full bg-[var(--water-soft)] text-water">
        <Icon name={icon} size={30} className="[stroke-width:0.8]" />
      </div>
      <div className="flex max-w-[380px] flex-col items-center gap-[7px] text-center">
        <span className="text-[19px] font-semibold tracking-[-0.02em]">{title}</span>
        <span className="text-[13px] leading-[1.5] text-ink-2">{hint}</span>
      </div>
      {children}
    </div>
  );
}

type Props = { favorites: boolean; settings: Settings | null; onOpen: (text: string) => void };

/** История и Избранное: один список, разный фильтр. */
export function HistoryTab({ favorites, settings, onOpen }: Props) {
  const reduce = useReducedMotion();
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<Entry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [exported, setExported] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const [copiedId, setCopiedId] = useState<number | null>(null);
  // Строка, ждущая настоящего удаления: скрыта из списка, пока висит тост «Отменить».
  const [pendingId, setPendingId] = useState<number | null>(null);
  const pendingRef = useRef<number | null>(null);
  const undoTimer = useRef<number | undefined>(undefined);

  function load() {
    return api
      .history(query, favorites)
      .then((rows) => { setItems(rows); setError(""); })
      .catch((e) => setError(errorText(e)))
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    const t = window.setTimeout(load, SEARCH_MS);
    return () => window.clearTimeout(t);
  }, [query, favorites]);

  /** Довести отложенное удаление до базы. Зовём по таймеру, перед новым удалением и при уходе с вкладки. */
  function flushDelete() {
    const id = pendingRef.current;
    window.clearTimeout(undoTimer.current);
    pendingRef.current = null;
    setPendingId(null);
    if (id === null) return;
    api.deleteEntry(id).then(load, (e) => { setError(errorText(e)); load(); });
  }

  // Уходим с вкладки — доводим отложенное удаление молча: тоста и списка уже нет.
  useEffect(() => () => {
    window.clearTimeout(undoTimer.current);
    if (pendingRef.current !== null) void api.deleteEntry(pendingRef.current);
  }, []);

  function remove(e: Entry) {
    flushDelete();
    pendingRef.current = e.id;
    setPendingId(e.id);
    undoTimer.current = window.setTimeout(flushDelete, UNDO_MS);
  }

  function undoDelete() {
    window.clearTimeout(undoTimer.current);
    pendingRef.current = null;
    setPendingId(null);
  }

  async function toggle(e: Entry) {
    setItems((list) => list.map((x) => (x.id === e.id ? { ...x, isFavorite: !x.isFavorite } : x)));
    try { await api.setFavorite(e.id, !e.isFavorite); } catch (err) { setError(errorText(err)); }
    load();
  }

  function copyRow(e: Entry) {
    void api.copy(e.resultText).then(() => {
      setCopiedId(e.id);
      window.setTimeout(() => setCopiedId((id) => (id === e.id ? null : id)), COPIED_MS);
    });
  }

  async function exportCsv() {
    try {
      setExported(await api.exportFavorites());
      window.setTimeout(() => setExported(""), 6000);
    } catch (e) { setError(errorText(e)); }
  }

  /** Очистка истории необратима и трогает много строк — поэтому подтверждение, а не отмена. */
  async function clear() {
    if (!confirmClear) {
      setConfirmClear(true);
      window.setTimeout(() => setConfirmClear(false), 3000);
      return;
    }
    setConfirmClear(false);
    try { await api.clearHistory(); } catch (e) { setError(errorText(e)); }
    load();
  }

  const shown = pendingId === null ? items : items.filter((e) => e.id !== pendingId);
  const empty = items.length === 0 && !query;
  const bounds = dayBounds();
  let lastGroup: Group | null = null;

  return (
    <Card className="relative flex min-h-0 flex-1 flex-col gap-3 p-4">
      <div className="flex items-center gap-2.5">
        <Field
          icon="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={favorites ? "Поиск в избранном" : "Поиск по тексту и переводу"}
          aria-label={favorites ? "Поиск в избранном" : "Поиск в истории"}
          wrapClassName="h-9! flex-1"
        />
        {/* Обе кнопки работают со всем списком, а не с найденным, поэтому гаснут только на пустой базе. */}
        {favorites ? (
          <Pill size="md" icon="copy" disabled={empty} onClick={exportCsv} title="Сохранить CSV в «Загрузки»">
            Экспорт CSV
          </Pill>
        ) : (
          <Pill size="md" icon="trash" disabled={empty} onClick={clear} className={confirmClear ? "text-err" : ""}>
            {confirmClear ? "Точно очистить?" : "Очистить"}
          </Pill>
        )}
      </div>

      {error && (
        <div className="flex items-center gap-3 px-3 text-[13px] text-err">
          {error}
          <Pill icon="refresh" onClick={() => { setLoading(true); load(); }}>Повторить</Pill>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <div className="flex flex-col gap-4 p-3">
            {[0, 1, 2, 3, 4].map((i) => <div key={i} className="sk w-full" />)}
          </div>
        ) : shown.length === 0 ? (
          query ? (
            <Empty icon="search" title="Ничего не найдено" hint="Поиск идёт по оригиналу и переводу. Попробуйте другое слово или очистите строку." />
          ) : favorites ? (
            <Empty icon="star" title="В избранном пусто" hint="Отметьте перевод звёздочкой — в попапе или в истории, — и он появится здесь.">
              <span className="text-[11px] text-ink-3">Экспорт CSV включится, когда появится первая запись.</span>
            </Empty>
          ) : (
            <Empty icon="search" title="История пуста" hint="Выделите текст в любом окне и нажмите сочетание — перевод сохранится сюда.">
              <Keys combo={settings?.hotkeyPopup ?? "Ctrl+Alt+T"} />
            </Empty>
          )
        ) : (
          <div className="flex flex-col gap-px">
            {shown.map((e, i) => {
              const group = groupOf(e.createdAt, bounds);
              const head = group !== lastGroup;
              lastGroup = group;
              return (
                <motion.div
                  key={e.id}
                  initial={{ opacity: 0, y: reduce ? 0 : 6 }}
                  animate={{
                    opacity: 1,
                    y: 0,
                    transition: { duration: reduce ? 0.15 : 0.2, ease: EASE_OUT, delay: query || i > 11 ? 0 : i * 0.03 },
                  }}
                >
                  {head && (
                    <div className="flex items-center gap-2.5 px-3 pb-1 pt-2">
                      <span className="text-[11px] font-semibold uppercase tracking-[0.09em] text-ink-3">{GROUP_LABEL[group]}</span>
                      <div className="h-px flex-1 bg-line" />
                    </div>
                  )}
                  <div
                    className="row group flex items-center gap-3 px-3 py-[9px]"
                    tabIndex={0}
                    title="Двойной клик — открыть в «Переводе»"
                    onDoubleClick={() => onOpen(e.sourceText)}
                    onKeyDown={(ev) => { if (ev.key === "Enter" && ev.target === ev.currentTarget) onOpen(e.sourceText); }}
                  >
                    <span className="w-11 shrink-0 text-xs text-ink-3">{fmtTime(e.createdAt, group)}</span>
                    <Badge tone="mist" dot={false} className="h-[22px] shrink-0 tracking-[0.05em]">
                      {e.sourceLang.toUpperCase()} → {e.targetLang.toUpperCase()}
                    </Badge>
                    <span className="w-[34%] shrink-0 truncate text-[13px]" title={e.sourceText}>{e.sourceText}</span>
                    <span className="min-w-0 flex-1 truncate text-[13px] text-ink-2" title={e.resultText}>{e.resultText}</span>
                    <div className="flex shrink-0 items-center gap-1">
                      <IconButton
                        icon="star"
                        size={28}
                        tone="sand"
                        active={e.isFavorite}
                        className={e.isFavorite ? "" : "ghost"}
                        label={e.isFavorite ? "Убрать из избранного" : "В избранное"}
                        onClick={() => toggle(e)}
                      />
                      <div className="flex gap-1 opacity-0 transition-opacity duration-150 group-focus-within:opacity-100 group-hover:opacity-100">
                        <IconButton
                          icon={copiedId === e.id ? "check" : "copy"}
                          size={28}
                          className="ghost"
                          label={copiedId === e.id ? "Скопировано" : "Копировать перевод"}
                          onClick={() => copyRow(e)}
                        />
                        <IconButton icon="trash" size={28} className="ghost" label="Удалить" onClick={() => remove(e)} />
                      </div>
                    </div>
                  </div>
                </motion.div>
              );
            })}
          </div>
        )}
      </div>

      <span className="shrink-0 px-3 text-xs text-ink-3">
        {exported
          ? `Сохранено: ${exported}`
          : favorites
            ? "Экспорт кладёт CSV в «Загрузки» и показывает путь здесь."
            : "Двойной клик по строке открывает текст во вкладке «Перевод». Очистка не трогает избранное."}
      </span>

      <AnimatePresence>
        {pendingId !== null && (
          <motion.div
            key="undo"
            role="status"
            className="pop absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-3 rounded-full py-1.5 pl-5 pr-1.5"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: reduce ? 0.1 : 0.18, ease: EASE_OUT }}
          >
            <span className="text-[13px]">Удалено</span>
            <Pill icon="refresh" onClick={undoDelete}>Отменить</Pill>
          </motion.div>
        )}
      </AnimatePresence>
    </Card>
  );
}
