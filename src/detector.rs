use crate::{
    geometry::{Shape, XY},
    parser::bounds,
};
use geo::Orient;
use geo::orient::Direction;
use rstar::{
    AABB, RTree,
    primitives::{GeomWithData, Rectangle},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Violation {
    pub gap_mm: f64,
    pub x_mm: f64,
    pub y_mm: f64,
    pub point_a_mm: XY,
    pub point_b_mm: XY,
    pub island_a: usize,
    pub island_b: usize,
    pub kind: String,
    pub ring_distance_mm: Option<f64>,
    pub at_threshold: bool,
    pub sample_count: usize,
    pub sample_bounds_mm: [f64; 4],
    pub id: usize,
    pub image: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Detection {
    pub segments: usize,
    pub raw_pairs: usize,
    pub corner_filtered_pairs: usize,
}
struct Segment {
    a: XY,
    b: XY,
    t: XY,
    n: XY,
    size: f64,
    ring: usize,
    index: usize,
    count: usize,
    offset: f64,
    length: f64,
    island: usize,
}
fn sub(a: XY, b: XY) -> XY {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: XY, b: XY) -> XY {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: XY, b: f64) -> XY {
    [a[0] * b, a[1] * b]
}
fn dot(a: XY, b: XY) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: XY, b: XY) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: XY) -> f64 {
    a[0].hypot(a[1])
}
fn projection(p: XY, s: &Segment) -> XY {
    add(s.a, mul(s.t, dot(sub(p, s.a), s.t).clamp(0., s.size)))
}
fn shortest(a: &Segment, b: &Segment) -> (XY, XY, f64) {
    if cross(a.t, b.t).abs() < 1e-10 {
        let p0 = dot(sub(b.a, a.a), a.t);
        let p1 = dot(sub(b.b, a.a), a.t);
        let lo = 0f64.max(p0.min(p1));
        let hi = a.size.min(p0.max(p1));
        if hi - lo > 1e-10 {
            let pa = add(a.a, mul(a.t, (lo + hi) / 2.));
            let pb = projection(pa, b);
            return (pa, pb, norm(sub(pb, pa)));
        }
    } else {
        let delta = sub(b.a, a.a);
        let denominator = cross(a.t, b.t);
        let ta = cross(delta, b.t) / denominator;
        let tb = cross(delta, a.t) / denominator;
        if (0. ..=a.size).contains(&ta) && (0. ..=b.size).contains(&tb) {
            let p = add(a.a, mul(a.t, ta));
            return (p, p, 0.);
        }
    }
    [
        (a.a, projection(a.a, b)),
        (a.b, projection(a.b, b)),
        (projection(b.a, a), b.a),
        (projection(b.b, a), b.b),
    ]
    .into_iter()
    .map(|(pa, pb)| (pa, pb, norm(sub(pb, pa))))
    .min_by(|x, y| x.2.total_cmp(&y.2))
    .unwrap()
}

pub fn find_gaps(
    copper: &Shape,
    threshold: f64,
    tolerance: f64,
    arc_filter: bool,
    negative: bool,
    cluster_radius: f64,
) -> (Vec<Violation>, Detection) {
    let copper = copper.orient(Direction::Default);
    let all_bounds = bounds(&copper);
    let mut segments = vec![];
    let mut rid = 0;
    let mut frame_excluded = false;
    for (island, poly) in copper.0.iter().enumerate() {
        let b = bounds(&geo::MultiPolygon(vec![poly.clone()]));
        for (ring_number, ring) in std::iter::once(poly.exterior())
            .chain(poly.interiors())
            .enumerate()
        {
            if negative && ring_number == 0 && b == all_bounds {
                frame_excluded = true;
                continue;
            }
            let pts = ring
                .0
                .windows(2)
                .filter(|p| (p[1].x - p[0].x).hypot(p[1].y - p[0].y) > 1e-10)
                .map(|p| [p[0].x, p[0].y])
                .collect::<Vec<_>>();
            if pts.len() < 2 {
                continue;
            }
            let sizes = (0..pts.len())
                .map(|k| norm(sub(pts[(k + 1) % pts.len()], pts[k])))
                .collect::<Vec<_>>();
            let length = sizes.iter().sum();
            let mut offset = 0.;
            for k in 0..pts.len() {
                let (a, b, size) = (pts[k], pts[(k + 1) % pts.len()], sizes[k]);
                if size <= 1e-10 {
                    continue;
                }
                let t = mul(sub(b, a), 1. / size);
                segments.push(Segment {
                    a,
                    b,
                    t,
                    n: [t[1], -t[0]],
                    size,
                    ring: rid,
                    index: k,
                    count: pts.len(),
                    offset,
                    length,
                    island,
                });
                offset += size;
            }
            rid += 1;
        }
    }
    let tree = RTree::bulk_load(
        segments
            .iter()
            .enumerate()
            .map(|(i, s)| {
                GeomWithData::new(
                    Rectangle::from_corners(
                        [s.a[0].min(s.b[0]), s.a[1].min(s.b[1])],
                        [s.a[0].max(s.b[0]), s.a[1].max(s.b[1])],
                    ),
                    i,
                )
            })
            .collect(),
    );
    let mut stats = Detection {
        segments: segments.len(),
        ..Default::default()
    };
    let mut records = vec![];
    let limit = threshold - tolerance;
    for (i, a) in segments.iter().enumerate() {
        let window = AABB::from_corners(
            [a.a[0].min(a.b[0]) - limit, a.a[1].min(a.b[1]) - limit],
            [a.a[0].max(a.b[0]) + limit, a.a[1].max(a.b[1]) + limit],
        );
        for entry in tree.locate_in_envelope_intersecting(&window) {
            let j = entry.data;
            if i >= j {
                continue;
            }
            let b = &segments[j];
            let same = a.ring == b.ring;
            let delta = a.index.abs_diff(b.index);
            if same && delta.min(a.count - delta) <= 1 {
                continue;
            }
            let (pa, pb, d) = shortest(a, b);
            if d <= 1e-9 || d >= limit {
                continue;
            }
            let direction = mul(sub(pb, pa), 1. / d);
            if dot(direction, a.n) < 0.5 - 1e-9 || -dot(direction, b.n) < 0.5 - 1e-9 {
                continue;
            }
            let mid = mul(add(pa, pb), 0.5);
            // 輪郭の空間索引から水平レイの交点だけを数え、大きなネガ層でも全頂点を走査しない。
            let ray = AABB::from_corners(mid, [all_bounds[2], mid[1]]);
            let mut inside = frame_excluded;
            let mut boundary = false;
            for hit in tree.locate_in_envelope_intersecting(&ray) {
                let s = &segments[hit.data];
                if norm(sub(mid, projection(mid, s))) < 1e-12 {
                    boundary = true;
                    break;
                }
                if (s.a[1] > mid[1]) != (s.b[1] > mid[1]) {
                    let x = s.a[0] + (mid[1] - s.a[1]) * (s.b[0] - s.a[0]) / (s.b[1] - s.a[1]);
                    if x > mid[0] {
                        inside = !inside;
                    }
                }
            }
            if inside || boundary {
                continue;
            }
            let sa = a.offset + norm(sub(pa, a.a));
            let sb = b.offset + norm(sub(pb, b.a));
            let arc = (sa - sb).abs().min(a.length - (sa - sb).abs());
            if arc_filter && same && arc < d * 6. + 0.05 {
                stats.corner_filtered_pairs += 1;
                continue;
            }
            records.push(Violation {
                gap_mm: d,
                x_mm: mid[0],
                y_mm: mid[1],
                point_a_mm: pa,
                point_b_mm: pb,
                island_a: a.island.min(b.island),
                island_b: a.island.max(b.island),
                kind: if a.island == b.island {
                    "self_approach"
                } else {
                    "separate_islands"
                }
                .into(),
                ring_distance_mm: if same { Some(arc) } else { None },
                at_threshold: (d - threshold).abs() <= tolerance,
                sample_count: 1,
                sample_bounds_mm: [mid[0], mid[1], mid[0], mid[1]],
                id: 0,
                image: String::new(),
            });
        }
    }
    stats.raw_pairs = records.len();
    (cluster(records, cluster_radius), stats)
}
pub fn cluster(records: Vec<Violation>, radius: f64) -> Vec<Violation> {
    let tree = RTree::bulk_load(
        records
            .iter()
            .enumerate()
            .map(|(i, r)| GeomWithData::new([r.x_mm, r.y_mm], i))
            .collect(),
    );
    let mut used = vec![false; records.len()];
    let mut order = (0..records.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        records[a]
            .gap_mm
            .total_cmp(&records[b].gap_mm)
            .then(records[a].x_mm.total_cmp(&records[b].x_mm))
            .then(records[a].y_mm.total_cmp(&records[b].y_mm))
    });
    let mut out = vec![];
    for i in order {
        if used[i] {
            continue;
        }
        let mut rep = records[i].clone();
        let mut b = [rep.x_mm, rep.y_mm, rep.x_mm, rep.y_mm];
        let mut count = 0;
        for entry in tree.locate_within_distance([rep.x_mm, rep.y_mm], radius * radius) {
            let j = entry.data;
            let r = &records[j];
            if used[j] || (r.island_a, r.island_b) != (rep.island_a, rep.island_b) {
                continue;
            }
            used[j] = true;
            count += 1;
            b[0] = b[0].min(r.x_mm);
            b[1] = b[1].min(r.y_mm);
            b[2] = b[2].max(r.x_mm);
            b[3] = b[3].max(r.y_mm);
        }
        rep.sample_count = count;
        rep.sample_bounds_mm = b;
        rep.id = out.len() + 1;
        out.push(rep);
    }
    out
}
