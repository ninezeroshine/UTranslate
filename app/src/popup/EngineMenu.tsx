import { AnimatePresence, motion, type Variants } from "motion/react";
import { engineName } from "../lib/api";

type Props = {
  open: boolean;
  /** Порядок движков из настроек; «Автоматически» добавляется первым пунктом. */
  engines: string[];
  selected: string | undefined;
  variants: Variants;
  onChoose: (engine: string | undefined) => void;
};

/** Выбор движка перевода: радиогруппа под бейджем в шапке. */
export function EngineMenu({ open, engines, selected, variants, onChoose }: Props) {
  return (
    <AnimatePresence initial={false}>
      {open && (
        <motion.div
          key="engine-menu"
          className="popup-engine-menu"
          role="menu"
          aria-label="Движок перевода"
          variants={variants}
          initial="hidden"
          animate="visible"
          exit="exit"
        >
          <button
            type="button"
            role="menuitemradio"
            aria-checked={selected === undefined}
            className={selected === undefined ? "active" : ""}
            onClick={() => onChoose(undefined)}
          >
            Автоматически
          </button>
          {engines.map((engine) => (
            <button
              type="button"
              role="menuitemradio"
              aria-checked={selected === engine}
              className={selected === engine ? "active" : ""}
              key={engine}
              onClick={() => onChoose(engine)}
            >
              {engineName(engine)}
            </button>
          ))}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
