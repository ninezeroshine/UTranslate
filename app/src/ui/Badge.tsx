import type { ReactNode } from "react";

type Props = {
  /** sand — цепочка ушла на резервный движок: единственное тёплое пятно в интерфейсе. */
  tone?: "neutral" | "sand";
  /** Точка слева: зелёная у обычного бейджа, в цвет текста у песочного. */
  dot?: boolean;
  title?: string;
  children: ReactNode;
};

/** Бейдж состояния: движок, резервный движок, определён язык. */
export function Badge({ tone = "neutral", dot = true, title, children }: Props) {
  return (
    <span className={tone === "sand" ? "badge sand" : "badge"} title={title}>
      {dot && <span className="badge-dot" />}
      {children}
    </span>
  );
}
