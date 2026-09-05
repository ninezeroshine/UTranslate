import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, lstat, mkdir, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const toolsDir = dirname(fileURLToPath(import.meta.url));
const repoDir = resolve(toolsDir, "..");
const spec = JSON.parse(await readFile(join(toolsDir, "ocr", "manifest.json"), "utf8"));
const cacheDir = join(toolsDir, "ocr", ".cache");
const resourceDir = join(repoDir, "app", "src-tauri", "resources", "ocr");
const modelDir = join(resourceDir, "models");
const runtimeDir = join(resourceDir, "runtime");
const licenseDir = join(resourceDir, "licenses");

if (process.platform !== "win32") {
  throw new Error("OCR resources are prepared only for the Windows target");
}

function sameWindowsPath(left, right) {
  return resolve(left).toLowerCase() === resolve(right).toLowerCase();
}

function isStrictChild(root, target) {
  const rel = relative(resolve(root), resolve(target));
  return rel !== "" && rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

async function verifyPlainDirectory(path) {
  const info = await lstat(path);
  if (!info.isDirectory() || info.isSymbolicLink()) {
    throw new Error(`Refusing unsafe generated path (not a plain directory): ${path}`);
  }
  const actual = await realpath(path);
  if (!sameWindowsPath(actual, path)) {
    throw new Error(`Refusing generated path redirected by a junction: ${path} -> ${actual}`);
  }
}

async function recreateGeneratedChild(knownRoot, target) {
  const absoluteRoot = resolve(knownRoot);
  const absoluteTarget = resolve(target);
  if (!isStrictChild(absoluteRoot, absoluteTarget)) {
    throw new Error(`Refusing recursive operation outside generated root: ${absoluteTarget}`);
  }
  await mkdir(absoluteRoot, { recursive: true });
  await verifyPlainDirectory(absoluteRoot);

  let current = absoluteRoot;
  for (const component of relative(absoluteRoot, absoluteTarget).split(sep)) {
    current = join(current, component);
    if (existsSync(current)) {
      const info = await lstat(current);
      if (info.isSymbolicLink()) {
        throw new Error(`Refusing generated path containing a junction: ${current}`);
      }
      const actual = await realpath(current);
      if (!sameWindowsPath(actual, current) || !isStrictChild(absoluteRoot, actual)) {
        throw new Error(`Refusing generated path outside known root: ${current} -> ${actual}`);
      }
    }
  }

  await rm(absoluteTarget, { recursive: true, force: true });
  await mkdir(absoluteTarget, { recursive: true });
}

await mkdir(cacheDir, { recursive: true });
await Promise.all([
  recreateGeneratedChild(resourceDir, modelDir),
  recreateGeneratedChild(resourceDir, runtimeDir),
  recreateGeneratedChild(resourceDir, licenseDir),
]);

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

async function verify(path, expectedHash, expectedBytes) {
  if (!existsSync(path)) return false;
  const info = await stat(path);
  if (expectedBytes !== undefined && info.size !== expectedBytes) return false;
  return sha256(await readFile(path)) === expectedHash.toLowerCase();
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
  if (!(await verify(target, hash, bytes))) throw new Error(`Copied file failed verification: ${target}`);
}

function run(file, args, options = {}) {
  execFileSync(file, args, { stdio: "pipe", windowsHide: true, ...options });
}

const detTar = await downloadPinned("ppocrv5-det.tar", spec.artifacts.detArchive);
const recTar = await downloadPinned("ppocrv5-eslav-rec.tar", spec.artifacts.recArchive);
const dict = await downloadPinned("ppocrv5-eslav-dict.txt", spec.artifacts.dict);
const ortZip = await downloadPinned("onnxruntime-win-x64-1.23.2.zip", spec.artifacts.ortArchive);

const extracted = join(cacheDir, "extracted");
await recreateGeneratedChild(cacheDir, extracted);
run("tar.exe", ["-xf", detTar, "-C", extracted]);
run("tar.exe", ["-xf", recTar, "-C", extracted]);
run("pwsh.exe", [
  "-NoProfile", "-NonInteractive", "-Command",
  "Expand-Archive -LiteralPath $env:OCR_ZIP -DestinationPath $env:OCR_DST -Force",
], { env: { ...process.env, OCR_ZIP: ortZip, OCR_DST: extracted } });

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
await copyVerified(dict, join(modelDir, "ppocrv5_eslav_dict.txt"), spec.artifacts.dict.sha256, spec.artifacts.dict.bytes);

const ortLib = join(extracted, spec.artifacts.ortArchive.directory, "lib");
await copyVerified(join(ortLib, "onnxruntime.dll"), join(runtimeDir, "onnxruntime.dll"), spec.artifacts.ortArchive.dllSha256, 14186016);
await copyVerified(join(ortLib, "onnxruntime_providers_shared.dll"), join(runtimeDir, "onnxruntime_providers_shared.dll"), spec.artifacts.ortArchive.providersSha256, 22088);

const vswhere = "C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe";
if (!existsSync(vswhere)) throw new Error("Visual Studio Build Tools with VC Redistributable files are required");
const installation = execFileSync(vswhere, ["-latest", "-products", "*", "-property", "installationPath"], {
  encoding: "utf8", windowsHide: true,
}).trim();
if (!installation) throw new Error("No Visual Studio installation with redistributable files was found");

const redistRoot = join(installation, "VC", "Redist", "MSVC");
const redistPath = execFileSync("pwsh.exe", [
  "-NoProfile", "-NonInteractive", "-Command",
  "$root=$env:VC_ROOT; $d=Get-ChildItem -LiteralPath $root -Directory | Where-Object { $_.Name -match '^14\\.\\d+\\.\\d+$' } | Sort-Object { [version]$_.Name } -Descending | ForEach-Object { Join-Path $_.FullName 'x64/Microsoft.VC143.CRT' } | Where-Object { Test-Path $_ } | Select-Object -First 1; if(-not $d){exit 3}; $d",
], { encoding: "utf8", windowsHide: true, env: { ...process.env, VC_ROOT: redistRoot } }).trim();

const vcDlls = ["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "msvcp140_1.dll"];
const vcManifest = [];
for (const name of vcDlls) {
  const source = join(redistPath, name);
  if (!existsSync(source)) throw new Error(`Required Visual C++ runtime file is missing: ${source}`);
  const details = JSON.parse(execFileSync("pwsh.exe", [
    "-NoProfile", "-NonInteractive", "-Command",
    "$p=$env:VC_DLL; $s=Get-AuthenticodeSignature -LiteralPath $p; $i=Get-Item -LiteralPath $p; [pscustomobject]@{status=$s.Status.ToString(); signer=$s.SignerCertificate.Subject; version=$i.VersionInfo.FileVersion} | ConvertTo-Json -Compress",
  ], { encoding: "utf8", windowsHide: true, env: { ...process.env, VC_DLL: source } }));
  if (details.status !== "Valid" || !details.signer.includes("Microsoft Corporation")) {
    throw new Error(`Authenticode validation failed for ${source}: ${details.status} ${details.signer}`);
  }
  if (Number(details.version.split(".")[0]) < 14) throw new Error(`Unsupported VC runtime version ${details.version}`);
  const data = await readFile(source);
  await copyFile(source, join(runtimeDir, name));
  vcManifest.push({ name, bytes: data.length, sha256: sha256(data), version: details.version, signer: details.signer });
}

for (const name of ["PaddleOCR-LICENSE.txt", "ONNXRuntime-LICENSE.txt", "ONNXRuntime-ThirdPartyNotices.txt", "APACHE-2.0.txt", "VC-RUNTIME-NOTICE.md"]) {
  await copyFile(join(toolsDir, "ocr", "licenses", name), join(licenseDir, name));
}

const generated = {
  schema: 1,
  sourceManifest: spec,
  visualCppRuntime: {
    note: "App-local retail DLLs copied from the licensed Visual Studio Build Tools REDIST directory; bytes depend on the installed toolset and are recorded here rather than claimed as pinned downloads.",
    sourceKind: "Visual Studio Build Tools VC143 x64 REDIST",
    toolsetVersion: basename(dirname(dirname(redistPath))),
    files: vcManifest,
  },
};
await writeFile(join(resourceDir, "manifest.generated.json"), `${JSON.stringify(generated, null, 2)}\n`);
console.log(`OCR resources prepared: ORT ${spec.versions.onnxRuntime}, PP-OCRv5 det+eslav rec, VC ${vcManifest[0].version}`);
