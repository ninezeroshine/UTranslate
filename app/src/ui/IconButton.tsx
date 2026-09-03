import type { ButtonHTMLAttributes } from "react";
import { Icon, type IconName } from "../lib/icons";

const SIZE = { 28: "sm", 34: "", 36: "md" } as const;

type Props = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  icon: IconName;
  /** Подпись: и для экранного диктора, и как подсказка при наведении. */
  label: string;
  size?: keyof typeof SIZE;
  /** Кнопка-переключатель: задан — рисуем включённое состояние и отдаём aria-pressed. */
  active?: boolean;
  /** water — обычное включение (закреплён), sand — избранное. */
  tone?: "water" | "sand";
};

/** Круглая кнопка с иконкой. */
export function IconButton({ icon, label, size = 34, active, tone = "water", className = "", ...rest }: Props) {
  const cls = ["rb", SIZE[size], active ? (tone === "sand" ? "warm" : "active") : "", className];
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active}
      className={cls.filter(Boolean).join(" ")}
      {...rest}
    >
      <Icon name={icon} size={size === 28 ? 14 : 16} />
    </button>
  );
}
