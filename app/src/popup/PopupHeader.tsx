import type { RefObject } from "react";
import { AnimatePresence, motion, type Variants } from "motion/react";
import { engineLabel, type TranslateResult } from "../lib/api";
import { Icon } from "../lib/icons";
import { Badge, IconButton } from "../ui";
import type { PopupOrigin, PopupStatus } from "./types";

type Props = {
  /** По этой строке меряется ширина свёрнутой пилюли. */
  containerRef: RefObject<HTMLDivElement | null>;
  engineButtonRef: RefObject<HTMLButtonElement | null>;
  expanded: boolean;
  status: PopupStatus;
  origin: PopupOrigin;
  detected: string | null;
  target: string;
  result: TranslateResult | null;
  engineMenuOpen: boolean;
  /** Идёт сохранение, смена движка, правка или замена — переключать язык и движок нельзя. */
  locked: boolean;
  pinned: boolean;
  headerFx: Variants;
  onSwitchTarget: () => void;
  onToggleEngineMenu: () => void;
  onTogglePin: () => void;
  onOpenMain: () => void;
  onClose: () => void;
};

/** Шапка капсулы: пилюля языков, бейджи движка и источника, кнопки окна. */
export function PopupHeader({
  containerRef, engineButtonRef, expanded, status, origin, detected, target, result,
  engineMenuOpen, locked, pinned, headerFx,
  onSwitchTarget, onToggleEngineMenu, onTogglePin, onOpenMain, onClose,
}: Props) {
  return (
    <div ref={containerRef} className="flex items-center gap-2">
      <div className={`langpill ${expanded ? "on" : ""}`}>
        <span className={`dot ${detected ? "" : "hollow"} ${status === "loading" || status === "recognizing" ? "pulse" : ""}`} />
        <span className="text-[12px] font-semibold tracking-[0.07em] text-ink-2">{(detected ?? "auto").toUpperCase()}</span>
        <Icon name="arrow" size={14} className="text-ink-3" />
        <button
          className="text-[12px] font-semibold tracking-[0.07em] text-water"
          onClick={onSwitchTarget}
          title="Сменить целевой язык"
          disabled={locked}
        >
          {target.toUpperCase()}
        </button>
      </div>
      {!expanded && status === "recognizing" && (
        <span className="pr-3 text-[12px] font-medium text-ink-2" role="status">Распознаём…</span>
      )}
      <AnimatePresence>
        {expanded && (
          <motion.div
            key="hdr"
            variants={headerFx}
            initial="hidden"
            animate="visible"
            exit="exit"
            style={{ transformOrigin: "right center" }}
            className="flex min-w-0 flex-1 items-center gap-2"
          >
            {status === "loading" && <span className="text-[12px] text-ink-3">переводим…</span>}
            {status === "error" && <span className="text-[12px] text-err">не удалось</span>}
            {status === "result" && result && (
              <button
                ref={engineButtonRef}
                type="button"
                className={`badge popup-engine-button ${result.fallbackFrom ? "sand" : ""}`}
                title={result.fallbackFrom ?? "Выбрать движок перевода"}
                aria-label="Выбрать движок перевода"
                aria-haspopup="menu"
                aria-expanded={engineMenuOpen}
                disabled={locked}
                onClick={onToggleEngineMenu}
              >
                {engineLabel(result)}
                <Icon name="chevron" size={10} className={`chevron ${engineMenuOpen ? "rotate-180" : ""}`} />
              </button>
            )}
            {origin === "screen" && (
              <Badge dot={false} className="popup-origin-badge shrink-0" title="Текст распознан с экрана">
                <Icon name="screen" size={12} />
                с экрана
              </Badge>
            )}
            <div className="flex-1" />
            <div className="flex shrink-0 gap-1">
              <IconButton icon="pin" label="Закрепить" active={pinned} onClick={onTogglePin} />
              <IconButton icon="expand" label="Открыть в окне" onClick={onOpenMain} />
              <IconButton icon="close" label="Закрыть (Esc)" onClick={onClose} />
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
