use crate::overlay::{GeometryOps, unary_union};
use anyhow::{Result, bail, ensure};
use geo::orient::Direction;
use geo::{Coord, LineString, MapCoords, MultiPolygon, Orient, Polygon};
use std::{
    collections::HashMap,
    f64::consts::{PI, TAU},
};

pub const QUAD_SEGS: usize = 64;
pub type Shape = MultiPolygon<f64>;
pub type XY = [f64; 2];

pub fn empty() -> Shape {
    MultiPolygon(vec![])
}
pub fn polygon(points: &[XY]) -> Polygon<f64> {
    Polygon::new(
        LineString::from(points.iter().map(|p| (p[0], p[1])).collect::<Vec<_>>()),
        vec![],
    )
}
pub fn rect(w: f64, h: f64) -> Shape {
    rectangle(-w / 2., -h / 2., w / 2., h / 2.)
}
pub fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> Shape {
    MultiPolygon(vec![polygon(&[
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y1],
        [x0, y0],
    ])])
}
pub fn regular_poly(d: f64, n: usize, angle: f64) -> Shape {
    MultiPolygon(vec![polygon(
        &(0..n)
            .map(|i| {
                let a = angle.to_radians() + TAU * i as f64 / n as f64;
                [d / 2. * a.cos(), d / 2. * a.sin()]
            })
            .collect::<Vec<_>>(),
    )])
}
pub fn circle(d: f64) -> Shape {
    if d <= 0. {
        return empty();
    }
    regular_poly(d, QUAD_SEGS * 4, 0.)
}
pub fn transform(shape: &Shape, sx: f64, sy: f64, angle: f64, dx: f64, dy: f64) -> Shape {
    let (s, c) = angle.to_radians().sin_cos();
    shape.map_coords(|p| Coord {
        x: p.x * sx * c - p.y * sy * s + dx,
        y: p.x * sx * s + p.y * sy * c + dy,
    })
}
pub fn translate(shape: &Shape, p: XY) -> Shape {
    transform(shape, 1., 1., 0., p[0], p[1])
}
pub fn stroke(path: &[XY], width: f64) -> Shape {
    let mut points = Vec::<XY>::new();
    for &p in path {
        if points
            .last()
            .is_none_or(|a| (a[0] - p[0]).hypot(a[1] - p[1]) > 1e-12)
        {
            points.push(p);
        }
    }
    if points.is_empty() {
        return empty();
    }
    if points.len() == 1 {
        return translate(&circle(width), points[0]);
    }
    let radius = width / 2.;
    let step = PI / (QUAD_SEGS * 2) as f64;
    let angles = points
        .windows(2)
        .map(|p| (p[1][1] - p[0][1]).atan2(p[1][0] - p[0][0]))
        .collect::<Vec<_>>();
    let mut parts = vec![];
    // 汎用バッファの分割数上限に依存せず、従来どおり1/4円64分割で掃引する。
    for (k, p) in points.windows(2).enumerate() {
        let angle = angles[k];
        let n = [-angle.sin() * radius, angle.cos() * radius];
        parts.push(
            MultiPolygon(vec![polygon(&[
                [p[0][0] + n[0], p[0][1] + n[1]],
                [p[1][0] + n[0], p[1][1] + n[1]],
                [p[1][0] - n[0], p[1][1] - n[1]],
                [p[0][0] - n[0], p[0][1] - n[1]],
            ])])
            .orient(Direction::Default),
        );
    }
    for k in 1..points.len() - 1 {
        let mut turn = (angles[k] - angles[k - 1] + PI).rem_euclid(TAU) - PI;
        if turn.abs() < 1e-12 {
            continue;
        }
        if turn.abs() > PI - 1e-9 {
            turn = PI;
        }
        let start = angles[k - 1] + if turn > 0. { -PI / 2. } else { PI / 2. };
        let n = (turn.abs() / step).round().max(1.) as usize;
        let p = points[k];
        let mut sector = vec![p];
        sector.extend((0..=n).map(|i| {
            let a = start + turn * i as f64 / n as f64;
            [p[0] + radius * a.cos(), p[1] + radius * a.sin()]
        }));
        parts.push(MultiPolygon(vec![polygon(&sector)]).orient(Direction::Default));
    }
    for (p, angle) in [
        (points[0], angles[0]),
        (*points.last().unwrap(), *angles.last().unwrap()),
    ] {
        parts.push(translate(
            &regular_poly(width, QUAD_SEGS * 4, angle.to_degrees()),
            p,
        ));
    }
    unary_union(&parts)
}
pub fn obround(w: f64, h: f64) -> Shape {
    let r = w.min(h) / 2.;
    let path = if w >= h {
        vec![[-w / 2. + r, 0.], [w / 2. - r, 0.]]
    } else {
        vec![[0., -h / 2. + r], [0., h / 2. - r]]
    };
    if w == h {
        circle(w)
    } else {
        stroke(&path, 2. * r)
    }
}

// マクロの式を再帰下降で評価し、任意コードの実行を避ける。
struct Expr<'a> {
    bytes: &'a [u8],
    pos: usize,
    params: &'a HashMap<usize, f64>,
}
impl Expr<'_> {
    fn ws(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn take(&mut self, c: u8) -> bool {
        self.ws();
        if self.bytes.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn sum(&mut self) -> Result<f64> {
        let mut n = self.product()?;
        loop {
            if self.take(b'+') {
                n += self.product()?;
            } else if self.take(b'-') {
                n -= self.product()?;
            } else {
                break;
            }
        }
        Ok(n)
    }
    fn product(&mut self) -> Result<f64> {
        let mut n = self.atom()?;
        loop {
            if self.take(b'x') || self.take(b'X') || self.take(b'*') {
                n *= self.atom()?;
            } else if self.take(b'/') {
                let d = self.atom()?;
                ensure!(d != 0., "AM のゼロ除算");
                n /= d;
            } else {
                break;
            }
        }
        Ok(n)
    }
    fn atom(&mut self) -> Result<f64> {
        if self.take(b'+') {
            return self.atom();
        }
        if self.take(b'-') {
            return Ok(-self.atom()?);
        }
        if self.take(b'(') {
            let n = self.sum()?;
            ensure!(self.take(b')'), "AM の括弧が閉じていない");
            return Ok(n);
        }
        if self.take(b'$') {
            let start = self.pos;
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            let k = std::str::from_utf8(&self.bytes[start..self.pos])?.parse::<usize>()?;
            return Ok(*self.params.get(&k).unwrap_or(&0.));
        }
        self.ws();
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_digit() || b == b'.' {
                self.pos += 1;
            } else if b == b'e' || b == b'E' {
                self.pos += 1;
                if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        ensure!(self.pos > start, "AM の式が解釈できない");
        Ok(std::str::from_utf8(&self.bytes[start..self.pos])?.parse()?)
    }
}
pub fn am_eval(expr: &str, params: &HashMap<usize, f64>) -> Result<f64> {
    let mut e = Expr {
        bytes: expr.as_bytes(),
        pos: 0,
        params,
    };
    let n = e.sum()?;
    e.ws();
    ensure!(
        e.pos == e.bytes.len() && n.is_finite(),
        "AM の式が不正: {expr}"
    );
    Ok(n)
}
pub fn macro_shape(name: &str, statements: &[String], args: &[f64]) -> Result<Shape> {
    let mut params: HashMap<_, _> = args.iter().enumerate().map(|(i, v)| (i + 1, *v)).collect();
    let mut shape = empty();
    for st in statements {
        let st = st.trim();
        if st.is_empty() || st.starts_with('0') {
            continue;
        }
        if let Some(rest) = st.strip_prefix('$') {
            let (k, e) = rest
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("AM の代入が不正"))?;
            let v = am_eval(e, &params)?;
            params.insert(k.trim().parse()?, v);
            continue;
        }
        let vals = st
            .split(',')
            .map(|v| am_eval(v, &params))
            .collect::<Result<Vec<_>>>()?;
        let code = vals[0] as i32;
        let a = &vals[1..];
        let required = match code {
            1 => 4,
            4 => 2,
            5 => 5,
            7 => 6,
            20 => 6,
            21 | 22 => 5,
            _ => bail!("未対応の AM プリミティブ: {code}"),
        };
        ensure!(a.len() >= required, "AM {name} の引数が足りない");
        let (exposure, g, rot) = match code {
            1 => (
                a[0],
                translate(&circle(a[1]), [a[2], a[3]]),
                *a.get(4).unwrap_or(&0.),
            ),
            4 => {
                let n = a[1] as usize;
                ensure!(
                    (3..=5000).contains(&n) && a.len() >= 2 + 2 * (n + 1),
                    "AM アウトラインが不正"
                );
                (
                    a[0],
                    MultiPolygon(vec![polygon(
                        &(0..=n)
                            .map(|k| [a[2 + 2 * k], a[3 + 2 * k]])
                            .collect::<Vec<_>>(),
                    )]),
                    *a.get(2 + 2 * (n + 1)).unwrap_or(&0.),
                )
            }
            5 => {
                ensure!((3. ..=5000.).contains(&a[1]), "AM 多角形の頂点数が不正");
                (
                    a[0],
                    translate(&regular_poly(a[4], a[1] as usize, 0.), [a[2], a[3]]),
                    *a.get(5).unwrap_or(&0.),
                )
            }
            7 => {
                let (cx, cy, outer, inner, gap, rot) = (a[0], a[1], a[2], a[3], a[4], a[5]);
                ensure!(
                    outer > inner && inner >= 0. && gap >= 0. && gap < outer / 2f64.sqrt(),
                    "サーマルの径またはギャップが不正"
                );
                let cross = rectangle(-outer, -gap / 2., outer, gap / 2.).union(&rectangle(
                    -gap / 2.,
                    -outer,
                    gap / 2.,
                    outer,
                ));
                (
                    1.,
                    translate(
                        &circle(outer).difference(&circle(inner)).difference(&cross),
                        [cx, cy],
                    ),
                    rot,
                )
            }
            20 => {
                let dx = a[4] - a[2];
                let dy = a[5] - a[3];
                let length = dx.hypot(dy);
                let g = if length > 0. {
                    transform(
                        &rect(length, a[1]),
                        1.,
                        1.,
                        dy.atan2(dx).to_degrees(),
                        (a[2] + a[4]) / 2.,
                        (a[3] + a[5]) / 2.,
                    )
                } else {
                    empty()
                };
                (a[0], g, *a.get(6).unwrap_or(&0.))
            }
            21 => (
                a[0],
                translate(&rect(a[1], a[2]), [a[3], a[4]]),
                *a.get(5).unwrap_or(&0.),
            ),
            22 => (
                a[0],
                rectangle(a[3], a[4], a[3] + a[1], a[4] + a[2]),
                *a.get(5).unwrap_or(&0.),
            ),
            _ => unreachable!(),
        };
        let g = transform(&g, 1., 1., rot, 0., 0.);
        shape = if exposure >= 0.5 {
            shape.union(&g)
        } else {
            shape.difference(&g)
        };
    }
    ensure!(!shape.0.is_empty(), "AM {name} から図形が得られない");
    Ok(shape)
}

pub fn arc_points(start: XY, end: XY, i: f64, j: f64, cw: bool, multi: bool) -> Vec<XY> {
    let centers = if multi {
        vec![[start[0] + i, start[1] + j]]
    } else {
        vec![
            [start[0] + i.abs(), start[1] + j.abs()],
            [start[0] + i.abs(), start[1] - j.abs()],
            [start[0] - i.abs(), start[1] + j.abs()],
            [start[0] - i.abs(), start[1] - j.abs()],
        ]
    };
    let mut best = None;
    let mut fallback = None;
    for [cx, cy] in centers {
        let r0 = (start[0] - cx).hypot(start[1] - cy);
        let r1 = (end[0] - cx).hypot(end[1] - cy);
        let a0 = (start[1] - cy).atan2(start[0] - cx);
        let a1 = (end[1] - cy).atan2(end[0] - cx);
        let mut sweep = a1 - a0;
        if cw {
            while sweep > 0. {
                sweep -= TAU;
            }
        } else {
            while sweep < 0. {
                sweep += TAU;
            }
        }
        if multi && sweep.abs() < 1e-12 {
            sweep = if cw { -TAU } else { TAU };
        }
        let c = ((r0 - r1).abs(), cx, cy, r0, r1, a0, sweep);
        if fallback.is_none_or(|v: (f64, f64, f64, f64, f64, f64, f64)| c.0 < v.0) {
            fallback = Some(c);
        }
        if !multi && sweep.abs() > PI / 2. + 1e-9 {
            continue;
        }
        if best.is_none_or(|v: (f64, f64, f64, f64, f64, f64, f64)| c.0 < v.0) {
            best = Some(c);
        }
    }
    let (_, cx, cy, r0, r1, a0, sweep) = best.or(fallback).unwrap();
    if r0 < 1e-12 || !sweep.is_finite() {
        return vec![start, end];
    }
    let n = ((sweep.abs() / (PI / 2.) * QUAD_SEGS as f64).ceil() as usize).max(2);
    let mut points = (0..=n)
        .map(|k| {
            let f = k as f64 / n as f64;
            let a = a0 + sweep * f;
            let r = r0 + (r1 - r0) * f;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect::<Vec<_>>();
    points[0] = start;
    points[n] = end;
    points
}
