"""座標一覧、ローカル HTML、および実銅箔の PNG を生成する。"""
import csv
import html
import json
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from shapely.geometry import box
from shapely.strtree import STRtree

from .parser import polygons


BACKGROUND = "#111b2b"
COPPER = "#d2a45c"
MARKER = "#ff5577"


def font(size):
    for name in ("C:/Windows/Fonts/consola.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


class Renderer:
    def __init__(self, copper):
        self.parts = polygons(copper)
        self.tree = STRtree(self.parts)
        self.bounds = copper.bounds

    def image(self, target, title, records, bounds=None, detail=None):
        width, height = 1100, 900
        image = Image.new("RGB", (width, height), BACKGROUND)
        draw = ImageDraw.Draw(image)
        draw.text((32, 18), title, font=font(23), fill="white")
        if detail:
            d = detail
            draw.text((32, 54), f"gap = {d['gap_mm']:.7f} mm  ({d['gap_mm'] * 1000:.3f} um)",
                      font=font(22), fill=MARKER)
            draw.text((32, 88), f"Gerber X = {d['x_mm']:.6f}  Y = {d['y_mm']:.6f} mm",
                      font=font(19), fill="white")
        else:
            draw.text((32, 58), f"{len(records)} locations | copper: gold, measured gap: red",
                      font=font(18), fill="white")
        x0, y0, x1, y1 = bounds or self.bounds
        margin = 0.04 * max(x1 - x0, y1 - y0, 0.1)
        x0, y0, x1, y1 = x0 - margin, y0 - margin, x1 + margin, y1 + margin
        zoom = min(1000 / max(x1 - x0, 0.1), 650 / max(y1 - y0, 0.1))
        left = (width - (x1 - x0) * zoom) / 2
        top = 140 + (650 - (y1 - y0) * zoom) / 2

        def pixel(x, y):
            return (left + (x - x0) * zoom, top + (y1 - y) * zoom)

        window = box(x0, y0, x1, y1)
        for index in self.tree.query(window):
            for poly in polygons(self.parts[index].intersection(window)):
                draw.polygon([pixel(x, y) for x, y in poly.exterior.coords], fill=COPPER)
                for hole in poly.interiors:
                    draw.polygon([pixel(x, y) for x, y in hole.coords], fill=BACKGROUND)
        label_boxes = []
        for record in records:
            px, py = pixel(record["x_mm"], record["y_mm"])
            if detail:
                draw.line([pixel(*record["point_a_mm"]), pixel(*record["point_b_mm"])],
                          fill=MARKER, width=3)
                draw.ellipse((px - 18, py - 18, px + 18, py + 18), outline=MARKER, width=2)
                draw.line((px - 34, py, px - 22, py), fill=MARKER, width=2)
                draw.line((px + 22, py, px + 34, py), fill=MARKER, width=2)
            else:
                # 全体図でも位置が分かるように円・十字と番号の引き出しを付ける。
                draw.ellipse((px - 12, py - 12, px + 12, py + 12), outline=BACKGROUND, width=6)
                draw.ellipse((px - 12, py - 12, px + 12, py + 12), outline=MARKER, width=3)
                draw.line((px - 18, py, px + 18, py), fill=MARKER, width=2)
                draw.line((px, py - 18, px, py + 18), fill=MARKER, width=2)
                label = str(record["id"])
                label_font = font(20)
                text_box = draw.textbbox((0, 0), label, font=label_font)
                label_width = text_box[2] - text_box[0] + 16
                label_height = 30
                # 近接箇所の番号が重ならない配置を探す。
                candidates = [(dx, dy) for dy in (-38, 12, -74, 48, -110, 84)
                              for dx in (24, -label_width - 24)]
                for dx, dy in candidates:
                    lx = max(8, min(width - label_width - 8, px + dx))
                    ly = max(130, min(790 - label_height, py + dy))
                    label_box = (lx, ly, lx + label_width, ly + label_height)
                    if not any(label_box[0] < other[2] + 4 and label_box[2] > other[0] - 4
                               and label_box[1] < other[3] + 4 and label_box[3] > other[1] - 4
                               for other in label_boxes):
                        break
                label_boxes.append(label_box)
                draw.line((px, py, lx + label_width / 2, ly + label_height / 2), fill=BACKGROUND, width=5)
                draw.line((px, py, lx + label_width / 2, ly + label_height / 2), fill=MARKER, width=2)
                draw.rounded_rectangle(label_box, radius=5, fill=BACKGROUND, outline=MARKER, width=2)
                draw.text((lx + 8 - text_box[0], ly + (label_height - text_box[3] + text_box[1]) / 2 - text_box[1]),
                          label, font=label_font, fill="white")
        draw.rectangle((left, top, left + (x1 - x0) * zoom,
                        top + (y1 - y0) * zoom), outline="#65718a")
        draw.text((32, 812), f"X: {x0:.4f} .. {x1:.4f} mm   Y: {y0:.4f} .. {y1:.4f} mm",
                  font=font(18), fill="white")
        bar_mm = 10 ** math.floor(math.log10(180 / zoom))
        draw.line((40, 855, 40 + bar_mm * zoom, 855), fill="white", width=3)
        draw.text((50 + bar_mm * zoom, 840), f"{bar_mm:g} mm | +Y up | all layers viewed from top",
                  font=font(16), fill="white")
        image.save(target)


def write_layer_images(copper, records, directory, stem, window_mm, display_name=None):
    renderer = Renderer(copper)
    directory = Path(directory)
    overview = f"{stem}-overview.png"
    title = display_name or stem
    renderer.image(directory / overview, title, records)
    for number, rec in enumerate(records, 1):
        rec["id"] = number
        rec["image"] = f"{stem}-{number:04d}.png"
        x0, y0, x1, y1 = rec["sample_bounds_mm"]
        half = max(window_mm / 2, (x1 - x0) / 2 + 0.4, (y1 - y0) / 2 + 0.4)
        x, y = rec["x_mm"], rec["y_mm"]
        renderer.image(directory / rec["image"], f"{title}  #{number}", [rec],
                       (x - half, y - half, x + half, y + half), rec)
    return overview


def save_reports(report, outdir):
    outdir = Path(outdir)
    (outdir / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    fields = ["layer", "id", "gap_mm", "x_mm", "y_mm", "kind", "at_threshold",
              "point_a_x_mm", "point_a_y_mm", "point_b_x_mm", "point_b_y_mm", "sample_count", "image"]
    with (outdir / "violations.csv").open("w", encoding="utf-8-sig", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        for layer in report["layers"]:
            for rec in layer.get("violations", []):
                row = {k: rec[k] for k in fields if k in rec}
                row.update(layer=layer["name"], point_a_x_mm=rec["point_a_mm"][0],
                           point_a_y_mm=rec["point_a_mm"][1], point_b_x_mm=rec["point_b_mm"][0],
                           point_b_y_mm=rec["point_b_mm"][1])
                writer.writerow(row)
    escape = html.escape
    body = ["<!doctype html><html lang='ja'><meta charset='utf-8'>",
            "<meta name='viewport' content='width=device-width'><title>ガーバークリアランス検査</title>",
            "<style>body{font:16px system-ui;max-width:1200px;margin:40px auto;padding:0 24px;",
            "background:#111b2b;color:#e4eaf5}a{color:#83c8ff}table{border-collapse:collapse;width:100%}",
            "td,th{padding:10px;border-bottom:1px solid #42516a;text-align:left}",
            ".table-scroll{overflow-x:auto}.preview{display:block;width:320px;max-width:none}",
            "figure{margin:20px 0}figcaption{margin:8px 0;color:#becbe0}",
            "img{max-width:100%;height:auto}summary{cursor:pointer;padding:10px}.error{color:#ff8199}",
            "code{overflow-wrap:anywhere}li{margin:8px 0}</style><h1>ガーバークリアランス検査</h1>",
            f"<p>判定: {report['threshold_mm']:g} mm 以下 / 検出 {report['violation_count']} 箇所 / "
            f"検査エラー {report['error_count']} 層</p>",
            "<p>座標: ガーバー原点基準、mm、X は右向き、Y は上向き。裏面も上面からの透視。</p>"]
    for layer in report["layers"]:
        if not layer.get("violations") and not layer.get("error"):
            continue
        body.append(f"<h2>{escape(layer['name'])}</h2>")
        if layer.get("error"):
            body.append(f"<p class='error'>検査失敗: {escape(layer['error'])}</p>")
            continue
        body.extend(f"<p>{escape(note)}</p>" for note in layer["warnings"])
        body.append(f"<p>{len(layer['violations'])} 箇所 / "
                    f"コーナー除外 {layer['detection']['corner_filtered_pairs']} 線分ペア</p>")
        body.append(f"<figure><figcaption>層全体の画像 — 赤いマークと番号が違反箇所のおおよその位置です。</figcaption>"
                    f"<a href='{layer['overview']}'><img src='{layer['overview']}' alt='{escape(layer['name'], quote=True)} の違反位置'></a></figure>")
        body.append("<div class='table-scroll'><table><tr><th>No.</th><th>ギャップ mm</th><th>X mm</th><th>Y mm</th><th>形状</th><th>違反箇所の画像</th></tr>")
        for rec in layer["violations"]:
            kind = "同一銅箔の自己接近" if rec["kind"] == "self_approach" else "別の銅箔どうし"
            body.append(f"<tr id='{layer['key']}-{rec['id']}'><td>{rec['id']}</td>"
                        f"<td>{rec['gap_mm']:.7f}</td><td>{rec['x_mm']:.6f}</td>"
                        f"<td>{rec['y_mm']:.6f}</td><td>{kind}</td>"
                        f"<td><a href='{rec['image']}'><img class='preview' loading='lazy' src='{rec['image']}' "
                        f"alt='違反箇所 {rec['id']} の拡大画像'></a></td></tr>")
        body.append("</table></div>")
    body.append("<h2>入力の選択</h2><ul>")
    body.extend(f"<li><code>{escape(item['path'])}</code>: {escape(item['reason'])}</li>"
                for item in report["skipped_inputs"])
    body.append("</ul><h2>補足</h2>"
                "<p><a href='violations.csv'>座標一覧 CSV</a> · <a href='report.json'>機械処理用 JSON</a></p><ul>")
    body.extend(f"<li>{escape(note)}</li>" for note in report["limitations"])
    body.append("</ul></html>")
    (outdir / "index.html").write_text("\n".join(body), encoding="utf-8")
