import type { ReactNode } from "react";

type Props = {
  /** sand — цепочка ушла на резервный движок: единственное тёплое пятно в интерфейсе.
   *  mist — служебная отметка без оценки: «определён», направление перевода. */
  tone?: "neutral" | "sand" | "mist";
  /** Точка слева: зелёная у обычного бейджа, в цвет текста у песочного, туманная у mist. */
  dot?: boolean;
  title?: string;
  className?: string;
  children: ReactNode;
};

/** Бейдж состояния: движок, резервный движок, определён язык. */
export function Badge({ tone = "neutral", dot = true, title, className = "", children }: Props) {
  return (
    <span className={["badge", tone === "neutral" ? "" : tone, className].filter(Boolean).join(" ")} title={title}>
      {dot && <span className="badge-dot" />}
      {children}
    </span>
  );
}
