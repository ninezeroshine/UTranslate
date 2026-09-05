import { AnimatePresence, motion, type Variants } from "motion/react";
import type { PopupOrigin } from "./types";

type Props = {
  shown: boolean;
  origin: PopupOrigin;
  source: string;
  disabled: boolean;
  variants: Variants;
  onChange: (value: string) => void;
};

/** Блок «Оригинал» над переводом. Для выделения — текст только на чтение,
 *  для экрана — распознанный текст, который можно исправить: правка запускает перевод заново. */
export function SourceBlock({ shown, origin, source, disabled, variants, onChange }: Props) {
  return (
    <AnimatePresence initial={false}>
      {shown && (origin === "screen" ? (
        <motion.div
          key="orig"
          data-popup-original
          className="popup-source-correction"
          variants={variants}
          initial="hidden"
          animate="visible"
          exit="exit"
        >
          <label htmlFor="popup-screen-source" className="text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">
            Распознанный текст
          </label>
          <textarea
            id="popup-screen-source"
            aria-label="Распознанный текст"
            value={source}
            rows={3}
            spellCheck={false}
            disabled={disabled}
            className="field popup-edit-textarea"
            onChange={(event) => onChange(event.target.value)}
          />
          <span className="text-[11px] text-ink-3">Правки переводятся автоматически.</span>
        </motion.div>
      ) : (
        <motion.div
          key="orig"
          data-popup-original
          variants={variants}
          initial="hidden"
          animate="visible"
          exit="exit"
          className="select-text border-b border-line px-1 pb-[11px] text-[13px] leading-[1.55] text-ink-3"
          style={{ maxHeight: 120, overflow: "auto" }}
        >
          {source}
        </motion.div>
      ))}
    </AnimatePresence>
  );
}
