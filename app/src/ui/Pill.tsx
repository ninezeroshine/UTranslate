import type { ButtonHTMLAttributes } from "react";
import { Icon, type IconName } from "../lib/icons";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  icon?: IconName;
  /** neutral — плитка, water — главное действие, active — выбранное состояние */
  variant?: "neutral" | "water" | "active";
  /** sm — 34 px (шапка, списки), md — 36 px (ряд действий) */
  size?: "sm" | "md";
};

/** Кнопка-пилюля. Состояния (наведение, нажатие, выключено, фокус) — в .pill, index.css. */
export function Pill({ icon, variant = "neutral", size = "sm", className = "", children, ...rest }: Props) {
  const cls = ["pill", variant === "neutral" ? "" : variant, size === "md" ? "md" : "", className];
  return (
    <button type="button" className={cls.filter(Boolean).join(" ")} {...rest}>
      {icon && <Icon name={icon} size={15} className={variant === "water" ? "" : "text-ink-2"} />}
      {children}
    </button>
  );
}
