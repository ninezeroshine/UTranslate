import type { HTMLAttributes } from "react";

type Props = HTMLAttributes<HTMLDivElement> & {
  /** panel — панель окна (радиус 24), tile — плитка внутри панели (радиус 20). */
  level?: "panel" | "tile";
};

/** Матовая поверхность: заливка на тон светлее фона, граница-волосок, мягкая тень. */
export function Card({ level = "panel", className = "", children, ...rest }: Props) {
  return (
    <div className={[level === "tile" ? "tile" : "card", className].filter(Boolean).join(" ")} {...rest}>
      {children}
    </div>
  );
}
