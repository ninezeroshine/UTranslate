const { createRequire } = require("node:module");
const os = require("node:os");
const path = require("node:path");
const playwrightRequire = createRequire(require.main.filename);
const { test, expect } = playwrightRequire("playwright/test");

const settings = {
  hotkeyPopup: "Ctrl+Alt+T",
  hotkeyReplace: "Ctrl+Alt+R",
  hotkeyWindow: "Ctrl+Alt+U",
  hotkeyScreen: "Ctrl+Alt+S",
  primaryLang: "ru",
  secondaryLang: "en",
  engines: ["google", "bing", "mymemory"],
  theme: "dark",
  uiLang: "ru",
  historyEnabled: true,
  showOriginal: false,
  fontSize: 18,
};

const pageErrors = new WeakMap();

test.beforeEach(async ({ page }) => {
  const errors = [];
  pageErrors.set(page, errors);
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript((initialSettings) => {
    let callbackId = 0;
    let eventId = 0;
    const callbacks = new Map();
    const state = {
      settings: { ...initialSettings },
      calls: [],
      eventHandlers: {},
      focused: true,
      outerPosition: { x: 100, y: 100 },
      innerSize: { width: 558, height: 388 },
      screenGate: null,
      releaseScreen: null,
      emit(event, payload, updateFocus = true) {
        if (updateFocus && event === "tauri://focus") state.focused = true;
        if (updateFocus && event === "tauri://blur") state.focused = false;
        const callback = callbacks.get(state.eventHandlers[event]);
        if (callback) callback({ event, id: 1, payload });
      },
      waitForScreen() {
        state.screenGate = new Promise((resolve) => { state.releaseScreen = resolve; });
      },
    };
    window.__screenMock = state;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: location.search.includes("w=popup") ? "popup" : "main" },
        currentWebview: { label: location.search.includes("w=popup") ? "popup" : "main" },
      },
      transformCallback(callback) {
        const id = ++callbackId;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id) { callbacks.delete(id); },
      async invoke(cmd, args = {}) {
        state.calls.push({ cmd, args });
        if (cmd === "settings_get") return { ...state.settings };
        if (cmd === "settings_set") return [];
        if (cmd === "hotkeys_status") return [];
        if (cmd === "autostart_get") return false;
        if (cmd === "update_available") return null;
        if (cmd === "history_list") return [];
        if (cmd === "translate_screen") {
          if (state.screenGate) await state.screenGate;
          return null;
        }
        if (cmd === "ack_screen_capture") return null;
        if (cmd === "translate_text") {
          return {
            text: `LOCAL: ${args.text}`,
            detected: args.target === "en" ? "ru" : "en",
            target: args.target || "ru",
            engine: args.engine || "google",
            alternatives: [],
            fallbackFrom: null,
            historyId: 77,
            wordMode: false,
            isFavorite: false,
            requestId: null,
          };
        }
        if (cmd === "update_translation_text") return false;
        if (cmd === "copy_text" || cmd === "history_set_favorite") return null;
        if (cmd === "plugin:app|version") return "test";
        if (cmd === "plugin:event|listen") {
          state.eventHandlers[args.event] = args.handler;
          return ++eventId;
        }
        if (cmd === "plugin:event|unlisten") return null;
        if (cmd === "plugin:window|set_size") {
          const raw = args.value.size || args.value;
          state.innerSize = { width: raw.width, height: raw.height };
          return null;
        }
        if (cmd === "plugin:window|set_position") {
          const raw = args.value.position || args.value;
          state.outerPosition = { x: raw.x, y: raw.y };
          return null;
        }
        if (cmd === "plugin:window|outer_position") return state.outerPosition;
        if (cmd === "plugin:window|inner_size") return state.innerSize;
        if (cmd === "plugin:window|is_focused") return state.focused;
        if (cmd === "plugin:window|scale_factor") return 1;
        if (cmd === "plugin:window|current_monitor") {
          return {
            name: "test",
            scaleFactor: 1,
            position: { x: 0, y: 0 },
            size: { width: 1000, height: 760 },
            workArea: { position: { x: 0, y: 0 }, size: { width: 1000, height: 760 } },
          };
        }
        return null;
      },
    };
    speechSynthesis.cancel = () => {};
    speechSynthesis.speak = () => {};
  }, settings);
});

test.afterEach(async ({ page }) => {
  expect(pageErrors.get(page)).toEqual([]);
});

async function openPopup(page) {
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__screenMock.eventHandlers["popup:show"]));
}

function result(requestId, text = "Screen translation") {
  return {
    requestId,
    text,
    detected: "en",
    target: "ru",
    engine: "google",
    alternatives: [],
    fallbackFrom: null,
    historyId: requestId,
    wordMode: false,
    isFavorite: false,
  };
}

async function showScreenResult(page, requestId = 10) {
  await page.evaluate(({ requestId }) => {
    window.__screenMock.emit("popup:show", {
      requestId,
      text: "",
      target: "ru",
      detected: null,
      clipboardReplaced: false,
      canReplace: true,
      origin: "screen",
      phase: "recognizing",
    });
    window.__screenMock.emit("popup:recognized", {
      requestId,
      text: "Recognized source",
      target: "ru",
      detected: "en",
    });
  }, { requestId });
  await page.evaluate(({ value }) => window.__screenMock.emit("popup:result", value), {
    value: result(requestId),
  });
  await expect(page.getByText("Screen translation", { exact: true })).toBeVisible();
}

/** Доля внутренней ширины карточки, которую занял текст перевода. */
async function translationWidthRatio(page) {
  return page.locator("[data-popup-translation]").evaluate((el) => {
    const card = el.closest(".pop").firstElementChild;
    const style = getComputedStyle(card);
    const inner = card.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    return el.getBoundingClientRect().width / inner;
  });
}

test("main screen button preserves the draft and exposes pending state", async ({ page }) => {
  await page.goto("http://127.0.0.1:1420/");
  const source = page.getByRole("textbox", { name: "Исходный текст" });
  await source.fill("Не терять этот черновик");
  await page.evaluate(() => window.__screenMock.waitForScreen());

  await page.getByRole("button", { name: "С экрана", exact: true }).click();
  await expect(page.getByRole("button", { name: "Открываем…", exact: true })).toBeDisabled();
  await expect(source).toHaveValue("Не терять этот черновик");
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-main-screen-action.png") });
  expect(await page.evaluate(() => window.__screenMock.calls.some((call) => call.cmd === "translate_screen"))).toBe(true);

  await page.evaluate(() => window.__screenMock.releaseScreen());
  await expect(page.getByRole("button", { name: "С экрана", exact: true })).toBeEnabled();
});

test("recognizing becomes translation in the same screen session and never offers Replace", async ({ page }) => {
  await openPopup(page);
  await page.evaluate(() => window.__screenMock.emit("popup:show", {
    requestId: 12,
    text: "",
    target: "ru",
    detected: null,
    clipboardReplaced: false,
    canReplace: true,
    origin: "screen",
    phase: "recognizing",
  }));
  await expect(page.getByText("Распознаём…", { exact: true })).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Введите текст для перевода…" })).toHaveCount(0);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-screen-recognizing.png") });

  await page.evaluate(() => window.__screenMock.emit("popup:recognized", {
    requestId: 12,
    text: "Recognized source",
    target: "ru",
    detected: "en",
  }));
  await expect(page.getByText("переводим…", { exact: true })).toBeVisible();
  await page.evaluate(() => window.__screenMock.emit("popup:result", {
    ...window.__screenResult,
    requestId: 12,
    text: "Перевод с экрана",
    detected: "en",
    target: "ru",
    engine: "google",
    alternatives: [],
    fallbackFrom: null,
    historyId: 12,
    wordMode: false,
    isFavorite: false,
  }));

  await expect(page.getByText("Перевод с экрана", { exact: true })).toBeVisible();
  await expect(page.getByText("с экрана", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Заменить", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Оригинал", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Распознанный текст" })).toHaveValue("Recognized source");
  await page.waitForTimeout(700);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-screen-source-correction.png") });
});

test("source correction and closed or newer sessions reject stale OCR and results", async ({ page }) => {
  await openPopup(page);
  await showScreenResult(page, 20);
  await page.getByRole("button", { name: "Оригинал", exact: true }).click();
  const source = page.getByRole("textbox", { name: "Распознанный текст" });
  await source.fill("");
  await expect(source).toBeVisible();
  await expect(source).toHaveValue("");
  await source.fill("Corrected source");
  await expect.poll(() => page.evaluate(() => window.__screenMock.calls.filter(
    (call) => call.cmd === "translate_text" && call.args.text === "Corrected source",
  ).length)).toBe(1);
  await expect(page.getByText("LOCAL: Corrected source", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    window.__screenMock.emit("popup:recognized", { requestId: 20, text: "STALE OCR", target: "ru", detected: "en" });
    window.__screenMock.emit("popup:result", {
      requestId: 20, text: "STALE RESULT", detected: "en", target: "ru", engine: "google",
      alternatives: [], fallbackFrom: null, historyId: 20, wordMode: false, isFavorite: false,
    });
  });
  await expect(source).toHaveValue("Corrected source");
  await expect(page.getByText("STALE RESULT", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Закрыть (Esc)" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.evaluate(() => {
    window.__screenMock.emit("popup:recognized", { requestId: 20, text: "CLOSED OCR", target: "ru", detected: "en" });
    window.__screenMock.emit("popup:result", {
      requestId: 20, text: "CLOSED RESULT", detected: "en", target: "ru", engine: "google",
      alternatives: [], fallbackFrom: null, historyId: 20, wordMode: false, isFavorite: false,
    });
    window.__screenMock.emit("popup:show", {
      requestId: 21, text: "New source", target: "ru", detected: "en",
      clipboardReplaced: false, canReplace: false,
    });
    window.__screenMock.emit("popup:result", {
      requestId: 20, text: "OLD SESSION", detected: "en", target: "ru", engine: "google",
      alternatives: [], fallbackFrom: null, historyId: 20, wordMode: false, isFavorite: false,
    });
  });
  await expect(page.getByText("OLD SESSION", { exact: true })).toHaveCount(0);
});

test("no-text screen error retries by starting a new capture", async ({ page }) => {
  await openPopup(page);
  await page.evaluate(() => {
    window.__screenMock.emit("popup:show", {
      requestId: 30, text: "", target: "ru", detected: null,
      clipboardReplaced: false, canReplace: false, origin: "screen", phase: "recognizing",
    });
    window.__screenMock.emit("popup:error", { requestId: 30, message: "Текст в области не найден" });
  });
  await expect(page.getByRole("button", { name: "Выделить заново", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Выделить заново", exact: true }).click();
  await expect.poll(() => page.evaluate(() => window.__screenMock.calls.filter((call) => call.cmd === "translate_screen").length)).toBe(1);
});

test("capture suspend ACK and resume preserve draft while clearing stale pending work", async ({ page }) => {
  await openPopup(page);
  await page.evaluate(() => {
    window.__screenMock.emit("popup:show", {
      requestId: 40, text: "Captured source", target: "ru", detected: "en",
      clipboardReplaced: false, canReplace: true,
    });
    window.__screenMock.emit("popup:result", {
      requestId: 40, text: "Saved result", detected: "en", target: "ru", engine: "google",
      alternatives: [], fallbackFrom: null, historyId: 40, wordMode: false, isFavorite: false,
    });
  });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  const draft = page.getByRole("textbox", { name: "Редактировать перевод" });
  await draft.fill("Несохранённый черновик");

  await page.evaluate(() => {
    window.__screenMock.emit("popup:capture-suspend", { requestId: 400 });
    window.__screenMock.emit("tauri://blur", false);
  });
  await expect.poll(() => page.evaluate(() => window.__screenMock.calls.some(
    (call) => call.cmd === "ack_screen_capture" && call.args.requestId === 400,
  ))).toBe(true);
  await page.evaluate(() => window.__screenMock.emit("popup:capture-resume", { requestId: 400 }));

  await expect(draft).toHaveValue("Несохранённый черновик");
  await expect(page.getByRole("button", { name: "Копировать" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Заменить", exact: true })).toBeDisabled();

  await page.evaluate(() => {
    window.__screenMock.emit("popup:show", {
      requestId: 41, text: "Still translating", target: "ru", detected: "en",
      clipboardReplaced: false, canReplace: true,
    });
    window.__screenMock.emit("popup:capture-suspend", { requestId: 401 });
    window.__screenMock.emit("popup:capture-resume", { requestId: 401 });
  });
  await expect(page.getByText("Выбор области отменён", { exact: true })).toBeVisible();
  await expect(page.getByText("переводим…", { exact: true })).toHaveCount(0);
});

test("screen footer carries recapture, edit and Original instead of the old origin row", async ({ page }) => {
  await openPopup(page);
  await showScreenResult(page, 50);

  await expect(page.locator(".popup-screen-origin")).toHaveCount(0);
  const footer = page.locator(".popup-footer");
  await expect(footer.getByRole("button", { name: "Выделить заново", exact: true })).toBeVisible();
  await expect(footer.getByRole("button", { name: "Редактировать перевод" })).toBeVisible();
  await expect(footer.getByRole("button", { name: "Оригинал" })).toBeVisible();
  await expect(footer.getByRole("button", { name: "Заменить", exact: true })).toHaveCount(0);
  // Бейдж источника переехал в шапку, к пилюле языков.
  const badge = page.locator(".badge", { hasText: "с экрана" });
  await expect(badge).toBeVisible();
  expect(await badge.evaluate((el) => Boolean(el.closest(".popup-footer")))).toBe(false);

  const rows = await footer.evaluate((el) => new Set(
    [...el.querySelectorAll("button")].map((button) => Math.round(button.getBoundingClientRect().top)),
  ).size);
  expect(rows).toBe(1);
  await page.waitForTimeout(600);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-screen-footer.png") });
});

test("screen translation text fills the card width", async ({ page }) => {
  await openPopup(page);
  await showScreenResult(page, 51);
  expect(await translationWidthRatio(page)).toBeGreaterThanOrEqual(0.9);
});

test("Original opens the recognized text and typing in it retranslates", async ({ page }) => {
  await openPopup(page);
  await showScreenResult(page, 52);
  await page.getByRole("button", { name: "Оригинал", exact: true }).click();
  const recognized = page.getByRole("textbox", { name: "Распознанный текст" });
  await expect(recognized).toHaveValue("Recognized source");

  await recognized.fill("Fixed by hand");
  await expect.poll(() => page.evaluate(() => window.__screenMock.calls.filter(
    (call) => call.cmd === "translate_text" && call.args.text === "Fixed by hand",
  ).length)).toBe(1);
  await expect(page.getByText("LOCAL: Fixed by hand", { exact: true })).toBeVisible();
});
