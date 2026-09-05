//! CPU-only PP-OCRv5 screenshot OCR.
//!
//! Models and ONNX Runtime are prepared by `tools/prepare-ocr.mjs`. Recognition is
//! fully in-memory: this module never writes images or recognized text and never uses
//! the network. The mutable ONNX sessions are serialized behind one process-wide mutex.

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use image::{imageops, RgbImage};
use ndarray::Array4;
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use ppocr_rs::{
    base_net::BaseNet,
    db_net::DbNet,
    ocr_result::{Point, TextBox},
    ocr_utils::OcrUtils,
    scale_param::ScaleParam,
};

const MAX_INPUT_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_TILES: usize = 128;
const MAX_DETECTIONS: usize = 2_000;
const MAX_LINE_CHARS: usize = 8_192;
const MAX_TOTAL_CHARS: usize = 50_000;
// Keep ordinary Full HD selections in one detector run. Larger selections are tiled;
// cancellation is checked between tiles.
const TILE_WIDTH: u32 = 2_048;
const TILE_HEIGHT: u32 = 2_048;
const TILE_OVERLAP: u32 = 128;
const SYNTHETIC_BORDER: u32 = 8;
const SMALL_IMAGE_LONG_EDGE: u32 = 464;
const REC_HEIGHT: u32 = 48;
const REC_BASE_WIDTH: u32 = 320;
const REC_MAX_WIDTH: u32 = 3_200;
const REC_BATCH: usize = 6;
const MAX_CROP_BATCH_PIXELS: u64 = 16 * 1024 * 1024;
/// Сессии ORT и модели держат около 60 МБ. Пользователь снимает область раз в несколько
/// минут, поэтому после простоя движок выгружается и грузится заново на следующем запросе.
const IDLE_UNLOAD_MS: u64 = 3 * 60 * 1000;
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(30);

static ENGINE: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();
static ORT_READY: OnceLock<Result<(), String>> = OnceLock::new();
static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static LAST_USED_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// Решение сторожевого потока: движок простаивает дольше порога, пора освободить память.
fn should_unload(now_ms: u64, last_used_ms: u64) -> bool {
    now_ms.saturating_sub(last_used_ms) > IDLE_UNLOAD_MS
}

/// Один сторожевой поток на процесс, запускается при первой загрузке движка.
/// Мьютекс берётся только через `try_lock`: занятый мьютекс означает идущее распознавание.
fn spawn_idle_watchdog() {
    static WATCHDOG: OnceLock<()> = OnceLock::new();
    WATCHDOG.get_or_init(|| {
        let _ = thread::Builder::new()
            .name("utranslate-ocr-idle".to_string())
            .spawn(|| loop {
                thread::sleep(IDLE_CHECK_INTERVAL);
                let Some(mutex) = ENGINE.get() else { continue };
                if !should_unload(now_ms(), LAST_USED_MS.load(AtomicOrdering::Relaxed)) {
                    continue;
                }
                if let Ok(mut guard) = mutex.try_lock() {
                    // Метку проверяем ещё раз под замком: запрос мог начаться прямо сейчас.
                    if should_unload(now_ms(), LAST_USED_MS.load(AtomicOrdering::Relaxed)) {
                        *guard = None;
                    }
                }
            });
    });
}

struct Engine {
    detector: DbNet,
    recognizer: Session,
    recognizer_input: String,
    keys: Vec<String>,
    resource_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
struct LocatedBlock {
    text: String,
    score: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl LocatedBlock {
    fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    fn height(&self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }

    fn center_y(&self) -> f32 {
        (self.top + self.bottom) * 0.5
    }
}

/// Recognize UTF-8 text in an RGBA screenshot crop.
///
/// `resource_dir` is Tauri's absolute resource directory. The function accepts at most
/// 64 MiPixels, validates all size arithmetic, uses CPU sessions only, and returns lines
/// in top-to-bottom/left-to-right order.
#[allow(dead_code)]
pub fn recognize(
    resource_dir: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<String, String> {
    recognize_cancellable(resource_dir, width, height, rgba, || true)
}

/// Variant used by the screenshot request pipeline to abandon stale work between tiles.
/// A native ORT inference already in progress finishes normally; cancellation is observed
/// while waiting for the shared engine and before every subsequent tile.
pub fn recognize_cancellable<F>(
    resource_dir: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
    is_current: F,
) -> Result<String, String>
where
    F: Fn() -> bool,
{
    let expected = checked_rgba_len(width, height)?;
    if rgba.len() != expected {
        return Err(format!(
            "Длина RGBA-буфера не совпадает с размером изображения: ожидалось {expected}, получено {}",
            rgba.len()
        ));
    }

    let root = resolve_resource_root(resource_dir)?;
    let mutex = ENGINE.get_or_init(|| Mutex::new(None));
    if !is_current() {
        return Err("OCR-запрос отменён".to_string());
    }
    // Устаревший запрос отваливается сразу, как только получит замок: ждать в цикле незачем.
    let mut guard = mutex
        .lock()
        .map_err(|_| "OCR-сессия недоступна после внутренней ошибки".to_string())?;
    if !is_current() {
        return Err("OCR-запрос отменён".to_string());
    }
    LAST_USED_MS.store(now_ms(), AtomicOrdering::Relaxed);
    if guard
        .as_ref()
        .is_none_or(|engine| engine.resource_root != root)
    {
        *guard = Some(Engine::load(root.clone())?);
        spawn_idle_watchdog();
    }

    let image = rgba_to_bgr_image(width, height, rgba)?;
    let result = guard
        .as_mut()
        .expect("engine initialized above")
        .recognize_image(&image, &is_current);
    // Отметка после работы: сторожевой поток не должен считать простоем долгое распознавание.
    LAST_USED_MS.store(now_ms(), AtomicOrdering::Relaxed);
    result
}

impl Engine {
    fn load(resource_root: PathBuf) -> Result<Self, String> {
        let runtime_dir = resource_root.join("runtime");
        let runtime = runtime_dir.join("onnxruntime.dll");
        let det = resource_root.join("models/ppocrv5_mobile_det.onnx");
        let rec = resource_root.join("models/ppocrv5_eslav_rec.onnx");
        let dict = resource_root.join("models/ppocrv5_eslav_dict.txt");
        for path in [&runtime, &det, &rec, &dict] {
            if !path.is_file() {
                return Err(format!(
                    "Не найден обязательный OCR-ресурс: {}",
                    path.display()
                ));
            }
        }

        let dict_text = std::fs::read_to_string(&dict)
            .map_err(|error| format!("Не удалось прочитать словарь OCR: {error}"))?;
        let dict_len = dict_text
            .strip_suffix('\n')
            .unwrap_or(&dict_text)
            .split('\n')
            .count();
        if dict_len != 517 {
            return Err(format!(
                "Словарь OCR несовместим с моделью: ожидалось 517 символов, найдено {dict_len}"
            ));
        }

        preload_runtime_dependencies(&runtime_dir)?;
        ORT_READY
            .get_or_init(|| {
                ort::init_from(runtime.to_string_lossy())
                    .with_name("UTranslate")
                    .with_telemetry(false)
                    .commit()
                    .map(|_| ())
                    .map_err(|error| format!("Не удалось загрузить ONNX Runtime: {error}"))
            })
            .clone()?;

        let mut detector = DbNet::new();
        detector
            .init_model(&det.to_string_lossy(), 4, None)
            .map_err(|error| format!("Не удалось открыть detector OCR: {error}"))?;
        let recognizer = Session::builder()
            .and_then(|builder| builder.with_optimization_level(GraphOptimizationLevel::Level2))
            .and_then(|builder| builder.with_intra_threads(4))
            .and_then(|builder| builder.with_inter_threads(1))
            .and_then(|builder| builder.commit_from_file(&rec))
            .map_err(|error| format!("Не удалось открыть recognizer OCR: {error}"))?;
        let recognizer_input = recognizer
            .inputs
            .first()
            .ok_or_else(|| "У recognizer OCR нет входного tensor".to_string())?
            .name
            .clone();
        let mut keys = Vec::with_capacity(519);
        keys.push(String::new());
        keys.extend(
            dict_text
                .strip_suffix('\n')
                .unwrap_or(&dict_text)
                .split('\n')
                .map(|line| line.trim_end_matches('\r').to_string()),
        );
        keys.push(" ".to_string());
        if keys.len() != 519 {
            return Err(format!(
                "CTC vocabulary должен содержать 519 классов, найдено {}",
                keys.len()
            ));
        }

        Ok(Self {
            detector,
            recognizer,
            recognizer_input,
            keys,
            resource_root,
        })
    }

    fn recognize_image<F>(&mut self, source: &RgbImage, is_current: &F) -> Result<String, String>
    where
        F: Fn() -> bool,
    {
        let origins = tile_origins(source.width(), source.height())?;
        let mut blocks = Vec::new();
        let mut total_detections = 0usize;

        for (origin_x, origin_y, tile_width, tile_height) in origins {
            if !is_current() {
                return Err("OCR-запрос отменён".to_string());
            }
            let tile =
                imageops::crop_imm(source, origin_x, origin_y, tile_width, tile_height).to_image();
            let scale = if tile_width.max(tile_height) <= SMALL_IMAGE_LONG_EDGE {
                2
            } else {
                1
            };
            // Удвоение безопасно: ветка работает только для сторон не больше 464 px.
            let scaled = if scale == 2 {
                imageops::resize(
                    &tile,
                    tile_width * 2,
                    tile_height * 2,
                    imageops::FilterType::CatmullRom,
                )
            } else {
                tile
            };
            let padded = edge_pad(&scaled, SYNTHETIC_BORDER)?;
            let target = padded.width().max(padded.height()).min(960);
            let resize = ScaleParam::get_scale_param(&padded, target);
            let text_boxes = self
                .detector
                .get_text_boxes(&padded, &resize, 0.6, 0.3, 1.5)
                .map_err(|error| format!("Ошибка detector OCR: {error}"))?;
            if !is_current() {
                return Err("OCR-запрос отменён".to_string());
            }

            total_detections = add_detection_count(total_detections, text_boxes.len())?;
            // The detector can return many boxes. Extract and recognize one bounded batch
            // at a time so perspective crops cannot accumulate without a memory limit.
            for text_box_batch in text_boxes.chunks(REC_BATCH) {
                validate_crop_batch(&padded, text_box_batch)?;
                let crops = OcrUtils::get_part_images(&padded, text_box_batch);
                let recognized = self.recognize_crops(&crops, is_current)?;
                for (text_box, (text, score)) in text_box_batch.iter().zip(recognized) {
                    if let Some(located) =
                        map_block(text_box, text, score, origin_x, origin_y, scale)
                    {
                        blocks.push(located);
                    }
                }
            }
        }

        let blocks = deduplicate_blocks(blocks);
        assemble_reading_order(blocks)
    }

    fn recognize_crops<F>(
        &mut self,
        crops: &[RgbImage],
        is_current: &F,
    ) -> Result<Vec<(String, f32)>, String>
    where
        F: Fn() -> bool,
    {
        let mut result = vec![(String::new(), 0.0); crops.len()];
        let mut indices: Vec<usize> = (0..crops.len()).collect();
        indices.sort_by(|&a, &b| {
            let ar = crops[a].width() as f32 / crops[a].height().max(1) as f32;
            let br = crops[b].width() as f32 / crops[b].height().max(1) as f32;
            ar.partial_cmp(&br).unwrap_or(Ordering::Equal)
        });

        for batch in indices.chunks(REC_BATCH) {
            if !is_current() {
                return Err("OCR-запрос отменён".to_string());
            }
            let max_ratio =
                batch
                    .iter()
                    .fold(REC_BASE_WIDTH as f32 / REC_HEIGHT as f32, |max, &index| {
                        max.max(crops[index].width() as f32 / crops[index].height().max(1) as f32)
                    });
            let target_width = (REC_HEIGHT as f32 * max_ratio) as u32;
            if target_width > REC_MAX_WIDTH {
                return Err(format!(
                    "Строка текста слишком широкая для recognizer: {target_width}px после нормализации (предел {REC_MAX_WIDTH})"
                ));
            }
            let mut tensor =
                Array4::<f32>::zeros((batch.len(), 3, REC_HEIGHT as usize, target_width as usize));
            for (batch_index, &source_index) in batch.iter().enumerate() {
                let crop = &crops[source_index];
                let resized_width = ((REC_HEIGHT as f32 * crop.width() as f32
                    / crop.height().max(1) as f32)
                    .ceil() as u32)
                    .min(target_width)
                    .max(1);
                let resized = imageops::resize(
                    crop,
                    resized_width,
                    REC_HEIGHT,
                    imageops::FilterType::Triangle,
                );
                for (x, y, pixel) in resized.enumerate_pixels() {
                    for channel in 0..3 {
                        tensor[[batch_index, channel, y as usize, x as usize]] =
                            (pixel[channel] as f32 / 255.0 - 0.5) / 0.5;
                    }
                }
            }

            let value = Tensor::from_array(tensor)
                .map_err(|error| format!("Не удалось создать recognizer tensor: {error}"))?;
            let outputs = self
                .recognizer
                .run(
                    inputs![self.recognizer_input.clone() => value].map_err(|error| {
                        format!("Не удалось собрать recognizer inputs: {error}")
                    })?,
                )
                .map_err(|error| format!("Ошибка recognizer OCR: {error}"))?;
            let view = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|error| format!("Неверный output recognizer OCR: {error}"))?;
            let shape = view.shape();
            if shape.len() != 3 || shape[0] != batch.len() || shape[2] != self.keys.len() {
                return Err(format!(
                    "Неверная форма output recognizer OCR: {:?}, ожидалось [{}, T, {}]",
                    shape,
                    batch.len(),
                    self.keys.len()
                ));
            }
            let timesteps = shape[1];
            let vocabulary = shape[2];
            let data: Vec<f32> = view.iter().copied().collect();
            for (batch_index, &source_index) in batch.iter().enumerate() {
                let start = batch_index * timesteps * vocabulary;
                result[source_index] = decode_ctc(
                    &data[start..start + timesteps * vocabulary],
                    timesteps,
                    vocabulary,
                    &self.keys,
                )?;
            }
        }
        Ok(result)
    }
}

fn add_detection_count(total: usize, detected: usize) -> Result<usize, String> {
    let total = total
        .checked_add(detected)
        .ok_or_else(|| "Переполнение счётчика OCR-фрагментов".to_string())?;
    if total > MAX_DETECTIONS {
        return Err(format!(
            "Найдено слишком много фрагментов текста (предел {MAX_DETECTIONS}); выберите меньшую область"
        ));
    }
    Ok(total)
}

fn decode_ctc(
    logits: &[f32],
    timesteps: usize,
    vocabulary: usize,
    keys: &[String],
) -> Result<(String, f32), String> {
    if vocabulary != keys.len() || logits.len() != timesteps.saturating_mul(vocabulary) {
        return Err("Размер CTC output не совпадает со словарём".to_string());
    }
    let mut text = String::new();
    let mut previous = usize::MAX;
    let mut score_sum = 0.0;
    let mut score_count = 0usize;
    for timestep in 0..timesteps {
        let row = &logits[timestep * vocabulary..(timestep + 1) * vocabulary];
        let (index, score) = row
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Less))
            .unwrap_or((0, 0.0));
        if index != 0 && index != previous {
            text.push_str(&keys[index]);
            score_sum += score;
            score_count += 1;
        }
        previous = index;
    }
    Ok((
        text,
        if score_count == 0 {
            0.0
        } else {
            score_sum / score_count as f32
        },
    ))
}

/// Единственная проверка размера RGBA-буфера в проекте; `screen_capture` зовёт её же.
pub(crate) fn checked_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "Переполнение площади OCR-изображения".to_string())?;
    if pixels == 0 {
        return Err("Пустое OCR-изображение".to_string());
    }
    if pixels > MAX_INPUT_PIXELS {
        return Err(format!(
            "OCR-изображение слишком велико: {pixels} пикселей (предел {MAX_INPUT_PIXELS})"
        ));
    }
    usize::try_from(
        pixels
            .checked_mul(4)
            .ok_or_else(|| "Переполнение размера RGBA-буфера".to_string())?,
    )
    .map_err(|_| "RGBA-буфер не помещается в адресное пространство".to_string())
}

fn resolve_resource_root(resource_dir: &Path) -> Result<PathBuf, String> {
    let candidates = [resource_dir.join("ocr"), resource_dir.join("resources/ocr")];
    candidates
        .into_iter()
        .find(|path| path.join("models/ppocrv5_mobile_det.onnx").is_file())
        .ok_or_else(|| {
            format!(
                "OCR-ресурсы не найдены в каталоге приложения: {}",
                resource_dir.display()
            )
        })
}

fn rgba_to_bgr_image(width: u32, height: u32, rgba: &[u8]) -> Result<RgbImage, String> {
    let expected_rgba = checked_rgba_len(width, height)?;
    if rgba.len() != expected_rgba {
        return Err("Длина RGBA-буфера не совпадает с размерами BGR-изображения".to_string());
    }
    let bgr_len = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(3))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| "BGR-буфер слишком велик".to_string())?;
    let mut bgr = Vec::new();
    bgr.try_reserve_exact(bgr_len)
        .map_err(|_| format!("Недостаточно памяти для BGR-буфера размером {bgr_len} байт"))?;
    for pixel in rgba.chunks_exact(4) {
        // Paddle's official inference.yml declares DecodeImage img_mode=BGR. `RgbImage`
        // is used only as a contiguous three-channel container here, so store B,G,R.
        bgr.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    RgbImage::from_raw(width, height, bgr)
        .ok_or_else(|| "Не удалось создать BGR-изображение".to_string())
}

fn validate_crop_batch(source: &RgbImage, boxes: &[TextBox]) -> Result<(), String> {
    let mut total_pixels = 0u64;
    for text_box in boxes {
        let pixels = validate_text_box(source, text_box)?;
        total_pixels = total_pixels
            .checked_add(pixels)
            .ok_or_else(|| "Переполнение суммарной площади OCR-crop".to_string())?;
        if total_pixels > MAX_CROP_BATCH_PIXELS {
            return Err(format!(
                "Суммарная площадь OCR-crop в одном batch превышает предел {MAX_CROP_BATCH_PIXELS} пикселей"
            ));
        }
    }
    Ok(())
}

fn validate_text_box(source: &RgbImage, text_box: &TextBox) -> Result<u64, String> {
    if text_box.points.len() != 4 {
        return Err(format!(
            "Detector OCR вернул {} точек вместо 4",
            text_box.points.len()
        ));
    }
    if text_box
        .points
        .iter()
        .any(|point| point.x > source.width() || point.y > source.height())
    {
        return Err("Detector OCR вернул crop за границами изображения".to_string());
    }

    // The ppocr-rs perspective helper expects four ordered corners of a convex
    // quadrilateral. Validate that contract before it reaches its infallible API.
    let mut orientation = 0i8;
    for index in 0..4 {
        let a = text_box.points[index];
        let b = text_box.points[(index + 1) % 4];
        let c = text_box.points[(index + 2) % 4];
        let ab = (
            i64::from(b.x) - i64::from(a.x),
            i64::from(b.y) - i64::from(a.y),
        );
        let bc = (
            i64::from(c.x) - i64::from(b.x),
            i64::from(c.y) - i64::from(b.y),
        );
        let cross = i128::from(ab.0) * i128::from(bc.1) - i128::from(ab.1) * i128::from(bc.0);
        if cross == 0 {
            return Err("Detector OCR вернул вырожденный crop".to_string());
        }
        let sign = if cross > 0 { 1 } else { -1 };
        if orientation != 0 && sign != orientation {
            return Err("Detector OCR вернул невыпуклый или неупорядоченный crop".to_string());
        }
        orientation = sign;
    }

    let edge_pixels = |a: Point, b: Point| {
        let dx = f64::from(a.x) - f64::from(b.x);
        let dy = f64::from(a.y) - f64::from(b.y);
        (dx.mul_add(dx, dy * dy).sqrt().ceil() as u64).max(1)
    };
    let width = edge_pixels(text_box.points[0], text_box.points[1]);
    let height = edge_pixels(text_box.points[0], text_box.points[3]);
    width
        .checked_mul(height)
        .ok_or_else(|| "Переполнение площади perspective OCR-crop".to_string())
}

/// Плитка не бывает шире 4096 px, а рамка — 8 px, поэтому сложение не переполняется.
fn edge_pad(source: &RgbImage, border: u32) -> Result<RgbImage, String> {
    let width = source.width() + border * 2;
    let height = source.height() + border * 2;
    let mut padded = RgbImage::new(width, height);
    for y in 0..height {
        let sy = y.saturating_sub(border).min(source.height() - 1);
        for x in 0..width {
            let sx = x.saturating_sub(border).min(source.width() - 1);
            padded.put_pixel(x, y, *source.get_pixel(sx, sy));
        }
    }
    Ok(padded)
}

fn tile_origins(width: u32, height: u32) -> Result<Vec<(u32, u32, u32, u32)>, String> {
    let xs = axis_origins(width, TILE_WIDTH);
    let ys = axis_origins(height, TILE_HEIGHT);
    let count = xs.len().saturating_mul(ys.len());
    if count > MAX_TILES {
        return Err(format!(
            "Область требует слишком много OCR-плиток: {count} (предел {MAX_TILES}); выберите меньшую область"
        ));
    }
    let mut out = Vec::with_capacity(count);
    for &y in &ys {
        for &x in &xs {
            out.push((
                x,
                y,
                (width - x).min(TILE_WIDTH),
                (height - y).min(TILE_HEIGHT),
            ));
        }
    }
    Ok(out)
}

fn axis_origins(length: u32, side: u32) -> Vec<u32> {
    if length <= side {
        return vec![0];
    }
    let step = side - TILE_OVERLAP;
    let mut origins = Vec::new();
    let mut current = 0;
    loop {
        origins.push(current);
        if current + side >= length {
            break;
        }
        current = (current + step).min(length - side);
    }
    origins
}

fn map_block(
    block: &TextBox,
    text: String,
    score: f32,
    origin_x: u32,
    origin_y: u32,
    scale: u32,
) -> Option<LocatedBlock> {
    let mut xs = block.points.iter().map(|point| point.x as f32);
    let mut ys = block.points.iter().map(|point| point.y as f32);
    let left = xs.next()?;
    let top = ys.next()?;
    let (left, right) = block.points.iter().fold((left, left), |(lo, hi), point| {
        (lo.min(point.x as f32), hi.max(point.x as f32))
    });
    let (top, bottom) = block.points.iter().fold((top, top), |(lo, hi), point| {
        (lo.min(point.y as f32), hi.max(point.y as f32))
    });
    let inv = 1.0 / scale as f32;
    let unpad = |value: f32| (value - SYNTHETIC_BORDER as f32).max(0.0) * inv;
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(LocatedBlock {
        text,
        score,
        left: origin_x as f32 + unpad(left),
        top: origin_y as f32 + unpad(top),
        right: origin_x as f32 + unpad(right),
        bottom: origin_y as f32 + unpad(bottom),
    })
}

fn intersection_over_union(a: &LocatedBlock, b: &LocatedBlock) -> f32 {
    let width = (a.right.min(b.right) - a.left.max(b.left)).max(0.0);
    let height = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0.0);
    let intersection = width * height;
    let union = a.width() * a.height() + b.width() * b.height() - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn deduplicate_blocks(mut blocks: Vec<LocatedBlock>) -> Vec<LocatedBlock> {
    blocks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    let mut kept: Vec<LocatedBlock> = Vec::with_capacity(blocks.len());
    'candidate: for block in blocks {
        for existing in &kept {
            if intersection_over_union(&block, existing) >= 0.45
                || (block.text == existing.text
                    && (block.center_y() - existing.center_y()).abs()
                        <= block.height().max(existing.height()) * 0.45
                    && (block.left - existing.left).abs() <= block.height().max(existing.height()))
            {
                continue 'candidate;
            }
        }
        kept.push(block);
    }
    kept
}

fn same_visual_line(a: &LocatedBlock, b: &LocatedBlock) -> bool {
    let overlap = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0.0);
    overlap >= a.height().min(b.height()) * 0.45
        || (a.center_y() - b.center_y()).abs() <= a.height().max(b.height()) * 0.40
}

fn assemble_reading_order(mut blocks: Vec<LocatedBlock>) -> Result<String, String> {
    blocks.sort_by(|a, b| {
        a.center_y()
            .partial_cmp(&b.center_y())
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.left.partial_cmp(&b.left).unwrap_or(Ordering::Equal))
    });
    let mut lines: Vec<Vec<LocatedBlock>> = Vec::new();
    for block in blocks {
        if let Some(line) = lines.iter_mut().rev().find(|line| {
            line.iter()
                .any(|existing| same_visual_line(existing, &block))
        }) {
            line.push(block);
        } else {
            lines.push(vec![block]);
        }
    }
    lines.sort_by(|a, b| {
        let ay = a.iter().map(LocatedBlock::center_y).sum::<f32>() / a.len() as f32;
        let by = b.iter().map(LocatedBlock::center_y).sum::<f32>() / b.len() as f32;
        ay.partial_cmp(&by).unwrap_or(Ordering::Equal)
    });

    let mut output = String::new();
    for mut line in lines {
        line.sort_by(|a, b| a.left.partial_cmp(&b.left).unwrap_or(Ordering::Equal));
        let mut rendered = String::new();
        let mut previous: Option<&LocatedBlock> = None;
        for block in &line {
            if let Some(prev) = previous {
                let gap = block.left - prev.right;
                if gap > prev.height().max(block.height()) * 0.12 {
                    rendered.push(' ');
                }
            }
            rendered.push_str(block.text.trim());
            previous = Some(block);
        }
        let rendered = rendered.trim();
        if rendered.chars().count() > MAX_LINE_CHARS {
            return Err(format!(
                "Распознанная строка превышает предел {MAX_LINE_CHARS} символов"
            ));
        }
        if !rendered.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(rendered);
            if output.chars().count() > MAX_TOTAL_CHARS {
                return Err(format!(
                    "Распознанный текст превышает предел {MAX_TOTAL_CHARS} символов; выберите меньшую область"
                ));
            }
        }
    }
    Ok(output)
}

/// Грузит `onnxruntime.dll` заранее и по абсолютному пути, чтобы ort потом открыл ровно этот
/// модуль. CRT рядом не кладём: сам UTranslate.exe слинкован с `VCRUNTIME140*.dll`, поэтому
/// у запущенного приложения редист уже установлен, и `LOAD_LIBRARY_SEARCH_SYSTEM32` находит
/// те же библиотеки в System32.
fn preload_runtime_dependencies(runtime_dir: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::System::LibraryLoader::{
            LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        },
    };

    let path = runtime_dir.join("onnxruntime.dll");
    if !path.is_file() {
        return Err(format!("Не найден OCR runtime: {}", path.display()));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a valid NUL-terminated absolute path for the duration of the call.
    // The safe search flags restrict transitive DLL resolution to the loaded DLL directory
    // and Windows' trusted default directories. The handle intentionally lives to process exit.
    unsafe {
        LoadLibraryExW(
            PCWSTR(wide.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .map_err(|error| {
        format!(
            "Не удалось загрузить {}: {error}. Установите Visual C++ Redistributable 2015–2022 x64",
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use serde::Deserialize;
    use std::time::Instant;

    fn block(text: &str, left: f32, top: f32, right: f32, bottom: f32) -> LocatedBlock {
        LocatedBlock {
            text: text.into(),
            score: 0.9,
            left,
            top,
            right,
            bottom,
        }
    }

    fn text_box(points: &[(u32, u32)]) -> TextBox {
        TextBox {
            points: points.iter().map(|&(x, y)| Point { x, y }).collect(),
            score: 0.9,
        }
    }

    #[test]
    fn rejects_invalid_and_oversized_buffers() {
        assert!(checked_rgba_len(0, 1).is_err());
        assert!(checked_rgba_len(16_384, 16_384).is_err());
        assert_eq!(checked_rgba_len(2, 3).unwrap(), 24);
    }

    #[test]
    fn engine_is_released_only_after_three_idle_minutes() {
        assert!(!should_unload(0, 0));
        assert!(!should_unload(IDLE_UNLOAD_MS, 0));
        assert!(should_unload(IDLE_UNLOAD_MS + 1, 0));
        // Метка из будущего (запрос стартовал между двумя чтениями) не выгружает движок.
        assert!(!should_unload(1_000, 10_000));
    }

    #[test]
    fn tiles_cover_large_images_with_overlap_and_bound_count() {
        let tiles = tile_origins(1920, 1080).unwrap();
        assert_eq!(tiles.first().copied(), Some((0, 0, 1920, 1080)));
        let last = tiles.last().copied().unwrap();
        assert_eq!(last.0 + last.2, 1920);
        assert_eq!(last.1 + last.3, 1080);
        assert!(tile_origins(300_000, 1).is_err());
    }

    #[test]
    fn raw_detection_limit_accumulates_across_tiles() {
        let first_tile = add_detection_count(0, 1_500).unwrap();
        assert_eq!(first_tile, 1_500);
        let error = add_detection_count(first_tile, 501).unwrap_err();
        assert!(error.contains("слишком много фрагментов"));
    }

    #[test]
    fn padding_repeats_only_pixels_inside_selection() {
        let mut image = RgbImage::new(2, 1);
        image.put_pixel(0, 0, Rgb([1, 2, 3]));
        image.put_pixel(1, 0, Rgb([4, 5, 6]));
        let padded = edge_pad(&image, 1).unwrap();
        assert_eq!(padded.dimensions(), (4, 3));
        assert_eq!(padded.get_pixel(0, 0), &Rgb([1, 2, 3]));
        assert_eq!(padded.get_pixel(3, 2), &Rgb([4, 5, 6]));
    }

    #[test]
    fn rejects_malformed_or_out_of_bounds_crop_boxes() {
        let image = RgbImage::new(100, 100);
        assert!(validate_crop_batch(&image, &[text_box(&[(0, 0), (50, 0), (50, 50)])]).is_err());
        assert!(
            validate_crop_batch(&image, &[text_box(&[(0, 0), (101, 0), (101, 50), (0, 50)])])
                .is_err()
        );
        assert!(validate_crop_batch(
            &image,
            &[text_box(&[(0, 0), (100, 100), (0, 100), (100, 0)])]
        )
        .is_err());
    }

    #[test]
    fn rejects_pathological_overlapping_crop_allocation() {
        let image = RgbImage::new(2_048, 2_048);
        let boxes: Vec<_> = (0..5)
            .map(|_| text_box(&[(0, 0), (2_048, 0), (2_048, 2_048), (0, 2_048)]))
            .collect();
        let error = validate_crop_batch(&image, &boxes).unwrap_err();
        assert!(error.contains("площадь OCR-crop"));
    }

    #[test]
    fn ctc_contract_collapses_before_removing_blank() {
        let dict = ["", "a", "b", " "];
        let indices = [0usize, 1, 1, 0, 1, 2, 2, 3];
        let mut previous = usize::MAX;
        let mut text = String::new();
        for index in indices {
            if index != previous && index != 0 {
                text.push_str(dict[index]);
            }
            previous = index;
        }
        assert_eq!(text, "aab ");
    }

    #[test]
    fn reading_order_merges_regions_on_the_same_line() {
        let blocks = vec![
            block("world", 55.0, 1.0, 100.0, 21.0),
            block("Second", 0.0, 30.0, 55.0, 50.0),
            block("Hello", 0.0, 0.0, 45.0, 20.0),
        ];
        assert_eq!(
            assemble_reading_order(blocks).unwrap(),
            "Hello world\nSecond"
        );
    }

    #[test]
    fn overlapping_tile_detections_are_deduplicated() {
        let kept = deduplicate_blocks(vec![
            block("same", 0.0, 0.0, 100.0, 20.0),
            LocatedBlock {
                score: 0.8,
                ..block("same", 2.0, 0.0, 102.0, 20.0)
            },
        ]);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    #[ignore = "requires prepared models and generated synthetic fixtures"]
    fn real_models_recognize_synthetic_fixture() {
        let resources = std::env::var_os("UTRANSLATE_OCR_RESOURCE_DIR")
            .expect("set UTRANSLATE_OCR_RESOURCE_DIR");
        let fixture =
            std::env::var_os("UTRANSLATE_OCR_FIXTURE").expect("set UTRANSLATE_OCR_FIXTURE");
        let image = image::open(fixture).unwrap().to_rgba8();
        let started = Instant::now();
        let text = recognize(
            Path::new(&resources),
            image.width(),
            image.height(),
            image.as_raw(),
        )
        .unwrap();
        println!(
            "{}",
            serde_json::json!({
                "elapsedMs": started.elapsed().as_secs_f64() * 1000.0,
                "recognizedChars": text.chars().count(),
                "width": image.width(), "height": image.height(),
            })
        );
        assert!(!text.trim().is_empty());
    }

    #[derive(Deserialize)]
    struct SyntheticFixture {
        name: String,
        path: String,
        lines: Vec<String>,
        font_px: u32,
        theme: String,
    }

    fn metric_normalize(text: &str) -> String {
        text.lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }

    fn edit_distance(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut previous: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut current = vec![i + 1];
            for (j, cb) in b.iter().enumerate() {
                current.push(
                    (current[j] + 1)
                        .min(previous[j + 1] + 1)
                        .min(previous[j] + usize::from(ca != *cb)),
                );
            }
            previous = current;
        }
        previous[b.len()]
    }

    #[test]
    #[ignore = "requires prepared models and generated 84-fixture suite"]
    fn real_models_synthetic_suite() {
        let resources = PathBuf::from(
            std::env::var_os("UTRANSLATE_OCR_RESOURCE_DIR")
                .expect("set UTRANSLATE_OCR_RESOURCE_DIR"),
        );
        let fixtures_dir = PathBuf::from(
            std::env::var_os("UTRANSLATE_OCR_FIXTURES_DIR")
                .expect("set UTRANSLATE_OCR_FIXTURES_DIR"),
        );
        let fixtures: Vec<SyntheticFixture> =
            serde_json::from_slice(&std::fs::read(fixtures_dir.join("ground_truth.json")).unwrap())
                .unwrap();

        let mut edits = 0usize;
        let mut chars = 0usize;
        let mut exact_lines = 0usize;
        let mut total_lines = 0usize;
        let mut latencies = Vec::new();
        let mut rows = Vec::new();
        for fixture in fixtures {
            let fixture_path = fixtures_dir.join(Path::new(&fixture.path).file_name().unwrap());
            let image = image::open(fixture_path).unwrap().to_rgba8();
            let started = Instant::now();
            let predicted =
                recognize(&resources, image.width(), image.height(), image.as_raw()).unwrap();
            let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
            let reference = fixture.lines.join("\n");
            let reference_norm = metric_normalize(&reference);
            let predicted_norm = metric_normalize(&predicted);
            let row_edits = edit_distance(&reference_norm, &predicted_norm);
            let row_chars = reference_norm.chars().count().max(1);
            let predicted_lines: Vec<String> = predicted.lines().map(metric_normalize).collect();
            exact_lines += fixture
                .lines
                .iter()
                .filter(|line| predicted_lines.contains(&metric_normalize(line)))
                .count();
            total_lines += fixture.lines.len();
            edits += row_edits;
            chars += row_chars;
            latencies.push(latency_ms);
            rows.push((
                row_edits as f64 / row_chars as f64,
                fixture.name,
                fixture.font_px,
                fixture.theme,
                reference,
                predicted,
            ));
        }
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let median = latencies[latencies.len() / 2];
        let p95 = latencies
            [((latencies.len() as f64 * 0.95).ceil() as usize - 1).min(latencies.len() - 1)];
        let normalized_cer = edits as f64 / chars as f64;
        let exact_line_recall = exact_lines as f64 / total_lines as f64;
        let worst: Vec<_> = rows
            .iter()
            .take(8)
            .map(|row| {
                serde_json::json!({
                    "cer": row.0, "fixture": row.1, "fontPx": row.2,
                    "theme": row.3, "reference": row.4, "predicted": row.5,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "samples": rows.len(), "normalizedCer": normalized_cer,
                "exactLineRecall": exact_line_recall, "warmMedianMs": median,
                "warmP95Ms": p95, "worst": worst,
            })
        );
        assert!(
            normalized_cer <= 0.05,
            "Rust OCR CER regression: {normalized_cer}"
        );
        if rows.len() >= 48 {
            assert!(
                exact_line_recall >= 0.70,
                "Rust OCR line recall regression: {exact_line_recall}"
            );
        }
    }
}
