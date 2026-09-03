import { listen as tauriListen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";

/** Страница может открываться и в обычном браузере — для отладки вёрстки. */
export const hasTauri = typeof (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined";

export const win: Window | null = hasTauri ? getCurrentWindow() : null;

export function listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  return hasTauri ? tauriListen<T>(event, handler) : Promise.resolve(() => undefined);
}
