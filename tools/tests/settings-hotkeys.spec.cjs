const { createRequire } = require("node:module");
const playwrightRequire = createRequire(require.main.filename);
const { test, expect } = playwrightRequire("playwright/test");

const settings = {
  hotkeyPopup: "Ctrl+Alt+T",
  hotkeyReplace: "Ctrl+Alt+R",
  hotkeyWindow: "Ctrl+Alt+U",
  primaryLang: "ru",
  secondaryLang: "en",
  engines: ["google"],
  theme: "dark",
  uiLang: "ru",
  historyEnabled: true,
  showOriginal: false,
  fontSize: 16,
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
    const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const state = {
      settings: { ...initialSettings },
      calls: [],
      completions: [],
      suspendPlan: [],
      settingsDelay: 0,
      suspended: false,
      suspendGatePending: false,
      releaseSuspendGate: null,
    };
    window.__settingsMock = state;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
      transformCallback(callback) {
        const id = ++callbackId;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id) { callbacks.delete(id); },
      async invoke(cmd, args = {}) {
        if (cmd === "settings_get") return { ...state.settings };
        if (cmd === "settings_set") {
          state.calls.push({ cmd, settings: { ...args.settings } });
          await wait(state.settingsDelay);
          state.settings = { ...args.settings };
          state.completions.push("settings_set");
          return [
            { field: "hotkeyPopup", error: null },
            { field: "hotkeyReplace", error: null },
            { field: "hotkeyWindow", error: null },
          ];
        }
        if (cmd === "hotkeys_suspend") {
          const suspended = args.suspended;
          state.calls.push({ cmd, suspended });
          const plan = state.suspendPlan.shift() || {};
          if (plan.gate) {
            await new Promise((resolve) => {
              state.suspendGatePending = true;
              state.releaseSuspendGate = resolve;
            });
            state.suspendGatePending = false;
            state.releaseSuspendGate = null;
          }
          await wait(plan.delay || 0);
          if (plan.fail) throw new Error("hotkey suspension failed");
          state.suspended = suspended;
          state.completions.push(`suspend:${suspended}`);
          return null;
        }
        if (cmd === "hotkeys_status") return [];
        if (cmd === "autostart_get") return false;
        if (cmd === "update_available") return null;
        if (cmd === "plugin:app|version") return "test";
        if (cmd === "plugin:event|listen") return ++eventId;
        if (cmd === "plugin:event|unlisten") return null;
        return null;
      },
    };
  }, settings);
  await page.goto("http://127.0.0.1:1420/");
  await page.getByRole("button", { name: "Настройки", exact: true }).click();
  await expect(page.getByText("Хоткеи", { exact: true })).toBeVisible();
});

test.afterEach(async ({ page }) => {
  expect(pageErrors.get(page)).toEqual([]);
});

function hotkey(page, label) {
  return page.getByRole("button", { name: new RegExp(`^${label}:`) });
}

test("saved hotkey resumes only after settings_set completes", async ({ page }) => {
  await page.evaluate(() => {
    window.__settingsMock.suspendPlan = [{ delay: 120 }];
    window.__settingsMock.settingsDelay = 120;
    window.__settingsMock.calls = [];
    window.__settingsMock.completions = [];
  });

  const popup = hotkey(page, "Перевести в попап");
  await popup.click();
  await popup.press("Control+Shift+KeyK");
  await page.waitForTimeout(30);
  expect(await page.evaluate(() => window.__settingsMock.calls
    .filter((call) => call.cmd === "settings_set"))).toEqual([]);

  await expect(popup).toHaveClass(/\brec\b/);
  await popup.press("Control+Shift+KeyK");
  await expect.poll(() => page.evaluate(() => window.__settingsMock.calls
    .some((call) => call.cmd === "settings_set"))).toBe(true);
  await page.getByRole("button", { name: "Перевод", exact: true }).click();
  await expect.poll(() => page.evaluate(() => window.__settingsMock.completions)).toEqual([
    "suspend:true",
    "settings_set",
    "suspend:false",
  ]);
});

test("repeat click does not acquire suspension twice", async ({ page }) => {
  const popup = hotkey(page, "Перевести в попап");
  await page.evaluate(() => {
    window.__settingsMock.suspendPlan = [{ delay: 100 }];
    window.__settingsMock.calls = [];
  });
  await popup.click();
  await popup.click();
  await expect.poll(() => page.evaluate(() => window.__settingsMock.calls
    .filter((call) => call.cmd === "hotkeys_suspend")
    .map((call) => call.suspended))).toEqual([true]);

  await expect(popup).toHaveClass(/\brec\b/);
  await popup.press("Escape");
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(false);
});

test("switching fields cannot let a stale release unsuspend the new recorder", async ({ page }) => {
  const popup = hotkey(page, "Перевести в попап");
  const replace = hotkey(page, "Заменить выделенное");
  await popup.click();
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(true);
  await page.evaluate(() => {
    window.__settingsMock.calls = [];
    window.__settingsMock.completions = [];
    window.__settingsMock.suspendPlan = [{ delay: 220 }, { delay: 0 }];
  });

  await replace.click();
  await expect(replace).toHaveClass(/\brec\b/);
  await page.waitForTimeout(280);
  expect(await page.evaluate(() => window.__settingsMock.suspended)).toBe(true);

  await replace.press("Escape");
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(false);
});

test("failed cancellation resume stays recoverable and unmount releases", async ({ page }) => {
  const popup = hotkey(page, "Перевести в попап");
  await popup.click();
  await expect(popup).toHaveClass(/\brec\b/);
  await page.evaluate(() => { window.__settingsMock.suspendPlan = [{ fail: true }]; });
  await popup.press("Escape");
  await expect(page.getByText("Не удалось включить хоткеи", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.__settingsMock.suspended)).toBe(true);

  await popup.click();
  await expect(popup).toHaveClass(/\brec\b/);
  await popup.press("Escape");
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(false);

  await popup.click();
  await expect(popup).toHaveClass(/\brec\b/);
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(true);
  await page.getByRole("button", { name: "Перевод", exact: true }).click();
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(false);
});

test("pending unmount cleanup cannot resume over a recorder after remount", async ({ page }) => {
  const popup = hotkey(page, "Перевести в попап");
  await page.evaluate(() => {
    window.__settingsMock.suspendPlan = [{ gate: true }, { delay: 0 }];
  });
  await popup.click();
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspendGatePending)).toBe(true);
  await page.getByRole("button", { name: "Перевод", exact: true }).click();
  await page.getByRole("button", { name: "Настройки", exact: true }).click();
  const remounted = hotkey(page, "Заменить выделенное");
  await remounted.click();
  expect(await page.evaluate(() => window.__settingsMock.suspendGatePending)).toBe(true);

  await page.evaluate(() => window.__settingsMock.releaseSuspendGate());
  await expect(remounted).toHaveClass(/\brec\b/);
  expect(await page.evaluate(() => window.__settingsMock.suspended)).toBe(true);
  expect(await page.evaluate(() => window.__settingsMock.completions)).toEqual(["suspend:true"]);

  await remounted.press("Escape");
  await expect.poll(() => page.evaluate(() => window.__settingsMock.suspended)).toBe(false);
});
