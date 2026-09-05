# -*- coding: utf-8 -*-
"""Генератор артбордов направлений оверлея выбора области: python gen.py -> *.dc.html + canvas.json"""
import json

HEAD = """<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <script src="./support.js"></script>
</head>
<body>
<x-dc>
<helmet>
  <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Onest:wght@400;500;600;700&display=swap">
  <style>
    :root{
      --bg:#121A1E; --bg-2:#0D1417; --card:#1A2429; --tile:#212D33; --tile-2:#2A373E;
      --line:rgba(226,238,242,0.09); --line-2:rgba(226,238,242,0.06);
      --ink:#E4EDF0; --ink-2:#9BADB5; --ink-3:#6F818A;
      --water:#63B6C6; --water-deep:#7FCBDA; --water-soft:rgba(99,182,198,0.14); --water-line:rgba(99,182,198,0.34);
      --stone:#4A5B63; --mist:#7C99A5;
      --sh-pop-l:0 18px 44px rgba(24,42,50,0.16), 0 3px 10px rgba(24,42,50,0.07);
      --sh-pop:0 18px 44px rgba(0,0,0,0.50), 0 3px 10px rgba(0,0,0,0.34);
    }
    body{margin:0;font-family:'Onest','Segoe UI Variable','Segoe UI',system-ui,sans-serif;-webkit-font-smoothing:antialiased;}
    a{color:var(--water);} a:hover{color:var(--water-deep);}
    .ic{fill:none;stroke:currentColor;stroke-width:1.5;stroke-linecap:round;stroke-linejoin:round;display:block;flex-shrink:0;}
    .cap{font-size:11px;font-weight:600;letter-spacing:0.09em;text-transform:uppercase;color:var(--ink-3);}
    .sub{font-size:11px;color:var(--ink-3);}
    .desk{position:relative;width:440px;height:300px;border-radius:14px;overflow:hidden;background:#0B1013;border:1px solid var(--line);}
    .ed{position:absolute;left:16px;top:22px;width:296px;height:262px;border-radius:10px;background:#1C1F22;border:1px solid rgba(255,255,255,0.06);}
    .ed .tb{height:26px;border-bottom:1px solid rgba(255,255,255,0.06);display:flex;align-items:center;gap:6px;padding:0 10px;}
    .ed .tb i{display:block;width:8px;height:8px;border-radius:50%;background:#3A4248;}
    .ed .ln{position:absolute;left:18px;font:12px/16px 'Cascadia Mono','Consolas',monospace;color:#D5DADE;white-space:nowrap;}
    .doc{position:absolute;left:326px;top:48px;width:180px;height:236px;border-radius:10px;background:#F3F5F6;border:1px solid rgba(0,0,0,0.08);}
    .doc .l{position:absolute;left:16px;height:8px;border-radius:4px;background:#C9D2D6;}
    .dim{position:absolute;inset:0;}
    .sel{position:absolute;box-sizing:border-box;}
    .cur{position:absolute;width:18px;height:18px;margin:-9px 0 0 -9px;color:#FFFFFF;filter:drop-shadow(0 0 1px rgba(0,0,0,0.9));}
    .state{display:flex;flex-direction:column;gap:9px;}
    .state .lab{display:flex;align-items:baseline;gap:8px;white-space:nowrap;overflow:hidden;}
    .state .lab .sub{overflow:hidden;text-overflow:ellipsis;}
    .k{display:inline-flex;align-items:center;justify-content:center;min-width:22px;height:20px;padding:0 6px;border-radius:6px;font-size:11px;font-weight:600;line-height:1;}
    .k.dark{background:rgba(255,255,255,0.10);border:1px solid rgba(255,255,255,0.14);color:#E4EDF0;}
    .k.light{background:#EEF2F3;border:1px solid rgba(23,37,42,0.10);box-shadow:0 1px 0 rgba(23,37,42,0.16);color:#1B252A;}
  </style>
</helmet>
"""

TAIL = """
</x-dc>
</body>
</html>
"""

ICON_SCREEN = ('<svg width="14" height="14" viewBox="0 0 16 16" class="ic"><path d="M2.5 5.5v-2a1 1 0 0 1 1-1h2"></path>'
               '<path d="M10.5 2.5h2a1 1 0 0 1 1 1v2"></path><path d="M13.5 10.5v2a1 1 0 0 1-1 1h-2"></path>'
               '<path d="M5.5 13.5h-2a1 1 0 0 1-1-1v-2"></path><path d="M5 8h6"></path></svg>')
CURSOR = ('<svg viewBox="0 0 18 18" class="cur" style="left:{x}px;top:{y}px;">'
          '<path d="M9 1v5M9 12v5M1 9h5M12 9h5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" fill="none"></path>'
          '<circle cx="9" cy="9" r="1.3" fill="currentColor"></circle></svg>')

ED_LINES = [
    (40, "The trick is to lean on what the system"),
    (56, "already ships: the webview, the clipboard,"),
    (72, "the speech voices."),
    (104, "resilient"),
    (136, "давай созвонимся после обеда"),
    (152, "и посмотрим, что получилось"),
    (184, "cargo test  ·  57 passed"),
]
DOC_LINES = [(20, 120), (36, 96), (52, 132), (68, 80), (100, 128), (116, 104), (132, 140), (148, 72), (180, 116), (196, 90)]


def desk(inner):
    ed = '<div class="ed"><div class="tb"><i></i><i></i><i></i></div>' + "".join(
        '<div class="ln" style="top:%dpx;">%s</div>' % (t, s) for t, s in ED_LINES) + '</div>'
    doc = '<div class="doc">' + "".join(
        '<div class="l" style="top:%dpx;width:%dpx;"></div>' % (t, w) for t, w in DOC_LINES) + '</div>'
    return '<div class="desk">%s%s%s</div>' % (ed, doc, inner)


SEL = dict(l=30, t=150, w=258, h=52)
BR = (SEL["l"] + SEL["w"], SEL["t"] + SEL["h"])


def state(caption, note, inner):
    return ('<div class="state"><div class="lab"><span class="cap">%s</span><span class="sub">%s</span></div>%s</div>'
            % (caption, note, desk(inner)))


def artboard(title, subtitle, states, legend_html):
    body = ('<div style="width:1480px;height:520px;box-sizing:border-box;padding:30px 36px 32px 36px;color:var(--ink);'
            'background-color:var(--bg-2);display:flex;flex-direction:column;gap:18px;">'
            '<div style="display:flex;align-items:baseline;gap:12px;"><span style="font-size:19px;font-weight:600;letter-spacing:-0.02em;">%s</span>'
            '<span class="sub">%s</span></div>'
            '<div style="display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:24px;">%s</div>'
            '<div style="display:flex;flex-wrap:wrap;gap:8px 18px;font-size:12px;color:var(--ink-2);">%s</div>'
            '</div>') % (title, subtitle, "".join(states), legend_html)
    return HEAD + body + TAIL


def legend(items):
    return "".join('<span style="display:flex;align-items:center;gap:6px;"><span style="width:6px;height:6px;border-radius:50%%;background:var(--water);"></span>%s</span>' % i for i in items)


def pill_light(text, key=None, h=40):
    key_html = '<span class="k light">%s</span><span style="color:#8798A0;">отмена</span>' % key if key else ''
    return ('<div style="display:flex;align-items:center;gap:9px;height:%dpx;padding:0 16px;border-radius:999px;'
            'background:#F8FAFA;border:1px solid rgba(23,37,42,0.08);box-shadow:var(--sh-pop-l);color:#1B252A;font-size:13px;font-weight:600;white-space:nowrap;">'
            '%s<span>%s</span>%s</div>') % (h, ICON_SCREEN, text, key_html)


def pill_dark(text, key=None, h=36, alpha=0.94):
    key_html = '<span class="k dark">%s</span><span style="color:#9BADB5;">отмена</span>' % key if key else ''
    return ('<div style="display:flex;align-items:center;gap:9px;height:%dpx;padding:0 15px;border-radius:999px;'
            'background:rgba(27,37,42,%s);color:#E4EDF0;font-size:13px;font-weight:600;white-space:nowrap;">'
            '%s<span>%s</span>%s</div>') % (h, alpha, ICON_SCREEN, text, key_html)


def chip_light(text):
    return ('<div style="display:inline-flex;align-items:center;height:24px;padding:0 10px;border-radius:999px;'
            'background:#F8FAFA;border:1px solid rgba(23,37,42,0.08);box-shadow:0 3px 10px rgba(24,42,50,0.14);font-size:12px;font-weight:500;color:#55666C;white-space:nowrap;">%s</div>' % text)


def chip_dark(text, alpha=0.94):
    return ('<div style="display:inline-flex;align-items:center;height:24px;padding:0 10px;border-radius:999px;'
            'background:rgba(27,37,42,%s);font-size:12px;font-weight:500;color:#E4EDF0;white-space:nowrap;">%s</div>' % (alpha, text))


def at(x, y, html):
    return '<div style="position:absolute;left:%dpx;top:%dpx;">%s</div>' % (x, y, html)


def center_top(y, html):
    return '<div style="position:absolute;left:0;right:0;top:%dpx;display:flex;justify-content:center;">%s</div>' % (y, html)


def recognizing_pill():
    return ('<div style="display:flex;align-items:center;gap:10px;width:fit-content;height:40px;padding:0 16px;border-radius:999px;'
            'background:#F8FAFA;border:1px solid rgba(23,37,42,0.08);box-shadow:var(--sh-pop-l);">'
            '<span style="width:7px;height:7px;border-radius:50%;background:#2F6E7C;"></span>'
            '<span style="font-size:12px;font-weight:600;letter-spacing:0.07em;color:#55666C;">RU</span>'
            '<svg width="14" height="14" viewBox="0 0 14 14" class="ic" style="color:#8798A0;"><path d="M2.5 7h9"></path><path d="M8 3.5L11.5 7 8 10.5"></path></svg>'
            '<span style="font-size:12px;font-weight:600;letter-spacing:0.07em;color:#2F6E7C;">EN</span>'
            '<span style="font-size:12px;font-weight:500;color:#55666C;padding-left:2px;">Распознаём…</span></div>')


def sel_box(style, children=""):
    return '<div class="sel" style="left:%dpx;top:%dpx;width:%dpx;height:%dpx;%s">%s</div>' % (
        SEL["l"], SEL["t"], SEL["w"], SEL["h"], style, children)


def handles(color="#63B6C6", ring="#0E181C"):
    out = ""
    for x, y in [(0, 0), (1, 0), (0, 1), (1, 1)]:
        out += ('<span style="position:absolute;left:%s;right:%s;top:%s;bottom:%s;width:9px;height:9px;border-radius:50%%;'
                'background:%s;border:2px solid %s;box-sizing:border-box;"></span>') % (
            "-5px" if x == 0 else "auto", "-5px" if x == 1 else "auto",
            "-5px" if y == 0 else "auto", "-5px" if y == 1 else "auto", color, ring)
    return out


def cross(color):
    return ('<div style="position:absolute;left:0;right:0;top:158px;height:1px;background:%s;"></div>'
            '<div style="position:absolute;top:0;bottom:0;left:190px;width:1px;background:%s;"></div>') % (color, color)


def release_state(note="оверлей исчезает мгновенно, карточка встаёт под выделением"):
    return state("После отпускания", note, at(SEL["l"], BR[1] + 12, recognizing_pill()))


def current():
    dim = "rgba(0,0,0,0.44)"
    hint = pill_dark("Выделите область с текстом · Esc — отмена")
    idle = state("До протяжки", "затемнение 44 % чёрного, прицел белый 1 px",
                 '<div class="dim" style="background:%s;"></div>' % dim + cross("rgba(255,255,255,0.9)") + center_top(14, hint))
    drag = state("Протяжка", "рамка 2 px, прямые углы, бейдж внизу справа",
                 sel_box('border:2px solid #63B6C6;box-shadow:0 0 0 9999px %s;' % dim)
                 + at(BR[0] - 78, BR[1] + 8, chip_dark("258 × 52")) + center_top(14, hint) + CURSOR.format(x=BR[0], y=BR[1]))
    return artboard("Сейчас · как реализовано", "точка отсчёта: то, что работает в сборке сегодня",
                    [idle, drag, release_state()],
                    legend(["затемнение 44 % чёрного", "рамка 2 px вода, углы 0", "подсказка и бейдж — тёмные пилюли",
                            "прицел белый 1 px до и во время протяжки"]))


def direction_a():
    dim = "rgba(27,37,42,0.50)"
    hint = pill_light("Выделите область с текстом", key="Esc")
    idle = state("До протяжки", "мягкое затемнение тоном чернил, без прицела: только курсор",
                 '<div class="dim" style="background:%s;"></div>' % dim + center_top(14, hint) + CURSOR.format(x=190, y=158))
    drag = state("Протяжка", "матовая карточка: радиус 16, волосок, широкая тень",
                 sel_box('border-radius:16px;border:1px solid rgba(226,238,242,0.38);box-shadow:0 18px 44px rgba(0,0,0,0.42), 0 0 0 9999px %s;' % dim)
                 + at(BR[0] - 70, BR[1] + 8, chip_light("258 × 52")) + center_top(14, hint) + CURSOR.format(x=BR[0], y=BR[1]))
    return artboard("A · Карточка", "выделение читается как поднятая карточка, подсказка и бейдж — светлые пилюли из попапа",
                    [idle, drag, release_state("«Распознаём…» — та же пилюля, что подсказка")],
                    legend(["затемнение 50 % чернил #1B252A", "радиус 16, волосок 1 px, тень 0 18 44 / 42 %",
                            "подсказка: пилюля --card 40 px с кейкапом Esc", "бейдж: чип 24 px снаружи внизу справа",
                            "прицела нет: системный крестик"]))


def direction_b():
    dim = "rgba(44,66,76,0.58)"
    hint = pill_dark("Выделите область с текстом", key="Esc", alpha=0.92)
    idle = state("До протяжки", "холодный туман вместо чёрного, тонкий прицел цвета тумана",
                 '<div class="dim" style="background:%s;"></div>' % dim + cross("rgba(147,170,181,0.55)") + center_top(14, hint) + CURSOR.format(x=190, y=158))
    drag = state("Протяжка", "рамка 1,5 px, радиус 8, ручки по углам, прицел гаснет",
                 sel_box('border-radius:8px;border:1.5px solid #63B6C6;box-shadow:0 0 0 9999px %s;' % dim, handles())
                 + at(BR[0] - 70, BR[1] + 10, chip_dark("258 × 52", alpha=0.92)) + center_top(14, hint) + CURSOR.format(x=BR[0] + 2, y=BR[1] + 2))
    return artboard("B · Туман и ручки", "инструментальный характер: точный прицел, ручки, тёмные пилюли на 92 %",
                    [idle, drag, release_state("ручки исчезают вместе с оверлеем, карточка под выделением")],
                    legend(["затемнение 58 % сине-серого #2C424C", "рамка 1,5 px вода, радиус 8", "ручки 9 px с обводкой фона",
                            "прицел 1 px туман только до протяжки", "подсказка: тёмная пилюля 36 px, кейкап Esc"]))


def direction_c():
    flat = "rgba(27,37,42,0.52)"
    vign = "radial-gradient(240px 170px at 150px 158px, rgba(27,37,42,0.16), rgba(27,37,42,0.62) 75%)"
    hint = pill_dark("Выделите область", key="Esc", h=34, alpha=0.92)
    idle = state("До протяжки", "мягкий прожектор вокруг курсора, подсказка идёт за рукой",
                 '<div class="dim" style="background:%s;"></div>' % vign + at(166, 172, hint) + CURSOR.format(x=150, y=158))
    drag = state("Протяжка", "ровное затемнение, рамка 2 px с кромкой внутри",
                 sel_box('border-radius:12px;border:2px solid #63B6C6;box-shadow:inset 0 0 0 1px rgba(99,182,198,0.35), 0 0 0 9999px %s;' % flat)
                 + at(BR[0] - 70, BR[1] + 8, chip_light("258 × 52")) + CURSOR.format(x=BR[0], y=BR[1]))
    return artboard("C · Прожектор", "внимание там, где рука: виньетка вокруг курсора и подсказка рядом с ним",
                    [idle, drag, release_state("карточка под выделением, как в остальных вариантах")],
                    legend(["виньетка 16 → 62 % чернил до протяжки", "во время протяжки ровные 52 %",
                            "рамка 2 px вода, радиус 12, кромка 1 px внутри", "подсказка 34 px у курсора (+16, +14), гаснет при протяжке",
                            "бейдж внутри угла, если выделение выше 90 px, иначе снаружи"]))


def direction_d():
    dim = "rgba(27,37,42,0.40)"
    hint = pill_dark("Выделите область с текстом", key="Esc", h=32, alpha=0.86)
    idle = state("До протяжки", "лёгкое затемнение 40 %, маленькая подсказка, ничего лишнего",
                 '<div class="dim" style="background:%s;"></div>' % dim + center_top(12, hint) + CURSOR.format(x=190, y=158))
    drag = state("Протяжка", "рамка 2 px, радиус 6; подсказка ушла, бейдж после покоя",
                 sel_box('border-radius:6px;border:2px solid #63B6C6;box-shadow:0 0 0 9999px %s;' % dim)
                 + at(BR[0] - 70, BR[1] + 8, chip_dark("258 × 52", alpha=0.86)) + CURSOR.format(x=BR[0], y=BR[1]))
    return artboard("D · Тихий", "минимум хрома: подсказка гаснет через 1,5 с, бейдж не мельтешит, прицела нет",
                    [idle, drag, release_state("карточка под выделением")],
                    legend(["затемнение 40 % чернил", "рамка 2 px вода, радиус 6",
                            "подсказка 32 px на 86 %, исчезает через 1,5 с после первого движения",
                            "бейдж после 150 мс покоя курсора", "прицела нет"]))


def timeline_row(name, note, segments, total_ms=2400):
    """Полоса таймлайна: сегменты (start_ms, end_ms, kind) где kind: in|hold|out."""
    colors = {"in": "linear-gradient(90deg, rgba(99,182,198,0.10), rgba(99,182,198,0.85))",
              "hold": "rgba(99,182,198,0.85)",
              "out": "linear-gradient(90deg, rgba(99,182,198,0.85), rgba(99,182,198,0.10))"}
    bars = "".join('<div style="position:absolute;left:%.2f%%;width:%.2f%%;top:0;bottom:0;border-radius:4px;background:%s;"></div>'
                   % (a / total_ms * 100, (b - a) / total_ms * 100, colors[k]) for a, b, k in segments)
    return ('<div style="display:grid;grid-template-columns:150px 1fr 330px;gap:14px;align-items:center;">'
            '<span style="font-size:12px;font-weight:600;color:var(--ink);">%s</span>'
            '<div style="position:relative;height:12px;border-radius:4px;background:rgba(255,255,255,0.05);">%s</div>'
            '<span class="sub" style="font-size:11px;">%s</span></div>') % (name, bars, note)


def final_d():
    dim = "rgba(27,37,42,0.40)"
    hint = pill_dark("Выделите область с текстом", key="Esc", h=32, alpha=0.86)
    idle = state("До протяжки", "затемнение 40 % чернил, подсказка 32 px, прицела нет",
                 '<div class="dim" style="background:%s;"></div>' % dim + center_top(12, hint) + CURSOR.format(x=190, y=158))
    drag = state("Протяжка", "рамка 2 px вода, радиус 6; подсказка погасла; бейдж после покоя",
                 sel_box('border-radius:6px;border:2px solid #63B6C6;box-shadow:0 0 0 9999px %s;' % dim)
                 + at(BR[0] - 70, BR[1] + 8, chip_dark("258 × 52", alpha=0.86)) + CURSOR.format(x=BR[0], y=BR[1]))
    rel = release_state("выход 90 мс, карточка встаёт под выделением")
    ticks = "".join('<span style="position:absolute;left:%.2f%%;top:0;font-size:10px;color:var(--ink-3);white-space:nowrap;transform:translateX(-50%%);">%s</span>'
                    % (t / 2400 * 100, lab) for t, lab in [(0, "0"), (120, "120 мс"), (600, "0,6 с"), (1200, "1,2 с"), (1800, "1,8 с"), (2400, "2,4 с")])
    rows = [
        timeline_row("Затемнение", "0 → 40 % за 120 мс, ease-out; Esc гасит мгновенно, отпускание и ПКМ — за 90 мс",
                     [(0, 120, "in"), (120, 2100, "hold"), (2100, 2190, "out")]),
        timeline_row("Подсказка", "появляется вместе с затемнением; гаснет за 150 мс через 1,5 с после первого движения или сразу при нажатии",
                     [(0, 120, "in"), (120, 1700, "hold"), (1700, 1850, "out")]),
        timeline_row("Бейдж размера", "только в протяжке: 150 мс покоя курсора → вход 120 мс; любое движение прячет мгновенно",
                     [(1400, 1550, "in"), (1550, 2100, "hold"), (2100, 2190, "out")]),
        timeline_row("Рамка", "идёт за курсором без сглаживания: то, что тянут, не анимируют; гаснет вместе с затемнением",
                     [(1000, 2100, "hold"), (2100, 2190, "out")]),
    ]
    tl = ('<div style="display:flex;flex-direction:column;gap:10px;padding:16px 18px 14px 18px;border-radius:16px;background:var(--card);border:1px solid var(--line);">'
          '<div style="display:flex;align-items:baseline;gap:10px;"><span class="cap">Таймлайн одной сессии</span>'
          '<span class="sub">только прозрачность, easing 1 − (1 − t)³; при выключенных системных анимациях всё мгновенно</span></div>'
          '<div style="display:grid;grid-template-columns:150px 1fr 330px;gap:14px;"><span></span><div style="position:relative;height:14px;">%s</div><span></span></div>'
          '%s</div>') % (ticks, "".join(rows))
    body = ('<div style="width:1480px;height:760px;box-sizing:border-box;padding:30px 36px 32px 36px;color:var(--ink);'
            'background-color:var(--bg-2);display:flex;flex-direction:column;gap:18px;">'
            '<div style="display:flex;align-items:baseline;gap:12px;"><span style="font-size:19px;font-weight:600;letter-spacing:-0.02em;">Оверлей · Тихий</span>'
            '<span class="sub">выбранное направление D: минимум хрома, рисуется нативно через GDI+, анимируется только прозрачность</span></div>'
            '<div style="display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:24px;">%s</div>'
            '%s'
            '<div style="display:flex;flex-wrap:wrap;gap:8px 18px;font-size:12px;color:var(--ink-2);">%s</div>'
            '</div>') % ("".join([idle, drag, rel]), tl,
                         legend(["затемнение #1B252A на 40 %", "рамка 2 px #63B6C6, радиус 6, сглажена",
                                 "подсказка: пилюля 32 px на 86 % с кейкапом Esc", "бейдж: чип 24 px на 86 % снаружи у угла",
                                 "прицела нет: системный крестик"]))
    return HEAD + body + TAIL


FILES = {
    "Main.dc.html": final_d(),
    "DirectionA.dc.html": direction_a(),
    "DirectionB.dc.html": direction_b(),
    "DirectionC.dc.html": direction_c(),
    "DirectionD.dc.html": direction_d(),
    "Current.dc.html": current(),
}
for name, html in FILES.items():
    with open(name, "w", encoding="utf-8") as f:
        f.write(html)

CANVAS = {
    "pages": [{"id": "final", "name": "Оверлей · Тихий"}, {"id": "variants", "name": "Варианты"}],
    "artboards": [
        {"file": "Main.dc.html", "title": "Оверлей · Тихий", "x": 0, "y": 0, "w": 1480, "h": 760, "page": "final"},
        {"file": "Current.dc.html", "title": "Сейчас · как реализовано", "x": 0, "y": 0, "w": 1480, "h": 520, "page": "variants"},
        {"file": "DirectionA.dc.html", "title": "A · Карточка", "x": 0, "y": 660, "w": 1480, "h": 520, "page": "variants"},
        {"file": "DirectionB.dc.html", "title": "B · Туман и ручки", "x": 1580, "y": 660, "w": 1480, "h": 520, "page": "variants"},
        {"file": "DirectionC.dc.html", "title": "C · Прожектор", "x": 0, "y": 1320, "w": 1480, "h": 520, "page": "variants"},
        {"file": "DirectionD.dc.html", "title": "D · Тихий", "x": 1580, "y": 1320, "w": 1480, "h": 520, "page": "variants"},
    ],
    "annotations": [
        {"id": "ramka", "x": 1580, "y": 0, "w": 720, "page": "variants",
         "text": ("Оверлей выбора области · четыре направления\n"
                  "Общее для всех: оверлей открывается хоткеем, поэтому появляется мгновенно; единственная анимация — "
                  "затемнение набирает плотность за 100 мс (ease-out). Рамка и бейдж идут за курсором без задержки. "
                  "Закрытие мгновенное: под оверлеем тот же снимок, что и живой экран.\n"
                  "Рисуется нативно (GDI/GDI+): доступны сглаженные скругления, полупрозрачные заливки, мягкие тени, виньетка. "
                  "Недоступен блюр под окном, по гайду он и не нужен.\n"
                  "Попап после отпускания встаёт под выделением, выровнен по его левому краю; если места нет, над ним.")},
        {"id": "za-i-protiv", "x": 1580, "y": 290, "w": 720, "page": "variants",
         "text": ("A · Карточка — рекомендую\n"
                  "За: тот же материал, что у попапа и тоста (светлая пилюля, волосок, тень); выделение читается как поднятая карточка; ничего не мельтешит.\n"
                  "Против: светлые пилюли на светлом рабочем столе держатся только на тени и волоске; скруглённые углы визуальные, OCR берёт прямоугольник целиком.\n\n"
                  "B · Туман и ручки\n"
                  "За: точность инструмента, ручки подсказывают, что область можно тянуть.\n"
                  "Против: больше элементов на экране; ручки обещают то, чего пока нет (тянуть за них нельзя).\n\n"
                  "C · Прожектор\n"
                  "За: внимание идёт за рукой, подсказка рядом с курсором.\n"
                  "Против: движущаяся подсказка — лишнее движение для инструмента с хоткея; виньетка на 4K дороже плоского затемнения.\n\n"
                  "D · Тихий\n"
                  "За: минимум хрома, ничего не отвлекает.\n"
                  "Против: меньше подсказок новичку; исчезающая подсказка — ещё одно правило, которое надо помнить.")},
    ],
    "launch": {"view": "canvas", "page": "final"},
}
with open("canvas.json", "w", encoding="utf-8") as f:
    json.dump(CANVAS, f, ensure_ascii=False, indent=2)
print("written", list(FILES))
