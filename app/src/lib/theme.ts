/** Применение темы к документу. Атрибут data-theme — тот же, что переключает переменные
 *  @heroui/styles, поэтому свои классы и компоненты HeroUI переключаются одним движением.
 *  color-scheme задан в index.css рядом с переменными: тогда нативные элементы (селекты,
 *  скроллбары) правильны с первого кадра, ещё до того как отработает этот модуль. */

let stopWatch: (() => void) | undefined;

export function applyTheme(theme: string) {
  stopWatch?.();
  stopWatch = undefined;
  if (theme !== "system") {
    document.documentElement.dataset.theme = theme === "light" ? "light" : "dark";
    return;
  }
  const mq = matchMedia("(prefers-color-scheme: dark)");
  const set = () => {
    document.documentElement.dataset.theme = mq.matches ? "dark" : "light";
  };
  set();
  mq.addEventListener("change", set);
  stopWatch = () => mq.removeEventListener("change", set);
}
