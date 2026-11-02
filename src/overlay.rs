use crate::geometry::Shape;
use clipper2_rust::{ClipType, Clipper64, FillRule, Path64, Paths64, Point64, PolyTree64};
use geo::orient::Direction;
use geo::{LineString, MultiPolygon, Orient, Polygon};

// 全演算で固定の格子を使い、局所形状と層全体の再量子化による偽スリットを防ぐ。
pub const GRID_MM: f64 = 1e-10;
const SCALE: f64 = 1. / GRID_MM;
fn paths(shape: &Shape) -> Paths64 {
    shape
        .0
        .iter()
        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
        .map(|ring| {
            ring.0
                .iter()
                .take(ring.0.len().saturating_sub(1))
                .map(|p| {
                    assert!(
                        p.x.is_finite()
                            && p.y.is_finite()
                            && p.x.abs() <= 100_000.
                            && p.y.abs() <= 100_000.,
                        "形状の座標は有限値かつ ±100000 mm 以内が必要です"
                    );
                    Point64::new((p.x * SCALE).round() as i64, (p.y * SCALE).round() as i64)
                })
                .collect()
        })
        .collect()
}
fn ring(path: &Path64) -> LineString<f64> {
    let mut line = LineString::from(
        path.iter()
            .map(|p| (p.x as f64 / SCALE, p.y as f64 / SCALE))
            .collect::<Vec<_>>(),
    );
    line.close();
    line
}
fn execute(subject: &Paths64, clip: &Paths64, op: ClipType, fill: FillRule) -> Shape {
    let mut engine = Clipper64::new();
    engine.add_subject(subject);
    engine.add_clip(clip);
    let mut tree = PolyTree64::new();
    let mut open = vec![];
    assert!(
        engine.execute_tree(op, fill, &mut tree, &mut open),
        "64bit 形状演算に失敗しました"
    );
    let mut out = vec![];
    for (i, node) in tree.nodes.iter().enumerate().skip(1) {
        if tree.is_hole(i) || node.polygon().is_empty() {
            continue;
        }
        let holes = node
            .children()
            .iter()
            .map(|&j| ring(tree.nodes[j].polygon()))
            .collect();
        out.push(Polygon::new(ring(node.polygon()), holes));
    }
    MultiPolygon(out).orient(Direction::Default)
}
pub trait GeometryOps {
    fn as_shape(&self) -> Shape;
    fn union(&self, other: &Shape) -> Shape {
        execute(
            &paths(&self.as_shape()),
            &paths(other),
            ClipType::Union,
            FillRule::EvenOdd,
        )
    }
    fn difference(&self, other: &Shape) -> Shape {
        execute(
            &paths(&self.as_shape()),
            &paths(other),
            ClipType::Difference,
            FillRule::EvenOdd,
        )
    }
    fn intersection(&self, other: &Shape) -> Shape {
        execute(
            &paths(&self.as_shape()),
            &paths(other),
            ClipType::Intersection,
            FillRule::EvenOdd,
        )
    }
}
impl GeometryOps for Shape {
    fn as_shape(&self) -> Shape {
        self.clone()
    }
}
impl GeometryOps for Polygon<f64> {
    fn as_shape(&self) -> Shape {
        MultiPolygon(vec![self.clone()])
    }
}
pub fn unary_union(shapes: &[Shape]) -> Shape {
    let subject = shapes
        .iter()
        .flat_map(|s| paths(&s.orient(Direction::Default)))
        .collect();
    execute(&subject, &vec![], ClipType::Union, FillRule::NonZero)
}
