const { createRequire } = require("node:module");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs");
const playwrightRequire = createRequire(require.main.filename);
const { test, expect } = playwrightRequire("playwright/test");

const settings = {
  hotkeyPopup: "Ctrl+Alt+T",
  hotkeyReplace: "Ctrl+Alt+R",
  hotkeyWindow: "Ctrl+Alt+U",
  primaryLang: "ru",
  secondaryLang: "en",
  engines: ["google", "bing", "mymemory"],
  theme: "dark",
  uiLang: "ru",
  historyEnabled: true,
  showOriginal: false,
  fontSize: 21,
};

const longText = Array.from({ length: 18 }, (_, i) => `Translation line ${i + 1} keeps the result tall.`).join(" ");
const pageErrors = new WeakMap();

test.beforeEach(async ({ page }) => {
  const errors = [];
  pageErrors.set(page, errors);
  page.on("pageerror", (error) => errors.push(error.message));
});

test.afterEach(async ({ page }) => {
  expect(pageErrors.get(page)).toEqual([]);
});

test("Tauri capability permits every popup window geometry call", () => {
  const capability = JSON.parse(fs.readFileSync(
    path.resolve(__dirname, "../../app/src-tauri/capabilities/default.json"),
    "utf8",
  ));
  expect(capability.permissions).toEqual(expect.arrayContaining([
    // core:window:default (via core:default) grants currentMonitor, outerPosition and scaleFactor.
    "core:default",
    "core:window:allow-set-size",
    "core:window:allow-set-position",
  ]));
});

async function installTauriMock(page, work = { x: 0, y: 0, width: 900, height: 700, scale: 1 }) {
  await page.addInitScript(({ settings, longText, work }) => {
    let callbackId = 0;
    let eventId = 0;
    const callbacks = new Map();
    const state = {
      sizes: [],
      positions: [],
      translationCalls: [],
      translationPlan: [],
      updateTranslationCalls: [],
      updateTranslationPlan: [],
      copyCalls: [],
      speakCalls: [],
      replaceCalls: [],
      replacePlan: [],
      favoriteCalls: [],
      favoritePlan: [],
      persistedFavorite: false,
      focused: true,
      eventHandlers: {},
      outerPosition: {
        x: work.x + 100,
        y: work.y + work.height - 388 * work.scale,
      },
      innerSize: { width: 558, height: 388 },
      emit(event, payload, updateFocus = true) {
        if (updateFocus && event === "tauri://focus") state.focused = true;
        if (updateFocus && event === "tauri://blur") state.focused = false;
        const handler = callbacks.get(state.eventHandlers[event]);
        if (handler) handler({ event, id: 1, payload });
      },
    };
    window.__popupMock = state;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "popup" }, currentWebview: { label: "popup" } },
      transformCallback(callback) {
        const id = ++callbackId;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id) { callbacks.delete(id); },
      async invoke(cmd, args = {}) {
        if (cmd === "settings_get") return settings;
        if (cmd === "translate_text") {
          state.translationCalls.push({ text: args.text, target: args.target || null, engine: args.engine || null });
          const plan = state.translationPlan.shift() || {};
          if (plan.delay) await new Promise((resolve) => setTimeout(resolve, plan.delay));
          if (plan.fail) throw new Error(plan.message || "translation failed");
          if (args.text === "slow") await new Promise((resolve) => setTimeout(resolve, 250));
          if (args.text === "slow error") {
            await new Promise((resolve) => setTimeout(resolve, 250));
            throw new Error("stale error");
          }
          return {
            text: longText,
            detected: args.target === "en" ? "ru" : "en",
            target: args.target || "ru",
            engine: args.engine || "google",
            alternatives: [],
            fallbackFrom: null,
            historyId: 2,
            wordMode: false,
            isFavorite: false,
            requestId: null,
          };
        }
        if (cmd === "update_translation_text") {
          state.updateTranslationCalls.push({
            historyId: args.historyId,
            sourceText: args.sourceText,
            expectedText: args.expectedText,
            text: args.text,
          });
          const plan = state.updateTranslationPlan.shift() || {};
          if (plan.delay) await new Promise((resolve) => setTimeout(resolve, plan.delay));
          if (plan.fail) throw new Error(plan.message || "translation save failed");
          return plan.result === undefined ? true : plan.result;
        }
        if (cmd === "copy_text") {
          state.copyCalls.push(args.text);
          return null;
        }
        if (cmd === "replace_popup_translation") {
          state.replaceCalls.push({
            requestId: args.requestId,
            sourceText: args.sourceText,
            translatedText: args.translatedText,
          });
          const plan = state.replacePlan.shift() || {};
          if (plan.delay) await new Promise((resolve) => setTimeout(resolve, plan.delay));
          if (plan.fail) throw new Error(plan.message || "Не удалось вставить перевод");
          return null;
        }
        if (cmd === "history_set_favorite") {
          state.favoriteCalls.push({ id: args.id, favorite: args.favorite });
          const plan = state.favoritePlan.shift() || {};
          if (plan.delay) await new Promise((resolve) => setTimeout(resolve, plan.delay));
          if (plan.fail) throw new Error("favorite write failed");
          state.persistedFavorite = args.favorite;
          return null;
        }
        if (cmd === "open_main") {
          state.openMainCalls = state.openMainCalls || [];
          state.openMainCalls.push(args.text || null);
          return null;
        }
        if (cmd === "plugin:event|listen") {
          state.eventHandlers[args.event] = args.handler;
          return ++eventId;
        }
        if (cmd === "plugin:event|unlisten") return null;
        if (cmd === "plugin:window|set_size") {
          const raw = args.value.size || args.value;
          const next = { width: raw.width, height: raw.height };
          state.innerSize = next;
          state.sizes.push(next);
          return null;
        }
        if (cmd === "plugin:window|set_position") {
          const raw = args.value.position || args.value;
          const next = { x: raw.x, y: raw.y };
          state.outerPosition = next;
          state.positions.push(next);
          return null;
        }
        if (cmd === "plugin:window|outer_position") return state.outerPosition;
        if (cmd === "plugin:window|inner_size") return state.innerSize;
        if (cmd === "plugin:window|is_focused") return state.focused;
        if (cmd === "plugin:window|scale_factor") return work.scale;
        if (cmd === "plugin:window|current_monitor") {
          return {
            name: "test",
            scaleFactor: work.scale,
            position: { x: work.x, y: work.y },
            size: { width: work.width, height: work.height },
            workArea: { position: { x: work.x, y: work.y }, size: { width: work.width, height: work.height } },
          };
        }
        return null;
      },
    };
    const speech = window.speechSynthesis;
    speech.cancel = () => {};
    speech.speak = (utterance) => {
      window.__popupMock.speakCalls.push({ text: utterance.text, lang: utterance.lang });
    };
  }, { settings, longText, work });
}

async function showLongResult(page, requestId = 1) {
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(({ longText, requestId }) => {
    window.__utDemo.show({
      text: longText,
      target: "ru",
      detected: "en",
      clipboardReplaced: false,
      requestId,
      canReplace: true,
    });
    window.__utDemo.result({
      text: longText,
      detected: "en",
      target: "ru",
      engine: "google",
      alternatives: [],
      fallbackFrom: null,
      historyId: 1,
      wordMode: false,
      isFavorite: false,
      requestId,
    });
  }, { longText, requestId });
  await page.waitForTimeout(550);
}

test("fast EN translation cannot apply a stale loading shrink", async ({ page }) => {
  await installTauriMock(page);
  await page.setViewportSize({ width: 900, height: 700 });
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showLongResult(page);

  await page.getByTitle("Сменить целевой язык").click();
  await page.waitForTimeout(700);

  const geometry = await page.locator(".pop").evaluate((pop) => ({
    cardHeight: pop.firstElementChild.offsetHeight,
    popHeight: pop.getBoundingClientRect().height,
    scrollHeight: pop.scrollHeight,
  }));
  const sizes = await page.evaluate(() => window.__popupMock.sizes);
  const last = sizes.at(-1);
  expect(geometry.scrollHeight).toBeLessThanOrEqual(geometry.popHeight + 1);
  expect(last.height).toBeGreaterThanOrEqual(geometry.cardHeight + 128 - 1);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-popup-en.png") });
});

async function showResult(page, {
  source = "Original source",
  translated = "Точный перевод",
  requestId = 41,
  canReplace = true,
  target = "ru",
} = {}) {
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(({ source, translated, requestId, canReplace, target }) => {
    window.__utDemo.show({
      text: source,
      target,
      detected: "en",
      clipboardReplaced: false,
      requestId,
      canReplace,
    });
    window.__utDemo.result({
      text: translated,
      detected: "en",
      target,
      engine: "google",
      alternatives: [],
      fallbackFrom: null,
      historyId: 41,
      wordMode: false,
      isFavorite: false,
      requestId,
    });
  }, { source, translated, requestId, canReplace, target });
  await page.waitForTimeout(350);
}

test("Replace sends the displayed translation and captured source without translating again", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  const source = "  Original Ω  ";
  const translated = "Переведено точно\nстрока 2";
  await showResult(page, { source, translated, requestId: 712 });

  const replace = page.getByRole("button", { name: "Заменить" });
  await expect(replace).toBeEnabled();
  await expect(replace).toHaveAttribute("title", `Заменить на «${translated}»`);
  for (const name of ["Заменить", "Копировать", "Озвучить", "В избранное", "Оригинал"]) {
    await expect(page.getByRole("button", { name })).toBeInViewport();
  }
  const footerFits = await page.locator(".popup-footer").evaluate((footer) => {
    const card = footer.closest(".pop");
    const f = footer.getBoundingClientRect();
    const c = card.getBoundingClientRect();
    return f.left >= c.left && f.right <= c.right && f.width <= c.width;
  });
  expect(footerFits).toBe(true);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-popup-replace.png") });
  await replace.click();
  await expect(replace).toBeDisabled();

  const state = await page.evaluate(() => window.__popupMock);
  expect(state.replaceCalls).toEqual([{ requestId: 712, sourceText: source, translatedText: translated }]);
  expect(state.translationCalls).toEqual([]);
});

test("Replace stays disabled while loading and for manual or non-captured results", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__utDemo));

  await page.evaluate(() => window.__utDemo.show({
    text: "captured", target: "ru", detected: "en", clipboardReplaced: false, requestId: 50, canReplace: true,
  }));
  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();

  await showResult(page, { source: "not replaceable", requestId: 51, canReplace: false });
  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();

  await page.evaluate(() => window.__utDemo.show({
    text: "", target: "ru", detected: null, clipboardReplaced: false, requestId: 52, canReplace: false,
  }));
  const input = page.getByPlaceholder("Введите текст для перевода…");
  await input.fill("manual source");
  await input.press("Enter");
  await expect(page.getByText(longText)).toBeVisible();
  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();
  expect(await page.evaluate(() => window.__popupMock.replaceCalls)).toEqual([]);
});

test("Replace guards double clicks and keeps the result accessible but consumes the session after an error", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "keep me", translated: "оставь меня", requestId: 60 });
  await page.evaluate(() => {
    window.__popupMock.replacePlan = [{ delay: 250, fail: true, message: "Окно назначения больше недоступно" }];
    const button = document.querySelector('button[aria-label="Заменить"]');
    button.click();
    button.click();
  });

  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();
  expect(await page.evaluate(() => window.__popupMock.replaceCalls)).toHaveLength(1);
  await expect(page.getByRole("alert")).toContainText("Окно назначения больше недоступно");
  await expect(page.getByRole("alert")).toContainText("Выделите текст заново и вызовите перевод своим хоткеем");
  await expect(page.getByText("оставь меня")).toBeVisible();
  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();
  await expect(page.getByText("Выделите заново")).toBeVisible();
  await expect(page.getByText(/Заменено:/)).toHaveCount(0);
  expect(await page.evaluate(() => window.__popupMock.replaceCalls)).toHaveLength(1);
});

test("late native blur after Replace failure keeps recovery visible, then a real focus-blur hides it", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "focused source", translated: "видимый результат", requestId: 61 });
  await page.evaluate(() => {
    window.__popupMock.replacePlan = [{ delay: 250, fail: true, message: "Не удалось вернуть фокус" }];
  });
  await page.getByRole("button", { name: "Заменить" }).click();
  await expect(page.getByRole("alert")).toContainText("Не удалось вернуть фокус");
  await page.evaluate(() => window.__popupMock.emit("tauri://blur", false));
  await expect(page.getByText("видимый результат")).toBeVisible();
  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();

  await page.evaluate(() => window.__popupMock.emit("tauri://focus", true));
  await page.evaluate(() => window.__popupMock.emit("tauri://blur", false));
  await expect(page.getByRole("dialog")).toHaveCount(0);

  await showResult(page, { source: "close recovery", translated: "закрыть", requestId: 63 });
  await page.evaluate(() => {
    window.__popupMock.replacePlan = [{ fail: true, message: "Ошибка для закрытия" }];
  });
  await page.getByRole("button", { name: "Заменить" }).click();
  await expect(page.getByRole("alert")).toContainText("Ошибка для закрытия");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("retrying a capture translation error preserves Replace for the same source", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(() => {
    window.__utDemo.show({
      text: "captured retry", target: "ru", detected: "en", clipboardReplaced: false,
      requestId: 62, canReplace: true,
    });
    window.__popupMock.emit("popup:error", { message: "temporary network error", requestId: 62 });
  });
  await page.getByRole("button", { name: "Повторить" }).click();

  await expect(page.getByText(longText)).toBeVisible();
  await expect(page.getByRole("button", { name: "Заменить" })).toBeEnabled();
  await page.getByRole("button", { name: "Заменить" }).click();
  const state = await page.evaluate(() => window.__popupMock);
  expect(state.translationCalls).toEqual([{ text: "captured retry", target: "ru", engine: null }]);
  expect(state.replaceCalls).toEqual([{ requestId: 62, sourceText: "captured retry", translatedText: longText }]);
});

test("a new popup session invalidates an in-flight replacement and stale results", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "old", translated: "старый", requestId: 70 });
  await page.evaluate(() => { window.__popupMock.replacePlan = [{ delay: 300 }]; });
  await page.getByRole("button", { name: "Заменить" }).click();

  await showResult(page, { source: "new", translated: "новый", requestId: 71, canReplace: false });
  await page.evaluate(() => window.__popupMock.emit("popup:result", {
    text: "STALE REPLACEABLE", detected: "en", target: "ru", engine: "google", alternatives: [],
    fallbackFrom: null, historyId: 70, wordMode: false, isFavorite: false, requestId: 70,
  }));
  await page.waitForTimeout(350);
  await page.evaluate(() => window.__popupMock.emit("tauri://blur", false, false));

  await expect(page.getByText("новый")).toBeVisible();
  await expect(page.getByText("STALE REPLACEABLE")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Заменить" })).toBeDisabled();
});

test("target switch preserves replacement for the captured source and uses the new displayed result", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "capture once", translated: "первый", requestId: 80 });

  await page.getByTitle("Сменить целевой язык").click();
  await expect(page.getByText(longText)).toBeVisible();
  const replace = page.getByRole("button", { name: "Заменить" });
  await expect(replace).toBeEnabled();
  await replace.click();

  const state = await page.evaluate(() => window.__popupMock);
  expect(state.translationCalls).toEqual([{ text: "capture once", target: "en", engine: null }]);
  expect(state.replaceCalls).toEqual([{ requestId: 80, sourceText: "capture once", translatedText: longText }]);
});

test("small work area keeps Original and the action footer reachable", async ({ page }) => {
  const work = { x: -1400, y: -180, width: 900, height: 420, scale: 1.5 };
  await installTauriMock(page, work);
  await page.setViewportSize({ width: 600, height: 280 });
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showLongResult(page);
  await page.getByRole("button", { name: "Оригинал" }).click();
  await page.waitForTimeout(700);

  const state = await page.evaluate(() => window.__popupMock);
  expect(Math.max(...state.sizes.map((size) => size.height * work.scale))).toBeLessThanOrEqual(work.height);
  expect(state.positions.length).toBeGreaterThan(0);
  const lastSize = state.sizes.at(-1);
  const lastPosition = state.positions.at(-1);
  expect(lastPosition.x).toBeGreaterThanOrEqual(work.x);
  expect(lastPosition.y).toBeGreaterThanOrEqual(work.y);
  expect(lastPosition.x + lastSize.width * work.scale).toBeLessThanOrEqual(work.x + work.width);
  expect(lastPosition.y + lastSize.height * work.scale).toBeLessThanOrEqual(work.y + work.height);
  const scroller = page.locator("[data-popup-scroll]");
  await expect(scroller).toHaveCount(1);
  const readableBody = await scroller.evaluate((el) => el.clientHeight);
  expect(readableBody).toBeGreaterThanOrEqual(100);
  const originalVisibleHeight = await page.locator("[data-popup-original]").evaluate((el) => {
    const rect = el.getBoundingClientRect();
    const viewportBottom = Math.min(window.innerHeight, el.closest("[data-popup-scroll]").getBoundingClientRect().bottom);
    return Math.max(0, viewportBottom - Math.max(0, rect.top));
  });
  expect(originalVisibleHeight).toBeGreaterThanOrEqual(90);
  await expect(page.getByRole("button", { name: "Заменить" })).toBeInViewport();
  await expect(page.getByRole("button", { name: "Копировать" })).toBeInViewport();
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-popup-original-bottom.png") });
  await scroller.evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await expect(page.getByRole("button", { name: "Заменить" })).toBeInViewport();
  await expect(page.getByRole("button", { name: "Копировать" })).toBeInViewport();
});

test("pin remains enabled across popup:show sessions", async ({ page }) => {
  await installTauriMock(page);
  await page.setViewportSize({ width: 900, height: 700 });
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showLongResult(page);
  const pin = page.getByRole("button", { name: "Закрепить" });
  await pin.click();
  await expect(pin).toHaveAttribute("aria-pressed", "true");
  await page.evaluate(() => window.__utDemo.show({
    text: "next",
    target: "ru",
    detected: "en",
    clipboardReplaced: false,
    requestId: 2,
  }));
  await expect(page.getByRole("button", { name: "Закрепить" })).toHaveAttribute("aria-pressed", "true");
});

test("stale backend result and error events cannot replace the current request", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(() => {
    window.__utDemo.show({ text: "old", target: "ru", detected: "en", clipboardReplaced: false, requestId: 10 });
    window.__utDemo.show({ text: "current", target: "ru", detected: "en", clipboardReplaced: false, requestId: 11 });
    window.__popupMock.emit("popup:result", {
      text: "STALE RESULT", detected: "en", target: "ru", engine: "google", alternatives: [],
      fallbackFrom: null, historyId: 1, wordMode: false, isFavorite: false, requestId: 10,
    });
    window.__popupMock.emit("popup:error", { message: "STALE ERROR", requestId: 10 });
  });
  await expect(page.getByText("STALE RESULT")).toHaveCount(0);
  await expect(page.getByText("STALE ERROR")).toHaveCount(0);

  await page.evaluate(() => window.__popupMock.emit("popup:result", {
    text: "CURRENT RESULT", detected: "en", target: "ru", engine: "google", alternatives: [],
    fallbackFrom: null, historyId: 2, wordMode: false, isFavorite: true, requestId: 11,
  }));
  await expect(page.getByText("CURRENT RESULT")).toBeVisible();
  await expect(page.getByRole("button", { name: "В избранное" })).toHaveAttribute("aria-pressed", "true");
});

test("editing invalidates a direct translation result and error immediately", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(() => window.__utDemo.show({
    text: "", target: "ru", detected: null, clipboardReplaced: false, requestId: 20,
  }));
  const input = page.getByPlaceholder("Введите текст для перевода…");
  await input.fill("slow");
  await input.press("Enter");
  await input.fill("new draft");
  await page.waitForTimeout(350);
  await expect(page.getByText(longText)).toHaveCount(0);

  await input.fill("slow error");
  await input.press("Enter");
  await input.fill("newer draft");
  await page.waitForTimeout(350);
  await expect(page.getByText("stale error")).toHaveCount(0);
});

test("switching target cancels the pending edit debounce", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(() => window.__utDemo.show({
    text: "", target: "ru", detected: null, clipboardReplaced: false, requestId: 30,
  }));
  await page.getByPlaceholder("Введите текст для перевода…").fill("draft");
  await page.getByTitle("Сменить целевой язык").click();
  await page.waitForTimeout(750);

  const calls = await page.evaluate(() => window.__popupMock.translationCalls);
  expect(calls).toEqual([{ text: "draft", target: "en", engine: null }]);
});

test("favorite writes stay ordered when the first write is slower", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showLongResult(page);
  await page.evaluate(() => { window.__popupMock.favoritePlan = [{ delay: 1000 }, { delay: 0 }]; });
  const favorite = page.getByRole("button", { name: "В избранное" });
  await favorite.click({ force: true });
  await page.waitForTimeout(0);
  await favorite.click({ force: true });
  await page.waitForTimeout(50);
  expect(await page.evaluate(() => window.__popupMock.favoriteCalls)).toEqual([{ id: 1, favorite: true }]);
  await page.waitForTimeout(1200);

  const state = await page.evaluate(() => window.__popupMock);
  expect(state.favoriteCalls).toEqual([{ id: 1, favorite: true }, { id: 1, favorite: false }]);
  expect(state.persistedFavorite).toBe(false);
  await expect(favorite).toHaveAttribute("aria-pressed", "false");
});

test("multiple favorite failures roll back to the last confirmed state", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showLongResult(page);
  await page.evaluate(() => {
    window.__popupMock.favoritePlan = [{ delay: 300, fail: true }, { delay: 150, fail: true }];
  });
  const favorite = page.getByRole("button", { name: "В избранное" });
  await favorite.click({ force: true });
  await page.waitForTimeout(0);
  await favorite.click({ force: true });
  await page.waitForTimeout(50);
  expect(await page.evaluate(() => window.__popupMock.favoriteCalls)).toEqual([{ id: 1, favorite: true }]);
  await page.waitForTimeout(500);

  await expect(favorite).toHaveAttribute("aria-pressed", "false");
  const calls = await page.evaluate(() => window.__popupMock.favoriteCalls);
  expect(calls).toEqual([{ id: 1, favorite: true }, { id: 1, favorite: false }]);
});

test("editing exposes draft controls, cancel and Escape discard the draft", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: "original result", requestId: 901 });

  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  const editor = page.getByRole("textbox", { name: "Редактировать перевод" });
  await expect(editor).toHaveValue("original result");
  await editor.fill("draft text");
  await page.waitForTimeout(800);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-popup-edit-qa.png") });
  const footerGeometry = await page.locator(".popup-footer").evaluate((footer) => {
    const card = footer.closest(".pop").getBoundingClientRect();
    return [...footer.querySelectorAll("button")].every((button) => {
      const r = button.getBoundingClientRect();
      return r.left >= card.left && r.right <= card.right && r.top >= card.top && r.bottom <= card.bottom
        && r.left >= 0 && r.right <= window.innerWidth && r.top >= 0 && r.bottom <= window.innerHeight;
    });
  });
  expect(footerGeometry).toBe(true);
  await page.getByRole("button", { name: "Отменить" }).click();
  await expect(page.getByText("original result")).toBeVisible();
  await expect(page.getByText("draft text")).toHaveCount(0);

  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await editor.fill("draft text");
  await editor.press("Escape");
  await expect(page.getByText("original result")).toBeVisible();
  await expect(page.getByText("draft text")).toHaveCount(0);
});

test("draft is used exactly by Copy, Speak, and Replace before Done", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "captured source", translated: "server result", requestId: 902 });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  const editor = page.getByRole("textbox", { name: "Редактировать перевод" });
  await editor.fill("draft exact\nsecond line");

  await page.getByRole("button", { name: "Копировать" }).click();
  await page.getByRole("button", { name: "Озвучить" }).click();
  await page.getByRole("button", { name: "Заменить" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.copyCalls)).toEqual(["draft exact\nsecond line"]);
  await expect.poll(() => page.evaluate(() => window.__popupMock.speakCalls)).toEqual([
    { text: "draft exact\nsecond line", lang: "ru" },
  ]);
  await expect.poll(() => page.evaluate(() => window.__popupMock.replaceCalls)).toEqual([{
    requestId: 902, sourceText: "captured source", translatedText: "draft exact\nsecond line",
  }]);
});

test("Done persists an edited translation before saving its favorite state", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: "old", requestId: 903 });
  await page.evaluate(() => { window.__popupMock.updateTranslationPlan = [{ result: false }]; });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await page.getByRole("textbox", { name: "Редактировать перевод" }).fill("new saved text");
  await page.getByRole("button", { name: "Готово" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.updateTranslationCalls)).toEqual([{
    historyId: 41, sourceText: "source", expectedText: "old", text: "new saved text",
  }]);
  const favorite = page.getByRole("button", { name: "В избранное" });
  await expect(favorite).toBeEnabled();
  await favorite.click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.favoriteCalls)).toEqual([{ id: 41, favorite: true }]);
});

test("failed Done keeps the draft visible and does not lose the editing context", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: "old", requestId: 904 });
  await page.evaluate(() => { window.__popupMock.updateTranslationPlan = [{ fail: true, message: "save failed" }]; });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  const editor = page.getByRole("textbox", { name: "Редактировать перевод" });
  await editor.fill("draft retained");
  await page.getByRole("button", { name: "Готово" }).click();
  await expect(page.getByRole("alert")).toContainText("save failed");
  await expect(editor).toHaveValue("draft retained");
});

test("engine picker sends an explicit engine or automatic selection for the same source", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "same source", translated: "first result", requestId: 905 });

  const picker = page.getByRole("button", { name: "Выбрать движок перевода" });
  await picker.click();
  const menu = page.getByRole("menu", { name: "Движок перевода" });
  await expect(menu.getByRole("menuitemradio", { name: "Автоматически" })).toBeVisible();
  await expect(menu.getByRole("menuitemradio", { name: "Google" })).toBeVisible();
  await expect(menu.getByRole("menuitemradio", { name: "Bing" })).toBeVisible();
  await expect(menu.getByRole("menuitemradio", { name: "MyMemory" })).toBeVisible();
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-popup-engine-menu-qa.png") });
  await menu.getByRole("menuitemradio", { name: "Bing" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls)).toContainEqual({
    text: "same source", target: "ru", engine: "bing",
  });

  await picker.click();
  await menu.getByRole("menuitemradio", { name: "Автоматически" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls)).toContainEqual({
    text: "same source", target: "ru", engine: null,
  });
  await page.getByRole("button", { name: "Открыть в окне" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.openMainCalls)).toEqual(["same source"]);
});

test("long editor keeps Done, Cancel, and footer actions reachable", async ({ page }) => {
  await installTauriMock(page);
  await page.setViewportSize({ width: 600, height: 360 });
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: longText, requestId: 906 });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  const editor = page.getByRole("textbox", { name: "Редактировать перевод" });
  await editor.fill(Array.from({ length: 40 }, (_, i) => `edited line ${i + 1}`).join("\n"));
  await expect(page.getByRole("button", { name: "Готово" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Отменить" })).toBeVisible();
  const scroller = page.locator("[data-popup-scroll]");
  await scroller.evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await page.waitForTimeout(800);
  await page.screenshot({ path: path.join(os.tmpdir(), "utranslate-popup-long-editor-qa.png") });
  await expect(page.getByRole("button", { name: "Заменить" })).toBeInViewport();
  await expect(page.getByRole("button", { name: "Копировать" })).toBeInViewport();
  const footerGeometry = await page.locator(".popup-footer").evaluate((footer) => {
    const card = footer.closest(".pop").getBoundingClientRect();
    const buttons = [...footer.querySelectorAll("button")].map((button) => {
      const r = button.getBoundingClientRect();
      return r.left >= card.left && r.right <= card.right && r.top >= card.top && r.bottom <= card.bottom
        && r.left >= 0 && r.right <= window.innerWidth && r.top >= 0 && r.bottom <= window.innerHeight;
    });
    return buttons;
  });
  expect(footerGeometry.every(Boolean)).toBe(true);
});

test("editor accepts a long valid translation without truncating the saved text", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: "old", requestId: 907 });
  const longDraft = "x".repeat(5201);
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await page.getByRole("textbox", { name: "Редактировать перевод" }).fill(longDraft);
  await page.getByRole("button", { name: "Готово" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.updateTranslationCalls)).toHaveLength(1);
  await expect.poll(() => page.evaluate(() => window.__popupMock.updateTranslationCalls[0].text.length)).toBe(5201);
});

test("a new source session resets the previous manual engine choice", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "first source", translated: "first", requestId: 908 });
  await page.getByRole("button", { name: "Выбрать движок перевода" }).click();
  await page.getByRole("menu", { name: "Движок перевода" }).getByRole("menuitemradio", { name: "Bing" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls.at(-1).engine)).toBe("bing");

  await page.evaluate(() => window.__popupMock.emit("popup:show", {
    text: "", target: "ru", detected: null, clipboardReplaced: false, requestId: 909, canReplace: false,
  }));
  const input = page.getByPlaceholder("Введите текст для перевода…");
  await input.fill("second source");
  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls.at(-1))).toEqual({
    text: "second source", target: null, engine: null,
  });
  await page.waitForTimeout(350);
  await page.getByRole("button", { name: "Выбрать движок перевода" }).click();
  await page.getByRole("menu", { name: "Движок перевода" }).getByRole("menuitemradio", { name: "Автоматически" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls.at(-1))).toEqual({
    text: "second source", target: "ru", engine: null,
  });
});

test("editing the same input session resets a previous manual engine to automatic", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await page.waitForFunction(() => Boolean(window.__utDemo));
  await page.evaluate(() => window.__popupMock.emit("popup:show", {
    text: "", target: "ru", detected: null, clipboardReplaced: false, requestId: 917, canReplace: false,
  }));
  const input = page.getByPlaceholder("Введите текст для перевода…");
  await expect(input).toBeVisible();
  await input.fill("first");
  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls.at(-1))).toEqual({ text: "first", target: null, engine: null });
  await page.getByRole("button", { name: "Выбрать движок перевода" }).click();
  await page.getByRole("menu", { name: "Движок перевода" }).getByRole("menuitemradio", { name: "Bing" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls.at(-1).engine)).toBe("bing");
  await input.fill("second");
  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__popupMock.translationCalls.at(-1))).toEqual({ text: "second", target: null, engine: null });
});

test("blank draft disables every translation action", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: "result", requestId: 910 });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await page.getByRole("textbox", { name: "Редактировать перевод" }).fill("   \n  ");
  for (const name of ["Заменить", "Копировать", "Озвучить", "В избранное"]) {
    await expect(page.getByRole("button", { name })).toBeDisabled();
  }
});

test("dirty favorite saves the exact draft before toggling favorite", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "source", translated: "result", requestId: 911 });
  await page.evaluate(() => { window.__popupMock.updateTranslationPlan = [{ result: false }]; });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await page.getByRole("textbox", { name: "Редактировать перевод" }).fill("favorite draft");
  await page.getByRole("button", { name: "В избранное" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.updateTranslationCalls)).toEqual([{
    historyId: 41, sourceText: "source", expectedText: "result", text: "favorite draft",
  }]);
  await expect.poll(() => page.evaluate(() => window.__popupMock.favoriteCalls)).toEqual([{ id: 41, favorite: true }]);
});

test("late manual engine response cannot replace a newer popup session", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "old source", translated: "old result", requestId: 912 });
  await page.evaluate(() => { window.__popupMock.translationPlan = [{ delay: 400 }]; });
  await page.getByRole("button", { name: "Выбрать движок перевода" }).click();
  await page.getByRole("menu", { name: "Движок перевода" }).getByRole("menuitemradio", { name: "Bing" }).click();
  await page.evaluate(() => window.__popupMock.emit("popup:show", {
    text: "new source", target: "ru", detected: "en", clipboardReplaced: false, requestId: 913, canReplace: true,
  }));
  await page.evaluate(() => window.__popupMock.emit("popup:result", {
    text: "new result", detected: "en", target: "ru", engine: "google", alternatives: [], fallbackFrom: null,
    historyId: 99, wordMode: false, isFavorite: false, requestId: 913,
  }));
  await page.waitForTimeout(500);
  await expect(page.getByText("new result")).toBeVisible();
  await expect(page.getByText("old result")).toHaveCount(0);
});

test("manual engine failure preserves the saved edit and replacement context", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "captured", translated: "old", requestId: 914 });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await page.getByRole("textbox", { name: "Редактировать перевод" }).fill("edited");
  await page.getByRole("button", { name: "Готово" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.updateTranslationCalls)).toHaveLength(1);
  await page.evaluate(() => { window.__popupMock.translationPlan = [{ fail: true, message: "provider down" }]; });
  await page.getByRole("button", { name: "Выбрать движок перевода" }).click();
  await page.getByRole("menu", { name: "Движок перевода" }).getByRole("menuitemradio", { name: "Bing" }).click();
  await expect(page.getByText("edited")).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("provider down");
  await page.getByRole("button", { name: "Заменить" }).click();
  await expect.poll(() => page.evaluate(() => window.__popupMock.replaceCalls.at(-1))).toEqual({
    requestId: 914, sourceText: "captured", translatedText: "edited",
  });
});

test("late CAS save cannot overwrite a newer popup result", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("http://127.0.0.1:1420/?w=popup");
  await showResult(page, { source: "old source", translated: "old", requestId: 915 });
  await page.evaluate(() => { window.__popupMock.updateTranslationPlan = [{ delay: 400, result: true }]; });
  await page.getByRole("button", { name: "Редактировать перевод" }).click();
  await page.getByRole("textbox", { name: "Редактировать перевод" }).fill("late edit");
  await page.getByRole("button", { name: "Готово" }).click();
  await page.evaluate(() => window.__popupMock.emit("popup:show", {
    text: "new source", target: "ru", detected: "en", clipboardReplaced: false, requestId: 916, canReplace: true,
  }));
  await page.evaluate(() => window.__popupMock.emit("popup:result", {
    text: "new result", detected: "en", target: "ru", engine: "google", alternatives: [], fallbackFrom: null,
    historyId: 916, wordMode: false, isFavorite: false, requestId: 916,
  }));
  await page.waitForTimeout(500);
  await expect(page.getByText("new result")).toBeVisible();
  await expect(page.getByRole("button", { name: "В избранное" })).toHaveAttribute("aria-pressed", "false");
});
