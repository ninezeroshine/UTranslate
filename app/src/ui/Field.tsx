import type { InputHTMLAttributes } from "react";
import { Icon, type IconName } from "../lib/icons";

type Props = InputHTMLAttributes<HTMLInputElement> & {
  /** С иконкой поле становится оправой: рамка и кольцо фокуса живут на ней (.field:focus-within). */
  icon?: IconName;
  wrapClassName?: string;
};

/** Текстовое поле. */
export function Field({ icon, wrapClassName = "", className = "", ...rest }: Props) {
  if (!icon) return <input className={`field ${className}`} {...rest} />;
  return (
    <div className={`field flex items-center gap-2.5 ${wrapClassName}`}>
      <Icon name={icon} size={15} className="text-ink-3" />
      <input
        className={`min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-ink-3 ${className}`}
        {...rest}
      />
    </div>
  );
}
