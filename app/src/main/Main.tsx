import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { listen } from "../lib/tauri";
import { api, type Settings } from "../lib/api";
import { applyTheme } from "../lib/theme";
import { TitleBar, type Tab } from "./TitleBar";
import { TranslateTab } from "./TranslateTab";
import { HistoryTab } from "./HistoryTab";
import { SettingsTab } from "./SettingsTab";

const EASE_OUT = [0.23, 1, 0.32, 1] as const;

/** Каркас окна: шапка, вкладки, общие настройки. Данные каждой вкладки живут в ней самой. */
export default function Main() {
  const reduce = useReducedMotion();
  const [tab, setTab] = useState<Tab>("translate");
  // n растёт на каждую подстановку: тот же текст, присланный второй раз, тоже должен доехать.
  const [prefill, setPrefill] = useState<{ text: string; n: number }>({ text: "", n: 0 });
  const [settings, setSettings] = useState<Settings | null>(null);

  function openInTranslate(text: string) {
    setPrefill((p) => ({ text, n: p.n + 1 }));
    setTab("translate");
  }

  useEffect(() => {
    api.getSettings().then((s) => { setSettings(s); applyTheme(s.theme); }).catch(() => undefined);
    const un = listen<string>("main:prefill", ({ payload }) => openInTranslate(payload));
    return () => { un.then((f) => f()); };
  }, []);

  return (
    <div className="spot flex h-full flex-col">
      <TitleBar tab={tab} onTab={setTab} />
      <div className="relative min-h-0 flex-1 px-5 pb-5">
        {/* Перевод остаётся смонтированным: черновик и результат переживают просмотр других вкладок. */}
        <motion.div
          initial={false}
          animate={{ opacity: tab === "translate" ? 1 : 0 }}
          transition={{ duration: reduce ? 0.1 : 0.18, ease: EASE_OUT }}
          aria-hidden={tab !== "translate"}
          inert={tab !== "translate"}
          className="absolute inset-x-5 bottom-5 top-0 flex min-h-0 flex-col"
          style={{ pointerEvents: tab === "translate" ? "auto" : "none" }}
        >
          <TranslateTab prefill={prefill} settings={settings} />
        </motion.div>

        {/* Остальные вкладки монтируются при открытии, чтобы списки перечитывали свежие данные. */}
        <AnimatePresence mode="wait" initial={false}>
          {tab !== "translate" && (
            <motion.div
              key={tab}
              initial={{ opacity: 0 }}
              animate={{ opacity: 1, transition: { duration: reduce ? 0.1 : 0.18, ease: EASE_OUT } }}
              exit={{ opacity: 0, transition: { duration: reduce ? 0.1 : 0.12 } }}
              className="absolute inset-x-5 bottom-5 top-0 flex min-h-0 flex-col"
            >
              {tab === "history" && <HistoryTab favorites={false} settings={settings} onOpen={openInTranslate} />}
              {tab === "favorites" && <HistoryTab favorites settings={settings} onOpen={openInTranslate} />}
              {tab === "settings" && <SettingsTab onSettings={setSettings} />}
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
