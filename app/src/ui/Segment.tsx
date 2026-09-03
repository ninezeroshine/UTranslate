import { motion, useReducedMotion, type Transition } from "motion/react";

// Пружина resize из docs/motion.md: подложка переезжает без отскока.
const T_RESIZE: Transition = { type: "spring", duration: 0.3, bounce: 0 };

type Props<T extends string> = {
  items: readonly { id: T; label: string }[];
  value: T;
  onChange: (id: T) => void;
  /** Уникальный на странице: по нему Motion переносит подложку между кнопками. */
  layoutId: string;
  className?: string;
};

/** Сегментные вкладки: оправа .segbar, активная кнопка под подложкой .tab-pill. */
export function Segment<T extends string>({ items, value, onChange, layoutId, className = "" }: Props<T>) {
  const reduce = useReducedMotion();
  return (
    <div className={`segbar flex gap-0.5 p-[3px] ${className}`}>
      {items.map((it) => (
        <button
          key={it.id}
          type="button"
          aria-pressed={it.id === value}
          className={`seg ${it.id === value ? "active" : ""}`}
          onClick={() => onChange(it.id)}
        >
          {it.id === value && (
            <motion.span layoutId={layoutId} className="tab-pill" transition={reduce ? { duration: 0.15 } : T_RESIZE} />
          )}
          <span className="relative">{it.label}</span>
        </button>
      ))}
    </div>
  );
}
