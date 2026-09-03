import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";
import { applyTheme } from "./lib/theme";

const surface = new URLSearchParams(location.search).get("w") === "popup" ? "popup" : "main";
document.documentElement.dataset.surface = surface;
// Тему по системе ставим до первого кадра: settings_get идёт по IPC, и без этого окно
// успевало моргнуть светлым на тёмной системе. Настоящую тему поставит applyTheme в окне.
applyTheme("system");
document.addEventListener("contextmenu", (e) => {
  if (!(e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement)) e.preventDefault();
});

const Root = surface === "popup" ? React.lazy(() => import("./popup/Popup")) : React.lazy(() => import("./main/Main"));

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <React.Suspense fallback={null}>
      <Root />
    </React.Suspense>
  </React.StrictMode>,
);
