import { win } from "../lib/tauri";
import { Icon } from "../lib/icons";
import { Segment } from "../ui";

export type Tab = "translate" | "history" | "favorites" | "settings";

const TABS: readonly { id: Tab; label: string }[] = [
  { id: "translate", label: "Перевод" },
  { id: "history", label: "История" },
  { id: "favorites", label: "Избранное" },
  { id: "settings", label: "Настройки" },
];

/** Шапка окна без системной рамки: имя, вкладки, кнопки окна.
 *  data-tauri-drag-region — на самой полосе и на пустых местах вокруг вкладок: перетаскивание
 *  и двойной клик (развернуть) работают везде, кроме кнопок. Блок с именем — pointer-events:none,
 *  чтобы нажатие проваливалось на полосу под ним. */
export function TitleBar({ tab, onTab }: { tab: Tab; onTab: (t: Tab) => void }) {
  return (
    <div data-tauri-drag-region className="flex h-14 shrink-0 items-center pl-5 pr-3.5">
      <div className="pointer-events-none flex w-[210px] items-center gap-2.5">
        <div className="flex h-[26px] w-[26px] items-center justify-center rounded-full bg-water text-on-water">
          <Icon name="translate" size={15} />
        </div>
        <span className="text-sm font-semibold tracking-[-0.015em]">UTranslate</span>
      </div>
      <div data-tauri-drag-region className="flex flex-1 justify-center">
        <Segment items={TABS} value={tab} onChange={onTab} layoutId="tab-pill" />
      </div>
      <div className="flex w-[210px] justify-end gap-1">
        <button type="button" className="wc" onClick={() => win?.minimize()} title="Свернуть" aria-label="Свернуть">
          <Icon name="minimize" size={10} />
        </button>
        <button type="button" className="wc" onClick={() => win?.toggleMaximize()} title="Развернуть" aria-label="Развернуть">
          <Icon name="maximize" size={10} />
        </button>
        <button type="button" className="wc close" onClick={() => win?.hide()} title="Скрыть в трей" aria-label="Скрыть в трей">
          <Icon name="close" size={10} />
        </button>
      </div>
    </div>
  );
}
