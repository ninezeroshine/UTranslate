import type { RefObject } from "react";

type Props = {
  editorRef: RefObject<HTMLTextAreaElement | null>;
  value: string;
  saving: boolean;
  /** Пока идёт сохранение или замена, поле и кнопки заблокированы. */
  disabled: boolean;
  canSave: boolean;
  error: string | null;
  onChange: (value: string) => void;
  onCancel: () => void;
  onSave: () => void;
};

/** Правка перевода прямо в карточке: Ctrl+Enter — готово, Esc (в Popup.tsx) — отмена. */
export function TranslationEditor({
  editorRef, value, saving, disabled, canSave, error, onChange, onCancel, onSave,
}: Props) {
  return (
    <div className="popup-edit-box">
      <textarea
        ref={editorRef}
        aria-label="Редактировать перевод"
        value={value}
        rows={3}
        disabled={disabled}
        className="field popup-edit-textarea"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && e.ctrlKey) {
            e.preventDefault();
            onSave();
          }
        }}
      />
      <div className="popup-edit-controls">
        <span className="text-[11px] text-ink-3">Ctrl+Enter — готово</span>
        <span className="flex-1" />
        <button type="button" onClick={onCancel} disabled={disabled}>Отменить</button>
        <button type="button" className="primary" onClick={onSave} disabled={!canSave || disabled}>
          {saving ? "Сохраняем…" : "Готово"}
        </button>
      </div>
      {error && <div className="popup-inline-error" role="alert">{error}</div>}
    </div>
  );
}
