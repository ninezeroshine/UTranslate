import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const started = Date.now();
const toolsDir = dirname(fileURLToPath(import.meta.url));
const repoDir = resolve(toolsDir, "..");
const spec = JSON.parse(await readFile(join(toolsDir, "ocr", "manifest.json"), "utf8"));
const cacheDir = join(toolsDir, "ocr", ".cache");
const resourceDir = join(repoDir, "app", "src-tauri", "resources", "ocr");
const modelDir = join(resourceDir, "models");
const runtimeDir = join(resourceDir, "runtime");
const licenseDir = join(resourceDir, "licenses");
const generatedPath = join(resourceDir, "manifest.generated.json");

if (process.platform !== "win32") {
  throw new Error("OCR resources are prepared only for the Windows target");
}

// bsdtar из System32 распаковывает и .tar, и .zip. Явный путь, потому что в PATH
// разработчика может стоять GNU tar из MSYS/Git, а он ломается о "C:" в аргументе.
const tarExe = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
const licenseNames = [
  "PaddleOCR-LICENSE.txt",
  "ONNXRuntime-LICENSE.txt",
  "ONNXRuntime-ThirdPartyNotices.txt",
  "APACHE-2.0.txt",
];

// Пять файлов, ради которых существует скрипт. Хэши — пины из manifest.json.
const targets = [
  {
    path: join(modelDir, "ppocrv5_mobile_det.onnx"),
    sha256: spec.artifacts.detArchive.modelSha256,
    bytes: spec.artifacts.detArchive.modelBytes,
  },
  {
    path: join(modelDir, "ppocrv5_eslav_rec.onnx"),
    sha256: spec.artifacts.recArchive.modelSha256,
    bytes: spec.artifacts.recArchive.modelBytes,
  },
  {
    path: join(modelDir, "ppocrv5_eslav_dict.txt"),
    sha256: spec.artifacts.dict.sha256,
    bytes: spec.artifacts.dict.bytes,
  },
  {
    path: join(runtimeDir, "onnxruntime.dll"),
    sha256: spec.artifacts.ortArchive.dllSha256,
    bytes: 14186016,
  },
  {
    path: join(runtimeDir, "onnxruntime_providers_shared.dll"),
    sha256: spec.artifacts.ortArchive.providersSha256,
    bytes: 22088,
  },
];

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

async function verify(path, expectedHash, expectedBytes) {
  if (!existsSync(path)) return false;
  const info = await stat(path);
  if (expectedBytes !== undefined && info.size !== expectedBytes) return false;
  return sha256(await readFile(path)) === expectedHash.toLowerCase();
}

/// rm -rf + mkdir с единственной защитой: путь обязан лежать строго внутри известного корня.
async function resetDir(knownRoot, target) {
  const root = resolve(knownRoot);
  const absolute = resolve(target);
  const rel = relative(root, absolute);
  if (rel === "" || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw new Error(`Refusing recursive operation outside ${root}: ${absolute}`);
  }
  await rm(absolute, { recursive: true, force: true });
  await mkdir(absolute, { recursive: true });
}

// Быстрый путь: скрипт зовут из beforeDevCommand и beforeBuildCommand при каждом запуске.
async function everythingIsInPlace() {
  if (!existsSync(generatedPath)) return false;
  if (licenseNames.some((name) => !existsSync(join(licenseDir, name)))) return false;
  for (const target of targets) {
    if (!(await verify(target.path, target.sha256, target.bytes))) return false;
  }
  // Лишний файл в runtime/ (например, CRT из прошлых сборок) чинится полным путём:
  // LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR предпочтёт его системному.
  const runtimeFiles = await readdir(runtimeDir);
  return runtimeFiles.length === 2;
}

if (await everythingIsInPlace()) {
  console.log(
    `OCR resources already prepared: ORT ${spec.versions.onnxRuntime}, PP-OCRv5 det+eslav rec (checked in ${Date.now() - started} ms)`,
  );
  process.exit(0);
}

async function downloadPinned(name, artifact) {
  const target = join(cacheDir, name);
  if (await verify(target, artifact.sha256, artifact.bytes)) return target;
  const response = await fetch(artifact.url, { redirect: "follow" });
  if (!response.ok) throw new Error(`Download failed (${response.status}) for ${artifact.url}`);
  const data = Buffer.from(await response.arrayBuffer());
  const actual = sha256(data);
  if (data.length !== artifact.bytes || actual !== artifact.sha256) {
    throw new Error(`Integrity check failed for ${name}: bytes=${data.length}, sha256=${actual}`);
  }
  await writeFile(`${target}.part`, data);
  await rm(target, { force: true });
  await writeFile(target, data);
  await rm(`${target}.part`, { force: true });
  return target;
}

async function copyVerified(source, target, hash, bytes) {
  if (!(await verify(source, hash, bytes))) throw new Error(`Integrity check failed for ${source}`);
  await copyFile(source, target);
  if (!(await verify(target, hash, bytes))) {
    throw new Error(`Copied file failed verification: ${target}`);
  }
}

function untar(archive, destination, members = []) {
  execFileSync(tarExe, ["-xf", archive, "-C", destination, ...members], {
    stdio: "pipe",
    windowsHide: true,
  });
}

await mkdir(cacheDir, { recursive: true });
await resetDir(resourceDir, modelDir);
await resetDir(resourceDir, runtimeDir);
await resetDir(resourceDir, licenseDir);

const detTar = await downloadPinned("ppocrv5-det.tar", spec.artifacts.detArchive);
const recTar = await downloadPinned("ppocrv5-eslav-rec.tar", spec.artifacts.recArchive);
const dict = await downloadPinned("ppocrv5-eslav-dict.txt", spec.artifacts.dict);
const ortZip = await downloadPinned("onnxruntime-win-x64-1.23.2.zip", spec.artifacts.ortArchive);

const extracted = join(cacheDir, "extracted");
await resetDir(cacheDir, extracted);
untar(detTar, extracted);
untar(recTar, extracted);
const ortLibDir = `${spec.artifacts.ortArchive.directory}/lib`;
// Из zip достаём только две библиотеки: рядом лежат .pdb на 380 МБ, они не нужны.
untar(ortZip, extracted, [
  `${ortLibDir}/onnxruntime.dll`,
  `${ortLibDir}/onnxruntime_providers_shared.dll`,
]);

await copyVerified(
  join(extracted, spec.artifacts.detArchive.member),
  join(modelDir, "ppocrv5_mobile_det.onnx"),
  spec.artifacts.detArchive.modelSha256,
  spec.artifacts.detArchive.modelBytes,
);
await copyVerified(
  join(extracted, spec.artifacts.recArchive.member),
  join(modelDir, "ppocrv5_eslav_rec.onnx"),
  spec.artifacts.recArchive.modelSha256,
  spec.artifacts.recArchive.modelBytes,
);
await copyVerified(
  dict,
  join(modelDir, "ppocrv5_eslav_dict.txt"),
  spec.artifacts.dict.sha256,
  spec.artifacts.dict.bytes,
);

const ortLib = join(extracted, spec.artifacts.ortArchive.directory, "lib");
for (const target of targets.slice(3)) {
  await copyVerified(join(ortLib, basename(target.path)), target.path, target.sha256, target.bytes);
}

for (const name of licenseNames) {
  await copyFile(join(toolsDir, "ocr", "licenses", name), join(licenseDir, name));
}

await writeFile(
  generatedPath,
  `${JSON.stringify({ schema: 1, sourceManifest: spec }, null, 2)}\n`,
);
console.log(
  `OCR resources prepared: ORT ${spec.versions.onnxRuntime}, PP-OCRv5 det+eslav rec (${Date.now() - started} ms)`,
);
