"""銅箔輪郭の向かい合う辺を測定する（既存スキルの方式を配列演算化）。"""
import numpy as np
import shapely
from shapely.geometry import LineString, Point
from shapely.geometry.polygon import orient
from shapely.strtree import STRtree

from .parser import polygons


def find_gaps(copper, threshold=0.1, tolerance=1e-7, arc_filter=True,
              negative=False, cluster_radius=0.3):
    """しきい値未満のギャップと除外数を返す。座標・距離の単位は mm。"""
    segments, ring_ids, indices, counts, normals = [], [], [], [], []
    offsets, lengths, islands = [], [], []
    rid = 0
    for island, poly in enumerate(polygons(copper)):
        poly = orient(poly, sign=1.0)
        for ring_number, ring in enumerate([poly.exterior, *poly.interiors]):
            if negative and ring_number == 0 and tuple(poly.bounds) == tuple(copper.bounds):
                # ネガ反転で設けた人工矩形は基板外形ではない。
                continue
            points = np.asarray(ring.coords)
            vectors = np.diff(points, axis=0)
            sizes = np.linalg.norm(vectors, axis=1)
            valid = sizes > 1e-10
            points = np.vstack((points[:-1][valid], points[-1]))
            vectors = np.diff(points, axis=0)
            sizes = np.linalg.norm(vectors, axis=1)
            cumulative = np.concatenate(([0.0], np.cumsum(sizes)))
            n = len(sizes)
            for k, (a, b, vector, size) in enumerate(zip(points, points[1:], vectors, sizes)):
                if size <= 1e-10:
                    continue
                segments.append(LineString([a, b]))
                ring_ids.append(rid)
                indices.append(k)
                counts.append(n)
                normals.append([vector[1] / size, -vector[0] / size])
                offsets.append(cumulative[k])
                lengths.append(cumulative[-1])
                islands.append(island)
            rid += 1
    if not segments:
        return [], {"segments": 0, "raw_pairs": 0, "corner_filtered_pairs": 0}
    lines = np.asarray(segments, dtype=object)
    ring_ids, indices, counts = map(np.asarray, (ring_ids, indices, counts))
    normals, offsets, lengths, islands = map(np.asarray, (normals, offsets, lengths, islands))
    starts = shapely.get_point(lines, 0)
    start_xy = shapely.get_coordinates(starts)
    end_xy = shapely.get_coordinates(shapely.get_point(lines, 1))
    tangent = end_xy - start_xy
    segment_length = np.linalg.norm(tangent, axis=1)
    tangent /= segment_length[:, None]
    tree = STRtree(lines)
    shapely.prepare(copper)
    violations = []
    filtered = 0
    # しきい値との差が許容差以内なら同値として扱い、丸め誤差による違反を除く。
    limit = threshold - tolerance
    # 全候補を一度にメモリへ持たず、線分ブロックごとに検索する。
    for base in range(0, len(lines), 2048):
        pair = tree.query(lines[base:base + 2048], predicate="dwithin", distance=limit)
        a, b = pair[0] + base, pair[1]
        keep = a < b
        a, b = a[keep], b[keep]
        same = ring_ids[a] == ring_ids[b]
        delta = np.abs(indices[a] - indices[b])
        keep = ~(same & (np.minimum(delta, counts[a] - delta) <= 1))
        a, b = a[keep], b[keep]
        if not len(a):
            continue
        shortest = shapely.shortest_line(lines[a], lines[b])
        d = shapely.length(shortest)
        pa, pb = shapely.get_point(shortest, 0), shapely.get_point(shortest, 1)
        xy_a, xy_b = shapely.get_coordinates(pa), shapely.get_coordinates(pb)
        # 平行辺には同距離の最短点が連続する。閉端上の点を選ぶと「銅の中」
        # と誤判定するため、投影が重なる範囲の中央で測定する。
        parallel = np.abs(tangent[a, 0] * tangent[b, 1] - tangent[a, 1] * tangent[b, 0]) < 1e-10
        projection0 = np.sum((start_xy[b] - start_xy[a]) * tangent[a], axis=1)
        projection1 = np.sum((end_xy[b] - start_xy[a]) * tangent[a], axis=1)
        lower = np.maximum(0, np.minimum(projection0, projection1))
        upper = np.minimum(segment_length[a], np.maximum(projection0, projection1))
        movable = parallel & (upper - lower > 1e-10)
        xy_a[movable] = start_xy[a[movable]] + tangent[a[movable]] * ((lower[movable] + upper[movable]) / 2)[:, None]
        projection_b = np.sum((xy_a[movable] - start_xy[b[movable]]) * tangent[b[movable]], axis=1)
        xy_b[movable] = start_xy[b[movable]] + tangent[b[movable]] * projection_b[:, None]
        d[movable] = np.linalg.norm(xy_b[movable] - xy_a[movable], axis=1)
        mid = (xy_a + xy_b) / 2
        direction = (xy_b - xy_a) / np.maximum(d[:, None], 1e-12)
        facing = ((np.sum(direction * normals[a], axis=1) >= 0.5 - 1e-9)
                  & (np.sum(-direction * normals[b], axis=1) >= 0.5 - 1e-9))
        # 零幅は、既に接続した銅箔の継ぎ目との区別ができないため測定しない。
        keep = (d > 1e-9) & (d < limit) & facing
        keep &= ~shapely.covers(copper, shapely.points(mid))
        a, b, d, xy_a, xy_b, mid = [v[keep] for v in (a, b, d, xy_a, xy_b, mid)]
        if not len(a):
            continue
        same = ring_ids[a] == ring_ids[b]
        sa = offsets[a] + shapely.distance(starts[a], shapely.points(xy_a))
        sb = offsets[b] + shapely.distance(starts[b], shapely.points(xy_b))
        arc = np.minimum(np.abs(sa - sb), lengths[a] - np.abs(sa - sb))
        drop = same & (arc < d * 6.0 + 0.05) if arc_filter else np.zeros(len(a), dtype=bool)
        filtered += int(np.count_nonzero(drop))
        for k in np.flatnonzero(~drop):
            ia, ib = int(islands[a[k]]), int(islands[b[k]])
            violations.append({
                "gap_mm": float(d[k]), "x_mm": float(mid[k, 0]), "y_mm": float(mid[k, 1]),
                "point_a_mm": xy_a[k].tolist(), "point_b_mm": xy_b[k].tolist(),
                "island_a": min(ia, ib), "island_b": max(ia, ib),
                "kind": "self_approach" if ia == ib else "separate_islands",
                "ring_distance_mm": float(arc[k]) if same[k] else None,
                "at_threshold": abs(float(d[k]) - threshold) <= tolerance,
            })
    raw_count = len(violations)
    grouped = cluster(violations, cluster_radius)
    return grouped, {"segments": len(lines), "raw_pairs": raw_count,
                     "corner_filtered_pairs": filtered}


def cluster(records, radius):
    """同じ島ペアの近傍をまとめ、最小距離の実測点を代表座標にする。"""
    if not records:
        return []
    points = [Point(v["x_mm"], v["y_mm"]) for v in records]
    tree = STRtree(points)
    used = np.zeros(len(records), dtype=bool)
    out = []
    for i in sorted(range(len(records)), key=lambda k: (records[k]["gap_mm"],
                                                        records[k]["x_mm"], records[k]["y_mm"])):
        if used[i]:
            continue
        representative = dict(records[i])
        same_pair = (representative["island_a"], representative["island_b"])
        members = []
        for j in tree.query(points[i], predicate="dwithin", distance=radius):
            if not used[j] and (records[j]["island_a"], records[j]["island_b"]) == same_pair:
                used[j] = True
                members.append(int(j))
        representative["sample_count"] = len(members)
        # まとめた範囲を保存し、画像にも全範囲が入るようにする。
        representative["sample_bounds_mm"] = [
            min(records[j]["x_mm"] for j in members), min(records[j]["y_mm"] for j in members),
            max(records[j]["x_mm"] for j in members), max(records[j]["y_mm"] for j in members)]
        out.append(representative)
    return out
