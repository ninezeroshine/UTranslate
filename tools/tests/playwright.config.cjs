const path = require("node:path");

module.exports = {
  testDir: ".",
  testMatch: "*.spec.cjs",
  outputDir: path.join(require("node:os").tmpdir(), "utranslate-playwright"),
  use: { headless: true },
  webServer: {
    command: "pnpm dev --host 127.0.0.1",
    cwd: path.resolve(__dirname, "../../app"),
    url: "http://127.0.0.1:1420",
    reuseExistingServer: true,
    timeout: 120_000,
  },
};
