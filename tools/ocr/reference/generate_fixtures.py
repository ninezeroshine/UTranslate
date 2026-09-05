"""Generate the 84 neutral and 6 colored synthetic OCR fixtures used for the spike.

Requires Pillow. Output is intentionally untracked; ground_truth.json is deterministic.
"""

from __future__ import annotations

import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1] / "fixtures"
FONT = Path(r"C:/Windows/Fonts/segoeui.ttf")
TEXTS = [
    "Hello world", "Quick brown fox 123", "Settings > Privacy",
    r"File: C:\Users\Test", "https://example.com?q=42", "status_code = 200",
    "Привет, мир!", "Настройки и безопасность", "Перевод: русский + English",
    "Цена: 1 299,50 ₽", "Ошибка №42: файл не найден", "Ctrl+Shift+S — сохранить",
]
PANELS = [
    ["Open file", "Открыть папку", "Ctrl+O — выбрать"],
    ["Build: 2026.09.05", "Статус: готово", "https://localhost:5173"],
]
COLORS = [
    ("blue_light", "Settings: Blue accent", (245, 248, 252), (0, 102, 204), 16),
    ("red_light", "Ошибка: connection failed", (255, 248, 248), (210, 35, 45), 16),
    ("white_navy", "Перевод готов: English + русский", (12, 35, 70), (245, 248, 255), 16),
    ("yellow_dark", "Warning №42 — проверьте файл", (32, 32, 32), (255, 193, 7), 16),
    ("green_light", "Status 200: выполнено", (246, 252, 248), (22, 140, 75), 24),
    ("navy_pale", "https://example.com?q=цвет", (220, 235, 252), (15, 45, 90), 24),
]


def render_line(text: str, size: int, bg, fg, pad: int) -> Image.Image:
    font = ImageFont.truetype(str(FONT), size)
    box = font.getbbox(text)
    image = Image.new("RGB", (box[2] - box[0] + 2 * pad, box[3] - box[1] + 2 * pad), bg)
    ImageDraw.Draw(image).text((pad - box[0], pad - box[1]), text, font=font, fill=fg)
    return image


def main() -> None:
    neutral, colored = ROOT / "neutral", ROOT / "colored"
    neutral.mkdir(parents=True, exist_ok=True)
    colored.mkdir(parents=True, exist_ok=True)
    rows = []
    for size in (12, 16, 24):
        for theme in ("light", "dark"):
            bg, fg = ((248, 249, 250), (28, 30, 33)) if theme == "light" else ((31, 31, 31), (242, 242, 242))
            for index, text in enumerate(TEXTS):
                pad = (2, 8, 16)[index % 3]
                name = f"line_{size}_{theme}_{index:02d}_p{pad}"
                render_line(text, size, bg, fg, pad).save(neutral / f"{name}.png")
                rows.append({"name": name, "path": f"{name}.png", "lines": [text], "font_px": size, "theme": theme})
            font = ImageFont.truetype(str(FONT), size)
            for index, lines in enumerate(PANELS):
                boxes = [font.getbbox(text) for text in lines]
                widths = [box[2] - box[0] for box in boxes]
                heights = [box[3] - box[1] for box in boxes]
                gap, pad = max(5, size // 2), 10
                image = Image.new("RGB", (max(widths) + 2 * pad, sum(heights) + 2 * gap + 2 * pad), bg)
                draw, y = ImageDraw.Draw(image), pad
                for text, box, height in zip(lines, boxes, heights):
                    draw.text((pad - box[0], y - box[1]), text, font=font, fill=fg)
                    y += height + gap
                name = f"panel_{size}_{theme}_{index:02d}"
                image.save(neutral / f"{name}.png")
                rows.append({"name": name, "path": f"{name}.png", "lines": lines, "font_px": size, "theme": theme})
    (neutral / "ground_truth.json").write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")

    color_rows = []
    for name, text, bg, fg, size in COLORS:
        render_line(text, size, bg, fg, 16).save(colored / f"{name}.png")
        color_rows.append({"name": name, "path": f"{name}.png", "lines": [text], "font_px": size, "theme": name})
    (colored / "ground_truth.json").write_text(json.dumps(color_rows, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
