"""RS-274X を描画順・極性を維持して mm の銅箔形状へ変換する。"""
from dataclasses import dataclass, field
import math
import re

from shapely.affinity import rotate, scale, translate
from shapely.geometry import LineString, Polygon, box
from shapely.ops import unary_union
from shapely.validation import make_valid

from .geometry import (QUAD_SEGS, arc_points, circle, macro_shape, obround,
                       rect, regular_poly)


class GerberError(ValueError):
    """検査結果を信用できない入力は明示的に失敗させる。"""


def polygons(geometry):
    """輪郭修復で生じる線・点を除いて面だけ取り出す。"""
    if geometry.geom_type == "Polygon":
        return [geometry] if geometry.area > 0 else []
    return [p for g in getattr(geometry, "geoms", []) for p in polygons(g)]


def metadata(text):
    """通常の X2 属性と Altium の G04 #@! 属性の両方を読む。"""
    attrs = {}
    for name, value in re.findall(r"TF\.([A-Za-z]+),([^*%\r\n]*)", text):
        attrs[name] = value.strip()
    return attrs


@dataclass
class ParsedLayer:
    copper: object
    metadata: dict
    statistics: dict
    warnings: list = field(default_factory=list)


class GerberParser:
    def __init__(self, text, polarity="auto", padding=1.0):
        self.text = text
        self.attrs = metadata(text)
        self.polarity = polarity
        self.padding = padding
        self.apertures = {}
        self.macros = {}
        self.batch = []
        self.image = Polygon()
        self.dark = True
        self.stats = dict(flashes=0, draws=0, regions=0, arcs=0)
        self.warnings = []

    def flush(self):
        if self.batch:
            combined = unary_union(self.batch)
            self.image = (self.image.union(combined) if self.dark
                          else self.image.difference(combined))
            self.batch.clear()

    def emit(self, shape):
        self.batch.extend(polygons(make_valid(shape) if not shape.is_valid else shape))

    def coord(self, value, axis):
        if value is None:
            return None
        sign = -1 if value.startswith("-") else 1
        digits = value.lstrip("+-")
        integer, decimal = self.formats[axis]
        if len(digits) > integer + decimal:
            raise GerberError(f"座標の桁数超過: {value}")
        if self.zero == "T":
            digits = digits.ljust(integer + decimal, "0")
        return sign * int(digits) / 10 ** decimal * self.unit

    def aperture(self, definition):
        m = re.fullmatch(r"ADD(\d+)([A-Za-z_$.][\w$.\-]*)(?:,(.*))?", definition)
        if not m:
            raise GerberError(f"不正なアパーチャ定義: {definition}")
        number, name, params = m.groups()
        args = [float(v) for v in params.split("X")] if params else []
        if name in self.macros:
            shape = macro_shape(name, self.macros[name], args)
            width = None
        else:
            if name == "C":
                shape, hole, width = circle(args[0]), args[1:], args[0]
            elif name == "R":
                shape, hole, width = rect(*args[:2]), args[2:], None
            elif name == "O":
                shape, hole, width = obround(*args[:2]), args[2:], None
            elif name == "P":
                shape, hole, width = regular_poly(args), args[3:], None
            else:
                raise GerberError(f"未対応のアパーチャ: {name}")
            if len(hole) > 2:
                raise GerberError("アパーチャの穴の引数が多すぎる")
            if hole and hole[0] > 0:
                cut = circle(hole[0]) if len(hole) == 1 else rect(*hole)
                shape = shape.difference(cut)
                width = None
        self.apertures[int(number)] = (scale(shape, self.unit, self.unit, origin=(0, 0)),
                                      name, width * self.unit if width is not None else None)

    def parse(self):
        fs = re.findall(r"%FS([LT])([AI])X(\d)(\d)Y(\d)(\d)\*%", self.text)
        units = re.findall(r"%MO(MM|IN)\*%", self.text)
        if len(fs) != 1 or fs[0][1] != "A" or len(units) != 1:
            raise GerberError("FS の絶対座標指定および単一の MO 単位指定が必要")
        self.zero = fs[0][0]
        self.formats = [(int(fs[0][2]), int(fs[0][3])),
                        (int(fs[0][4]), int(fs[0][5]))]
        self.unit = 25.4 if units[0] == "IN" else 1.0
        x = y = 0.0
        aperture = None
        operation = None
        interpolation, multi = 1, True
        region, contour, contours = False, [], []
        mirror, angle, factor = "N", 0.0, 1.0
        image_negative = False
        ended = False

        def finish_contour():
            if len(contour) == 1:
                contour.clear()
                return
            if contour:
                if len(contour) < 4 or math.dist(contour[0], contour[-1]) > 1e-7:
                    raise GerberError("リージョンの輪郭が閉じていない")
                contours.append(Polygon(contour))
                contour.clear()

        # トークンの取りこぼしは未対応コマンドとして扱う。
        for match in re.finditer(r"%[^%]*%|[^%*]+\*|[^\s]+", self.text):
            raw = match.group().strip()
            if not raw:
                continue
            if ended:
                raise GerberError("M02 の後にコマンドがある")
            if raw.startswith("%"):
                body = raw[1:-1].strip().rstrip("*")
                if body.startswith("AM"):
                    name, *statements = body[2:].split("*")
                    self.macros[name] = statements
                    continue
                for command in body.split("*"):
                    command = command.strip()
                    if command.startswith("ADD"):
                        self.aperture(command)
                    elif command.startswith(("FS", "MO", "TF", "TA", "TO", "TD", "IN", "LN")):
                        pass
                    elif command in ("LPD", "LPC"):
                        if region:
                            raise GerberError("リージョン中の極性変更")
                        self.flush()
                        self.dark = command == "LPD"
                    elif re.fullmatch(r"LM(N|X|Y|XY)", command):
                        mirror = command[2:]
                    elif re.fullmatch(r"LR[+-]?[\d.]+", command):
                        angle = float(command[2:])
                    elif re.fullmatch(r"LS[\d.]+", command):
                        factor = float(command[2:])
                        if factor <= 0:
                            raise GerberError("LS は正数が必要")
                    elif command in ("IPPOS", "IPNEG"):
                        image_negative = command == "IPNEG"
                    else:
                        raise GerberError(f"未対応の拡張コマンド: {command}")
                continue
            if not raw.endswith("*"):
                raise GerberError(f"終端 * のないコマンド: {raw[:100]}")
            body = raw[:-1].strip()
            if body.startswith("G04"):
                continue
            if body in ("M02", "M00"):
                ended = True
                continue
            if body in ("G70", "G71", "G90"):
                if body in ("G70", "G71") and (body == "G70") != (self.unit == 25.4):
                    raise GerberError("G70/G71 と MO の単位が矛盾")
                continue
            if body == "G36":
                if region:
                    raise GerberError("リージョンの入れ子")
                region, contour, contours = True, [], []
                continue
            if body == "G37":
                if not region:
                    raise GerberError("G36 のない G37")
                finish_contour()
                # 別々の輪郭は和集合。穴は単一輪郭の cut-in から復元する。
                shape = unary_union([make_valid(poly) for poly in contours])
                self.emit(shape)
                self.stats["regions"] += 1
                region = False
                continue
            for code in re.findall(r"G0?[123]|G7[45]|G54", body):
                if code in ("G74", "G75"):
                    multi = code == "G75"
                elif code != "G54":
                    interpolation = int(code[1:])
            body = re.sub(r"G0?[123](?!\d)|G7[45]|G54", "", body)
            if not body:
                continue
            select = re.fullmatch(r"D(\d+)", body)
            if select and int(select[1]) >= 10:
                aperture = int(select[1])
                if aperture not in self.apertures:
                    raise GerberError(f"未定義の D{aperture}")
                continue
            cm = re.fullmatch(r"(?:X([+-]?\d+))?(?:Y([+-]?\d+))?"
                              r"(?:I([+-]?\d+))?(?:J([+-]?\d+))?(?:D0?([123]))?", body)
            if not cm:
                raise GerberError(f"未対応のコマンド: {body[:100]}")
            xv, yv, iv, jv, op = cm.groups()
            nx = self.coord(xv, 0) if xv is not None else x
            ny = self.coord(yv, 1) if yv is not None else y
            operation = int(op) if op else operation
            if operation is None:
                raise GerberError("描画操作 D01/D02/D03 が未指定")
            if operation == 1:
                if interpolation in (2, 3):
                    io, jo = self.coord(iv, 0) or 0.0, self.coord(jv, 1) or 0.0
                    if not (io or jo):
                        raise GerberError("円弧の I/J 中心がない")
                    path = arc_points((x, y), (nx, ny), io, jo, interpolation == 2, multi)
                    self.stats["arcs"] += 1
                else:
                    path = [(x, y), (nx, ny)]
            if region:
                if operation == 2:
                    finish_contour()
                    contour = [(nx, ny)]
                elif operation == 1:
                    contour.extend(path[1:] if contour else path)
                else:
                    raise GerberError("リージョン中のフラッシュ")
            elif operation in (1, 3):
                if aperture not in self.apertures:
                    raise GerberError("描画アパーチャが未選択")
                shape, name, width = self.apertures[aperture]
                shape = scale(shape, -factor if "X" in mirror else factor,
                              -factor if "Y" in mirror else factor, origin=(0, 0))
                if angle:
                    shape = rotate(shape, angle, origin=(0, 0))
                if operation == 3:
                    self.emit(translate(shape, nx, ny))
                    self.stats["flashes"] += 1
                else:
                    if name == "C" and width is not None:
                        self.emit(LineString(path).buffer(width * factor / 2, quad_segs=QUAD_SEGS))
                    elif name == "R" and shape.geom_type == "Polygon" and not shape.interiors:
                        # 固定姿勢の矩形を線分に沿って掃引する（丸い線端にはしない）。
                        self.emit(unary_union([
                            unary_union([translate(shape, *a), translate(shape, *b)]).convex_hull
                            for a, b in zip(path, path[1:])]))
                    else:
                        raise GerberError(f"未対応のストロークアパーチャ D{aperture} ({name})")
                    self.stats["draws"] += 1
            x, y = nx, ny
        if region or not ended:
            raise GerberError("リージョンまたはファイルが終端されていない")
        self.flush()
        if self.image.is_empty:
            raise GerberError("面形状が得られない")
        file_negative = self.attrs.get("FilePolarity", "Positive") == "Negative"
        negative = image_negative != file_negative
        if self.polarity != "auto":
            negative = self.polarity == "negative"
        if negative:
            # 無限平面を余白付きの有限矩形で代用する。人工外周は検出器から除く。
            x0, y0, x1, y1 = self.image.bounds
            frame = box(x0 - self.padding, y0 - self.padding,
                        x1 + self.padding, y1 + self.padding)
            self.image = frame.difference(self.image)
            self.warnings.append("ネガ層の外側は無限銅箔として解釈。基板外形との距離は検査対象外。")
        self.stats.update(apertures=len(self.apertures), negative=negative,
                          area_mm2=self.image.area, bounds_mm=list(self.image.bounds))
        return ParsedLayer(self.image, self.attrs, self.stats, self.warnings)
