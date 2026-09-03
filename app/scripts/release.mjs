#!/usr/bin/env node
// Выпуск релиза: версия во все манифесты, коммит, тег, push.
// Дальше работает .github/workflows/release.yml — собирает установщик, подписывает и кладёт latest.json.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("Использование: pnpm release 0.2.0");
  process.exit(1);
}

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const rust = join(app, "src-tauri");
const run = (cmd, args, cwd = app) => execFileSync(cmd, args, { cwd, stdio: "inherit" });

if (execFileSync("git", ["status", "--porcelain"], { cwd: app, encoding: "utf8" }).trim()) {
  console.error("Рабочее дерево не чистое: сначала закоммитьте или отмените изменения.");
  process.exit(1);
}

function edit(path, re, next) {
  const file = join(app, path);
  const src = readFileSync(file, "utf8");
  if (!re.test(src)) {
    console.error(`Не нашёл версию в ${path}`);
    process.exit(1);
  }
  writeFileSync(file, src.replace(re, next));
}

const SEMVER = /"version": "\d+\.\d+\.\d+"/;
edit("package.json", SEMVER, `"version": "${version}"`);
edit("src-tauri/tauri.conf.json", SEMVER, `"version": "${version}"`);
// Только версия самого пакета: она стоит до [dependencies], где версии тоже есть.
edit("src-tauri/Cargo.toml", /^version = "\d+\.\d+\.\d+"$/m, `version = "${version}"`);

// Cargo.lock хранит версию пакета — без обновления сборка в CI упадёт на «lock is out of date».
run("cargo", ["update", "-p", "app", "--offline"], rust);
const lock = readFileSync(join(rust, "Cargo.lock"), "utf8");
if (!new RegExp(`name = "app"\\r?\\nversion = "${version.replace(/\./g, "\\.")}"`).test(lock)) {
  console.error("Cargo.lock не обновился — поправьте вручную и повторите.");
  process.exit(1);
}

run("git", ["commit", "-am", `release: v${version}`]);
run("git", ["tag", `v${version}`]);
run("git", ["push"]);
run("git", ["push", "--tags"]);
console.log(`Тег v${version} отправлен. Сборка идёт в GitHub Actions.`);
