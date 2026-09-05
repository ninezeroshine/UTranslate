/** Типы, которые делят Popup.tsx и вынесенные из него компоненты. */
export type PopupStatus = "recognizing" | "loading" | "result" | "error" | "input";
export type PopupOrigin = "selection" | "screen";
export type ReplaceState = "idle" | "pending" | "done" | "failed";
