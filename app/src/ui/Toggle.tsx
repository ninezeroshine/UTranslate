type Props = {
  on: boolean;
  /** Подпись строки: у тумблера нет своего текста, диктор берёт её отсюда. */
  label: string;
  onClick: () => void;
};

/** Тумблер. Ползунок двигается transform-ом, состояния — .toggle-track/.toggle-thumb в index.css. */
export function Toggle({ on, label, onClick }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      className={`toggle-track ${on ? "on" : ""}`}
      onClick={onClick}
    >
      <span className={`toggle-thumb ${on ? "on" : ""}`} />
    </button>
  );
}
