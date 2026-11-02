use crate::overlay::GeometryOps;
use crate::{detector::Violation, engine::Report, geometry::Shape, parser::bounds};
use anyhow::{Context, Result};
use fontdue::{Font, FontSettings};
use geo::BoundingRect;
use rstar::{
    AABB, RTree,
    primitives::{GeomWithData, Rectangle},
};
use std::{fmt::Write as _, fs, path::Path, sync::OnceLock};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

const BACK: [u8; 4] = [17, 27, 43, 255];
const GOLD: [u8; 4] = [210, 164, 92, 255];
const RED: [u8; 4] = [255, 85, 119, 255];
const WHITE: [u8; 4] = [228, 234, 245, 255];
struct Renderer<'a> {
    copper: &'a Shape,
    tree: RTree<GeomWithData<Rectangle<[f64; 2]>, usize>>,
}
impl<'a> Renderer<'a> {
    fn new(copper: &'a Shape) -> Self {
        let tree = RTree::bulk_load(
            copper
                .0
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    p.bounding_rect().map(|b| {
                        GeomWithData::new(
                            Rectangle::from_corners([b.min().x, b.min().y], [b.max().x, b.max().y]),
                            i,
                        )
                    })
                })
                .collect(),
        );
        Self { copper, tree }
    }
}
fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p
}
fn line(image: &mut Pixmap, a: [f32; 2], b: [f32; 2], color: [u8; 4], width: f32) {
    let mut p = PathBuilder::new();
    p.move_to(a[0], a[1]);
    p.line_to(b[0], b[1]);
    if let Some(p) = p.finish() {
        image.stroke_path(
            &p,
            &paint(color),
            &Stroke {
                width,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}
fn text(image: &mut Pixmap, x: f32, y: f32, size: f32, value: &str, color: [u8; 4]) {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    let font = FONT.get_or_init(|| {
        [
            "C:/Windows/Fonts/consola.ttf",
            "C:/Windows/Fonts/arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/System/Library/Fonts/Menlo.ttc",
        ]
        .iter()
        .find_map(|p| {
            fs::read(p)
                .ok()
                .and_then(|v| Font::from_bytes(v, FontSettings::default()).ok())
        })
    });
    let Some(font) = font else {
        return;
    };
    let mut x = x;
    for ch in value.chars() {
        let (m, pixels) = font.rasterize(ch, size);
        for row in 0..m.height {
            for col in 0..m.width {
                let px = x as i32 + m.xmin + col as i32;
                let py = y as i32 + size as i32 - m.height as i32 - m.ymin + row as i32;
                if px < 0 || py < 0 || px >= image.width() as i32 || py >= image.height() as i32 {
                    continue;
                }
                let alpha = pixels[row * m.width + col] as f32 / 255.;
                let index = (py as usize * image.width() as usize + px as usize) * 4;
                for (k, c) in color[..3].iter().enumerate() {
                    image.data_mut()[index + k] = (*c as f32 * alpha
                        + image.data()[index + k] as f32 * (1. - alpha))
                        .round() as u8;
                }
            }
        }
        x += m.advance_width;
    }
}
fn render(
    renderer: &Renderer<'_>,
    target: &Path,
    title: &str,
    records: &[Violation],
    view: Option<[f64; 4]>,
    detail: bool,
) -> Result<()> {
    let mut image = Pixmap::new(1100, 900).context("画像を確保できません")?;
    image.fill(Color::from_rgba8(BACK[0], BACK[1], BACK[2], 255));
    text(&mut image, 32., 18., 23., title, WHITE);
    if detail {
        let d = &records[0];
        text(
            &mut image,
            32.,
            54.,
            22.,
            &format!("gap = {:.7} mm  ({:.3} um)", d.gap_mm, d.gap_mm * 1000.),
            RED,
        );
        text(
            &mut image,
            32.,
            88.,
            19.,
            &format!("Gerber X = {:.6}  Y = {:.6} mm", d.x_mm, d.y_mm),
            WHITE,
        );
    } else {
        text(
            &mut image,
            32.,
            58.,
            18.,
            &format!(
                "{} locations | copper: gold, measured gap: red",
                records.len()
            ),
            WHITE,
        );
    }
    let [mut x0, mut y0, mut x1, mut y1] = view.unwrap_or_else(|| bounds(renderer.copper));
    let margin = 0.04 * (x1 - x0).max(y1 - y0).max(0.1);
    x0 -= margin;
    y0 -= margin;
    x1 += margin;
    y1 += margin;
    let zoom = (1000. / (x1 - x0).max(0.1)).min(650. / (y1 - y0).max(0.1));
    let left = (1100. - (x1 - x0) * zoom) / 2.;
    let top = 140. + (650. - (y1 - y0) * zoom) / 2.;
    let pixel = |x: f64, y: f64| {
        [
            (left + (x - x0) * zoom) as f32,
            (top + (y1 - y) * zoom) as f32,
        ]
    };
    let window = crate::geometry::rectangle(x0, y0, x1, y1);
    let envelope = AABB::from_corners([x0, y0], [x1, y1]);
    for entry in renderer.tree.locate_in_envelope_intersecting(&envelope) {
        let poly = &renderer.copper.0[entry.data];
        let clipped;
        let parts = if detail {
            clipped = poly.intersection(&window);
            &clipped.0[..]
        } else {
            std::slice::from_ref(poly)
        };
        for poly in parts {
            let mut path = PathBuilder::new();
            for ring in std::iter::once(poly.exterior()).chain(poly.interiors()) {
                for (i, c) in ring.0.iter().enumerate() {
                    let [x, y] = pixel(c.x, c.y);
                    if i == 0 {
                        path.move_to(x, y);
                    } else {
                        path.line_to(x, y);
                    }
                }
                path.close();
            }
            if let Some(path) = path.finish() {
                image.fill_path(
                    &path,
                    &paint(GOLD),
                    FillRule::EvenOdd,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
    // 近接箇所の番号をずらし、画像の中で引き出し先を追えるようにする。
    let mut boxes = Vec::<[f32; 4]>::new();
    for r in records {
        let [px, py] = pixel(r.x_mm, r.y_mm);
        let radius = if detail { 18. } else { 12. };
        if let Some(circle) = PathBuilder::from_circle(px, py, radius) {
            image.stroke_path(
                &circle,
                &paint(RED),
                &Stroke {
                    width: 3.,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
        if detail {
            line(
                &mut image,
                pixel(r.point_a_mm[0], r.point_a_mm[1]),
                pixel(r.point_b_mm[0], r.point_b_mm[1]),
                RED,
                3.,
            );
        } else {
            line(&mut image, [px - 18., py], [px + 18., py], RED, 2.);
            line(&mut image, [px, py - 18.], [px, py + 18.], RED, 2.);
            let label = r.id.to_string();
            let width = label.len() as f32 * 13. + 16.;
            let mut chosen = [0.; 4];
            'candidate: for dy in [-38., 12., -74., 48., -110., 84.] {
                for dx in [24., -width - 24.] {
                    let lx = (px + dx).clamp(8., 1100. - width - 8.);
                    let ly = (py + dy).clamp(130., 760.);
                    chosen = [lx, ly, lx + width, ly + 30.];
                    if !boxes.iter().any(|b| {
                        chosen[0] < b[2] + 4.
                            && chosen[2] > b[0] - 4.
                            && chosen[1] < b[3] + 4.
                            && chosen[3] > b[1] - 4.
                    }) {
                        break 'candidate;
                    }
                }
            }
            boxes.push(chosen);
            line(
                &mut image,
                [px, py],
                [(chosen[0] + chosen[2]) / 2., chosen[1] + 15.],
                RED,
                2.,
            );
            if let Some(rect) = Rect::from_ltrb(chosen[0], chosen[1], chosen[2], chosen[3]) {
                image.fill_rect(rect, &paint(BACK), Transform::identity(), None);
                image.stroke_path(
                    &PathBuilder::from_rect(rect),
                    &paint(RED),
                    &Stroke {
                        width: 2.,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            text(
                &mut image,
                chosen[0] + 8.,
                chosen[1] + 2.,
                20.,
                &label,
                WHITE,
            );
        }
    }
    text(
        &mut image,
        32.,
        812.,
        18.,
        &format!("X: {x0:.4} .. {x1:.4} mm   Y: {y0:.4} .. {y1:.4} mm"),
        WHITE,
    );
    let bar = 10f64.powf((180. / zoom).log10().floor());
    line(
        &mut image,
        [40., 855.],
        [(40. + bar * zoom) as f32, 855.],
        WHITE,
        3.,
    );
    text(
        &mut image,
        (50. + bar * zoom) as f32,
        840.,
        16.,
        &format!("{bar} mm | +Y up | all layers viewed from top"),
        WHITE,
    );
    image
        .save_png(target)
        .with_context(|| format!("画像を保存できません: {}", target.display()))?;
    Ok(())
}
pub fn write_layer_images(
    copper: &Shape,
    records: &mut [Violation],
    directory: &Path,
    stem: &str,
    window_mm: f64,
    name: &str,
) -> Result<String> {
    let overview = format!("{stem}-overview.png");
    let renderer = Renderer::new(copper);
    for (i, r) in records.iter_mut().enumerate() {
        r.id = i + 1;
        r.image = format!("{stem}-{:04}.png", i + 1);
    }
    render(
        &renderer,
        &directory.join(&overview),
        name,
        records,
        None,
        false,
    )?;
    for r in records.iter() {
        let [x0, y0, x1, y1] = r.sample_bounds_mm;
        // 代表点が集約範囲の端でも、すべての検出点を拡大画像内へ収める。
        let half = (window_mm / 2.)
            .max((r.x_mm - x0).max(x1 - r.x_mm) + 0.4)
            .max((r.y_mm - y0).max(y1 - r.y_mm) + 0.4);
        render(
            &renderer,
            &directory.join(&r.image),
            &format!("{name}  #{}", r.id),
            std::slice::from_ref(r),
            Some([r.x_mm - half, r.y_mm - half, r.x_mm + half, r.y_mm + half]),
            true,
        )?;
    }
    Ok(overview)
}
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
pub fn save_reports(report: &Report, outdir: &Path) -> Result<()> {
    fs::write(
        outdir.join("report.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    let mut data = vec![0xef, 0xbb, 0xbf];
    {
        let mut csv = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(&mut data);
        csv.write_record([
            "layer",
            "id",
            "gap_mm",
            "x_mm",
            "y_mm",
            "kind",
            "at_threshold",
            "point_a_x_mm",
            "point_a_y_mm",
            "point_b_x_mm",
            "point_b_y_mm",
            "sample_count",
            "image",
        ])?;
        for l in &report.layers {
            for r in &l.violations {
                csv.write_record([
                    l.name.clone(),
                    r.id.to_string(),
                    r.gap_mm.to_string(),
                    r.x_mm.to_string(),
                    r.y_mm.to_string(),
                    r.kind.clone(),
                    if r.at_threshold { "True" } else { "False" }.into(),
                    r.point_a_mm[0].to_string(),
                    r.point_a_mm[1].to_string(),
                    r.point_b_mm[0].to_string(),
                    r.point_b_mm[1].to_string(),
                    r.sample_count.to_string(),
                    r.image.clone(),
                ])?;
            }
        }
        csv.flush()?;
    }
    fs::write(outdir.join("violations.csv"), data)?;
    let mut html = String::from(
        "<!doctype html><html lang='ja'><meta charset='utf-8'><meta name='viewport' content='width=device-width'><title>ガーバークリアランス検査</title><style>body{font:16px system-ui;max-width:1200px;margin:40px auto;padding:0 24px;background:#111b2b;color:#e4eaf5}a{color:#83c8ff}table{border-collapse:collapse;width:100%}td,th{padding:10px;border-bottom:1px solid #42516a;text-align:left}.table-scroll{overflow-x:auto}.preview{display:block;width:320px;max-width:none}figure{margin:20px 0}img{max-width:100%;height:auto}.error{color:#ff8199}code{overflow-wrap:anywhere}li{margin:8px 0}</style><h1>ガーバークリアランス検査</h1>",
    );
    write!(
        html,
        "<p>判定: {} mm 未満 / 検出 {} 箇所 / 検査エラー {} 層</p><p>座標: ガーバー原点基準、mm、X は右向き、Y は上向き。裏面も上面からの透視。</p>",
        report.threshold_mm, report.violation_count, report.error_count
    )?;
    for l in &report.layers {
        if l.violations.is_empty() && l.error.is_none() {
            continue;
        }
        write!(html, "<h2>{}</h2>", escape(&l.name))?;
        if let Some(error) = &l.error {
            write!(html, "<p class='error'>検査失敗: {}</p>", escape(error))?;
            continue;
        }
        for warning in &l.warnings {
            write!(html, "<p>{}</p>", escape(warning))?;
        }
        write!(
            html,
            "<p>{} 箇所 / コーナー除外 {} 線分ペア</p><figure><figcaption>層全体の画像 — 赤いマークと番号が違反箇所のおおよその位置です。</figcaption><a href='{}'><img src='{}' alt='{} の違反位置'></a></figure><div class='table-scroll'><table><tr><th>No.</th><th>ギャップ mm</th><th>X mm</th><th>Y mm</th><th>形状</th><th>違反箇所の画像</th></tr>",
            l.violations.len(),
            l.detection.corner_filtered_pairs,
            escape(&l.overview),
            escape(&l.overview),
            escape(&l.name)
        )?;
        for r in &l.violations {
            let kind = if r.kind == "self_approach" {
                "同一銅箔の自己接近"
            } else {
                "別の銅箔どうし"
            };
            write!(
                html,
                "<tr id='{}-{}'><td>{}</td><td>{:.7}</td><td>{:.6}</td><td>{:.6}</td><td>{kind}</td><td><a href='{}'><img class='preview' loading='lazy' src='{}' alt='違反箇所 {} の拡大画像'></a></td></tr>",
                l.key,
                r.id,
                r.id,
                r.gap_mm,
                r.x_mm,
                r.y_mm,
                escape(&r.image),
                escape(&r.image),
                r.id
            )?;
        }
        html.push_str("</table></div>");
    }
    html.push_str("<h2>入力の選択</h2><ul>");
    for s in &report.skipped_inputs {
        write!(
            html,
            "<li><code>{}</code>: {}</li>",
            escape(&s.path),
            escape(&s.reason)
        )?;
    }
    html.push_str("</ul><h2>補足</h2><p><a href='violations.csv'>座標一覧 CSV</a> · <a href='report.json'>機械処理用 JSON</a></p><ul>");
    for s in &report.limitations {
        write!(html, "<li>{}</li>", escape(s))?;
    }
    html.push_str("</ul></html>");
    fs::write(outdir.join("index.html"), html)?;
    Ok(())
}
