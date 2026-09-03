import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";

const surface = new URLSearchParams(location.search).get("w") === "popup" ? "popup" : "main";
document.documentElement.dataset.surface = surface;
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
