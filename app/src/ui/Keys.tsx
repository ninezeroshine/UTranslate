/** Сочетание кейкапами: «Ctrl+Alt+T» — три капсулы через плюс.
 *  «Super» показываем как «Win»: так короче и понятнее пользователю Windows. */
export function Keys({ combo, className = "" }: { combo: string; className?: string }) {
  const parts = combo.split("+").filter(Boolean).map((p) => (p === "Super" ? "Win" : p));
  return (
    <span className={`inline-flex items-center gap-1.5 ${className}`}>
      {parts.map((p, i) => (
        <span key={p} className="flex items-center gap-1.5">
          {i > 0 && <span className="text-xs text-ink-3">+</span>}
          <span className="key">{p}</span>
        </span>
      ))}
    </span>
  );
}
