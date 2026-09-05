import { Icon } from "../lib/icons";
import { IconButton, Pill } from "../ui";
import type { TranslateResult } from "../lib/api";
import type { PopupOrigin, PopupStatus, ReplaceState } from "./types";

type Props = {
  origin: PopupOrigin;
  status: PopupStatus;
  result: TranslateResult | null;
  hasVisibleTranslation: boolean;
  visibleTranslation: string;
  /** Общая блокировка действий: сохранение правки, смена движка, распознавание. */
  controlsBusy: boolean;
  canReplace: boolean;
  replaceState: ReplaceState;
  replaceError: string | null;
  screenActionPending: boolean;
  screenActionError: string | null;
  screenHotkeyHint: string;
  favorite: boolean;
  starPop: boolean;
  copied: boolean;
  editMode: boolean;
  showOriginal: boolean;
  canShowOriginal: boolean;
  onReplace: () => void;
  onScreenCapture: () => void;
  onCopy: () => void;
  onSpeak: () => void;
  onFavorite: () => void;
  onToggleEdit: () => void;
  onToggleOriginal: () => void;
};

/** Подвал карточки: главное действие источника, иконки над переводом и переключатель оригинала.
 *  Все кнопки одной высоты 36, поэтому смена источника или состояния не двигает ряд. */
export function PopupFooter({
  origin, status, result, hasVisibleTranslation, visibleTranslation, controlsBusy,
  canReplace, replaceState, replaceError, screenActionPending, screenActionError, screenHotkeyHint,
  favorite, starPop, copied, editMode, showOriginal, canShowOriginal,
  onReplace, onScreenCapture, onCopy, onSpeak, onFavorite, onToggleEdit, onToggleOriginal,
}: Props) {
  const busy = controlsBusy || replaceState === "pending";
  return (
    <div className="popup-footer">
      <div className="popup-footer-actions">
        {origin === "screen" ? (
          <Pill
            size="md"
            disabled={screenActionPending}
            aria-label="Выделить заново"
            title={`${screenHotkeyHint} — выбрать другую область экрана`}
            onClick={onScreenCapture}
          >
            {screenActionPending ? "Открываем…" : "Выделить заново"}
          </Pill>
        ) : (
          <Pill
            variant="water"
            size="md"
            disabled={!canReplace}
            aria-label="Заменить"
            title={result && hasVisibleTranslation ? `Заменить на «${visibleTranslation}»` : "Дождитесь непустого результата перевода"}
            onClick={onReplace}
          >
            {replaceState === "pending"
              ? "Заменяем…"
              : replaceState === "done"
                ? "Заменено"
                : replaceState === "failed"
                  ? "Выделите заново"
                  : "Заменить"}
          </Pill>
        )}
        {status === "result" && result && (<>
          <IconButton
            icon={copied ? "check" : "copy"}
            label={copied ? "Скопировано" : "Копировать"}
            size={36}
            onClick={onCopy}
            disabled={!hasVisibleTranslation || controlsBusy}
          />
          {!result.wordMode && (
            <IconButton
              icon="speaker"
              label="Озвучить"
              size={36}
              onClick={onSpeak}
              disabled={!hasVisibleTranslation || controlsBusy}
            />
          )}
          <IconButton
            icon="star"
            label="В избранное"
            size={36}
            tone="sand"
            active={favorite}
            className={starPop ? "star-pop" : ""}
            onClick={onFavorite}
            disabled={!result.historyId || !hasVisibleTranslation || controlsBusy}
          />
          <IconButton
            icon="edit"
            label="Редактировать перевод"
            size={36}
            active={editMode}
            onClick={onToggleEdit}
            disabled={busy}
          />
        </>)}
      </div>
      {canShowOriginal && (
        <button
          type="button"
          className="popup-original-toggle"
          style={{ color: showOriginal ? "var(--water)" : "var(--ink-3)" }}
          title={origin === "screen"
            ? (showOriginal ? "Скрыть распознанный текст" : "Показать и исправить распознанный текст")
            : (showOriginal ? "Скрыть оригинал" : "Показать оригинал")}
          aria-expanded={showOriginal}
          onClick={onToggleOriginal}
        >
          Оригинал
          <Icon name="chevron" size={12} className={`chevron ${showOriginal ? "rotate-180" : ""}`} />
        </button>
      )}
      {replaceError && <div className="popup-footer-error" role="alert">{replaceError}</div>}
      {screenActionError && <div className="popup-footer-error" role="alert">{screenActionError}</div>}
    </div>
  );
}
