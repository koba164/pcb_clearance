"""既存 check-gerber-clearance スキル由来のアパーチャ・円弧形状処理。"""
import math
import re
from shapely.affinity import rotate, translate
from shapely.geometry import LineString, Point, Polygon, box
from shapely.validation import make_valid

QUAD_SEGS = 64

def circle(diameter):
    return Point(0, 0).buffer(diameter / 2.0, quad_segs=QUAD_SEGS)


def rect(w, h):
    return box(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)


def obround(w, h):
    r = min(w, h) / 2.0
    return box(-w / 2 + r, -h / 2 + r, w / 2 - r, h / 2 - r).buffer(r, quad_segs=QUAD_SEGS)


def regular_poly(params):
    d, n = params[0], int(params[1])
    rot = params[2] if len(params) > 2 else 0.0
    return Polygon([
        (d / 2 * math.cos(math.radians(rot + 360.0 * i / n)),
         d / 2 * math.sin(math.radians(rot + 360.0 * i / n)))
        for i in range(n)
    ])


# ------------------------------------------------------- アパーチャマクロ(AM)
# KiCad はカスタムパッドを FreePoly0/1... という AM で出す。RoundRect も AM。
# プリミティブを素直に評価すれば名前を決め打ちせずに済む。

def am_eval(expr, params):
    """AM の演算式を評価する。$n は実引数、x / X は乗算。"""
    e = re.sub(r"\$(\d+)",
               lambda m: "(%r)" % params.get(int(m.group(1)), 0.0), expr.strip())
    e = e.replace("x", "*").replace("X", "*")
    if not e or not re.fullmatch(r"[0-9eE+\-*/(). ]+", e):
        raise SystemExit(f"AM の式が解釈できない: {expr}")
    try:
        return float(eval(e, {"__builtins__": {}}, {}))
    except Exception:
        raise SystemExit(f"AM の式が評価できない: {expr}")


def am_primitive(code, a):
    """AM プリミティブ1個を (露光, 図形, 回転角deg) にする。"""
    if code == 7:                                   # サーマル（常に露光あり）
        cx, cy, outer, inner, gap, rot = a
        if not (outer > inner >= 0 and 0 <= gap < outer / math.sqrt(2)):
            raise ValueError("サーマルの径またはギャップが不正")
        annulus = circle(outer).difference(circle(inner))
        cross = box(-outer, -gap / 2, outer, gap / 2).union(
            box(-gap / 2, -outer, gap / 2, outer))
        return 1, translate(annulus.difference(cross), cx, cy), rot
    if code == 1:                                   # 円
        exp, dia, cx, cy = a[0], a[1], a[2], a[3]
        rot = a[4] if len(a) > 4 else 0.0
        return exp, translate(circle(dia), cx, cy), rot
    if code == 4:                                   # アウトライン
        exp, n = a[0], int(a[1])
        pts = [(a[2 + 2 * k], a[3 + 2 * k]) for k in range(n + 1)]
        rot = a[2 + 2 * (n + 1)] if len(a) > 2 + 2 * (n + 1) else 0.0
        return exp, Polygon(pts), rot
    if code == 5:                                   # 正多角形
        exp, n, cx, cy, dia = a[0], int(a[1]), a[2], a[3], a[4]
        rot = a[5] if len(a) > 5 else 0.0
        return exp, translate(regular_poly([dia, n]), cx, cy), rot
    if code in (20, 21, 22):                        # 線 (ベクタ / 中心 / 左下)
        exp, w = a[0], a[1]
        if code == 20:
            g = LineString([(a[2], a[3]), (a[4], a[5])]).buffer(
                w / 2.0, cap_style=2, join_style=2)
            rot = a[6] if len(a) > 6 else 0.0
        else:
            h, cx, cy = a[2], a[3], a[4]
            g = (box(cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2)
                 if code == 21 else box(cx, cy, cx + w, cy + h))
            rot = a[5] if len(a) > 5 else 0.0
        return exp, g, rot
    raise SystemExit(f"未対応の AM プリミティブ: {code}")


def macro_shape(name, statements, args):
    """AM の定義と %ADD の実引数から図形を組み立てる。"""
    params = {i + 1: v for i, v in enumerate(args)}
    shape = Polygon()
    for st in statements:
        st = st.strip()
        if not st or st.startswith("0"):            # 空行 / コメント
            continue
        m = re.fullmatch(r"\$(\d+)\s*=\s*(.+)", st)
        if m:                                       # 変数代入
            params[int(m.group(1))] = am_eval(m.group(2), params)
            continue
        parts = st.split(",")
        code = int(am_eval(parts[0], params))
        vals = [am_eval(v, params) for v in parts[1:]]
        try:
            exposure, g, rot = am_primitive(code, vals)
        except IndexError:
            raise SystemExit(f"AM {name} のプリミティブ {code} の引数が足りない")
        if rot:
            g = rotate(g, rot, origin=(0, 0))
        if not g.is_valid:
            g = make_valid(g)
        # 露光1で加算、0で減算。順序どおりに適用する(仕様どおり)。
        shape = shape.union(g) if exposure >= 0.5 else shape.difference(g)
    if shape.is_empty:
        raise SystemExit(f"AM {name} から図形が得られない")
    return shape


# ------------------------------------------------------------------ 円弧
def arc_points(start, end, i, j, cw, multi_quadrant):
    """G02/G03 の円弧を折れ線へ展開する。start で始まり end で終わる。

    多象限(G75)なら中心は start + (I, J) で一意。start == end は全円を意味する。
    単象限(G74)は I/J が符号なしなので中心候補が4つあり、90度以内に収まり
    かつ始点終点の半径差が最小のものを選ぶ。

    分割数は円1/4あたり QUAD_SEGS。フラッシュの円近似と同じ粗さになる。
    半径は始点側と終点側を線形補間する(ガーバーの丸め誤差で両者はわずかに
    食い違うため、端点を動かさずに吸収する)。
    """
    if multi_quadrant:
        centers = [(start[0] + i, start[1] + j)]
    else:
        centers = [(start[0] + si * abs(i), start[1] + sj * abs(j))
                   for si in (1, -1) for sj in (1, -1)]

    best = None            # (半径差, cx, cy, r0, r1, a0, sweep)
    fallback = None
    for cx, cy in centers:
        r0 = math.hypot(start[0] - cx, start[1] - cy)
        r1 = math.hypot(end[0] - cx, end[1] - cy)
        a0 = math.atan2(start[1] - cy, start[0] - cx)
        a1 = math.atan2(end[1] - cy, end[0] - cx)
        sweep = a1 - a0
        if cw:
            while sweep > 0.0:
                sweep -= 2 * math.pi
        else:
            while sweep < 0.0:
                sweep += 2 * math.pi
        cand = (abs(r0 - r1), cx, cy, r0, r1, a0, sweep)
        if fallback is None or cand[0] < fallback[0]:
            fallback = cand
        if multi_quadrant:
            # 始点と終点が同じなら全円 (単象限モードでは全円を表現できない)。
            if abs(sweep) < 1e-12:
                cand = (cand[0], cx, cy, r0, r1, a0,
                        -2 * math.pi if cw else 2 * math.pi)
        elif abs(sweep) > math.pi / 2 + 1e-9:
            continue
        if best is None or cand[0] < best[0]:
            best = cand

    if best is None:
        best = fallback
    _, cx, cy, r0, r1, a0, sweep = best
    if r0 < 1e-12 or not math.isfinite(sweep):
        return [start, end]

    n = max(2, int(math.ceil(abs(sweep) / (math.pi / 2) * QUAD_SEGS)))
    pts = []
    for k in range(n + 1):
        f = k / n
        a = a0 + sweep * f
        r = r0 + (r1 - r0) * f
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    pts[0], pts[-1] = start, end
    return pts
