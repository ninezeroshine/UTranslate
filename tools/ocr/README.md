# UTranslate OCR assets

Run `pnpm ocr:prepare` from `app/` before Tauri development, Rust OCR tests, or packaging. The script downloads official Paddle model archives and the official ONNX Runtime 1.23.2 Windows x64 release, verifies pinned project SHA-256 values, extracts only the required runtime files, and writes generated resources to `app/src-tauri/resources/ocr/`. Frontend-only `pnpm dev`, `pnpm build`, and browser tests do not require OCR assets or Visual Studio REDIST files.

Model hashes are project pins computed after HTTPS download. Paddle does not publish checksums for these tarballs, so they are not described as vendor-verified hashes. The manifest links the official PaddlePaddle Hugging Face model cards, which declare Apache-2.0 for both selected weights. The Visual C++ retail runtime DLLs are copied app-locally from the licensed Visual Studio Build Tools REDIST directory. Their Microsoft Authenticode signatures and minimum major version are checked and their exact version/hash is recorded in `manifest.generated.json`; their bytes depend on the installed Build Tools version. The generated manifest records the REDIST kind and toolset version without embedding a developer-machine path or timestamp.

The installed resource layout is `resource_dir/ocr/{models,runtime,licenses}`. Recognition loads the absolute `ocr/runtime/onnxruntime.dll` path, preloads its app-local dependencies with safe Windows loader flags, disables ORT telemetry explicitly, selects CPU inference only, and performs no runtime downloads.

Synthetic benchmark sources and compact results live in `reference/` and `results/`. PNG fixtures are generated and not committed.

The preprocessing and decoding contracts were checked against PaddleOCR commit `b03f46425e8ff4442b268ce449e3eef758146cd4`, specifically `operators.py`, `db_postprocess.py`, `rec_postprocess.py`, `predict_rec.py`, and `predict_system.py`. The Rust geometry path is pinned to `ppocr-rs 0.7.3` at commit `bb287d4311c0d333da363b8f8ce944c16884aa9e`; its crate metadata declares Apache-2.0. Exact source and artifact versions are recorded in `manifest.json`.
