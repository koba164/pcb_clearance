use crate::geometry::*;
use crate::overlay::{GeometryOps, unary_union};
use anyhow::{Result, bail, ensure};
use geo::orient::Direction;
use geo::{Area, BoundingRect, ConvexHull, MultiPolygon, Orient};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("正規表現の定義")
}
static META: LazyLock<Regex> = LazyLock::new(|| re(r"TF\.([A-Za-z]+),([^*%\r\n]*)"));
static FS: LazyLock<Regex> = LazyLock::new(|| re(r"%FS([LT])([AI])X(\d)(\d)Y(\d)(\d)\*%"));
static MO: LazyLock<Regex> = LazyLock::new(|| re(r"%MO(MM|IN)\*%"));
static TOKEN: LazyLock<Regex> = LazyLock::new(|| re(r"%[^%]*%|[^%*]+\*|[^\s]+"));
static ADD: LazyLock<Regex> = LazyLock::new(|| re(r"^ADD(\d+)([A-Za-z_$.][\w$.\-]*)(?:,(.*))?$"));
static COORD: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^(?:X([+-]?\d+))?(?:Y([+-]?\d+))?(?:I([+-]?\d+))?(?:J([+-]?\d+))?(?:D0?([123]))?$")
});

pub fn metadata(text: &str) -> BTreeMap<String, String> {
    META.captures_iter(text)
        .map(|c| (c[1].to_owned(), c[2].trim().to_owned()))
        .collect()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Statistics {
    pub flashes: usize,
    pub draws: usize,
    pub regions: usize,
    pub arcs: usize,
    pub apertures: usize,
    pub negative: bool,
    pub area_mm2: f64,
    pub bounds_mm: [f64; 4],
}
pub struct ParsedLayer {
    pub copper: Shape,
    pub metadata: BTreeMap<String, String>,
    pub statistics: Statistics,
    pub warnings: Vec<String>,
}
struct Aperture {
    shape: Shape,
    name: String,
    width: Option<f64>,
}
pub fn bounds(shape: &Shape) -> [f64; 4] {
    shape
        .bounding_rect()
        .map(|b| [b.min().x, b.min().y, b.max().x, b.max().y])
        .unwrap_or([0.; 4])
}
pub struct GerberParser<'a> {
    text: &'a str,
    polarity: &'a str,
    padding: f64,
    apertures: HashMap<usize, Aperture>,
    macros: HashMap<String, Vec<String>>,
    batch: Vec<Shape>,
    image: Shape,
    dark: bool,
    unit: f64,
    formats: [(usize, usize); 2],
    trailing: bool,
    stats: Statistics,
}
impl<'a> GerberParser<'a> {
    pub fn new(text: &'a str, polarity: &'a str, padding: f64) -> Self {
        Self {
            text,
            polarity,
            padding,
            apertures: HashMap::new(),
            macros: HashMap::new(),
            batch: vec![],
            image: empty(),
            dark: true,
            unit: 1.,
            formats: [(4, 4); 2],
            trailing: false,
            stats: Statistics::default(),
        }
    }
    fn flush(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        let combined = unary_union(&self.batch);
        self.image = if self.dark {
            self.image.union(&combined)
        } else {
            self.image.difference(&combined)
        };
        self.batch.clear();
    }
    fn emit(&mut self, shape: Shape) {
        self.batch.push(shape.orient(Direction::Default));
    }
    fn coord(&self, value: &str, axis: usize) -> Result<f64> {
        let sign = if value.starts_with('-') { -1. } else { 1. };
        let mut digits = value.trim_start_matches(['+', '-']).to_string();
        let (integer, decimal) = self.formats[axis];
        ensure!(digits.len() <= integer + decimal, "座標の桁数超過: {value}");
        if self.trailing {
            while digits.len() < integer + decimal {
                digits.push('0');
            }
        }
        Ok(sign * digits.parse::<f64>()? / 10f64.powi(decimal as i32) * self.unit)
    }
    fn aperture(&mut self, definition: &str) -> Result<()> {
        let c = ADD
            .captures(definition)
            .ok_or_else(|| anyhow::anyhow!("不正なアパーチャ定義: {definition}"))?;
        let num = c[1].parse()?;
        let name = &c[2];
        let args = if let Some(p) = c.get(3) {
            p.as_str()
                .split('X')
                .map(str::parse::<f64>)
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            vec![]
        };
        ensure!(
            args.iter().all(|v| v.is_finite()),
            "アパーチャ引数が有限値ではない"
        );
        let (mut shape, mut width) = if let Some(st) = self.macros.get(name) {
            (macro_shape(name, st, &args)?, None)
        } else {
            let required = match name {
                "C" => 1,
                "R" | "O" | "P" => 2,
                _ => bail!("未対応のアパーチャ: {name}"),
            };
            ensure!(
                args.len() >= required && args[0] > 0.,
                "アパーチャの引数が不正: {definition}"
            );
            let (shape, hole, width) = match name {
                "C" => (circle(args[0]), &args[1..], Some(args[0])),
                "R" | "O" => {
                    ensure!(args[1] > 0., "アパーチャ高さが不正");
                    (
                        if name == "R" {
                            rect(args[0], args[1])
                        } else {
                            obround(args[0], args[1])
                        },
                        &args[2..],
                        None,
                    )
                }
                "P" => {
                    ensure!((3. ..=5000.).contains(&args[1]), "多角形の頂点数が不正");
                    (
                        regular_poly(args[0], args[1] as usize, *args.get(2).unwrap_or(&0.)),
                        args.get(3..).unwrap_or(&[]),
                        None,
                    )
                }
                _ => unreachable!(),
            };
            ensure!(hole.len() <= 2, "アパーチャの穴の引数が多すぎる");
            if hole.first().is_some_and(|v| *v > 0.) {
                (
                    shape.difference(&if hole.len() == 1 {
                        circle(hole[0])
                    } else {
                        rect(hole[0], hole[1])
                    }),
                    None,
                )
            } else {
                (shape, width)
            }
        };
        shape = transform(&shape, self.unit, self.unit, 0., 0., 0.);
        width = width.map(|w| w * self.unit);
        self.apertures.insert(
            num,
            Aperture {
                shape,
                name: name.to_string(),
                width,
            },
        );
        Ok(())
    }
    pub fn parse(mut self) -> Result<ParsedLayer> {
        let fs = FS.captures_iter(self.text).collect::<Vec<_>>();
        let units = MO.captures_iter(self.text).collect::<Vec<_>>();
        ensure!(
            fs.len() == 1 && &fs[0][2] == "A" && units.len() == 1,
            "FS の絶対座標指定および単一の MO 単位指定が必要"
        );
        ensure!(
            ["auto", "positive", "negative"].contains(&self.polarity),
            "極性指定が不正"
        );
        self.trailing = &fs[0][1] == "T";
        self.formats = [
            (fs[0][3].parse()?, fs[0][4].parse()?),
            (fs[0][5].parse()?, fs[0][6].parse()?),
        ];
        self.unit = if &units[0][1] == "IN" { 25.4 } else { 1. };
        let attrs = metadata(self.text);
        let mut position = [0., 0.];
        let mut aperture = None;
        let mut operation = None;
        let (mut interpolation, mut multi) = (1, true);
        let (mut region, mut contour, mut contours) =
            (false, Vec::<XY>::new(), Vec::<Shape>::new());
        let (mut mirror, mut angle, mut factor) = ("N".to_string(), 0., 1.);
        let (mut image_negative, mut ended) = (false, false);
        for token in TOKEN.find_iter(self.text) {
            let raw = token.as_str().trim().trim_start_matches('\u{feff}');
            if raw.is_empty() {
                continue;
            }
            ensure!(!ended, "M02 の後にコマンドがある");
            if raw.starts_with('%') {
                ensure!(raw.ends_with('%'), "拡張コマンドの終端がない");
                let body = raw[1..raw.len() - 1].trim().trim_end_matches('*');
                if let Some(rest) = body.strip_prefix("AM") {
                    let mut st = rest.split('*');
                    let name = st.next().unwrap().to_string();
                    self.macros.insert(name, st.map(str::to_string).collect());
                    continue;
                }
                for command in body.split('*').map(str::trim) {
                    if command.starts_with("ADD") {
                        self.aperture(command)?;
                    } else if ["FS", "MO", "TF", "TA", "TO", "TD", "IN", "LN"]
                        .iter()
                        .any(|s| command.starts_with(s))
                    {
                    } else if command == "LPD" || command == "LPC" {
                        ensure!(!region, "リージョン中の極性変更");
                        self.flush();
                        self.dark = command == "LPD";
                    } else if let Some(v) = command.strip_prefix("LM") {
                        ensure!(["N", "X", "Y", "XY"].contains(&v), "LM が不正");
                        mirror = v.to_string();
                    } else if let Some(v) = command.strip_prefix("LR") {
                        angle = v.parse::<f64>()?;
                        ensure!(angle.is_finite(), "LR が不正");
                    } else if let Some(v) = command.strip_prefix("LS") {
                        factor = v.parse::<f64>()?;
                        ensure!(factor.is_finite() && factor > 0., "LS は有限の正数が必要");
                    } else if command == "IPPOS" || command == "IPNEG" {
                        image_negative = command == "IPNEG";
                    } else {
                        bail!("未対応の拡張コマンド: {command}");
                    }
                }
                continue;
            }
            ensure!(raw.ends_with('*'), "終端 * のないコマンド: {raw}");
            let mut body = raw[..raw.len() - 1].trim();
            if body.starts_with("G04") {
                continue;
            }
            match body {
                "M02" | "M00" => {
                    ended = true;
                    continue;
                }
                "G70" | "G71" | "G90" => {
                    ensure!(
                        body == "G90" || (body == "G70") == (self.unit == 25.4),
                        "G70/G71 と MO の単位が矛盾"
                    );
                    continue;
                }
                "G36" => {
                    ensure!(!region, "リージョンの入れ子");
                    region = true;
                    contour.clear();
                    contours.clear();
                    continue;
                }
                "G37" => {
                    ensure!(region, "G36 のない G37");
                    finish_contour(&mut contour, &mut contours)?;
                    self.emit(unary_union(&contours));
                    self.stats.regions += 1;
                    region = false;
                    continue;
                }
                _ => {}
            }
            // G コードは数値全体を読んでから判定し、G12 を G1 と誤認しない。
            while let Some(rest) = body.strip_prefix('G') {
                let n = rest.bytes().take_while(u8::is_ascii_digit).count();
                ensure!(n > 0, "G コードが不正");
                let code = rest[..n].parse::<usize>()?;
                body = &rest[n..];
                match code {
                    1..=3 => interpolation = code,
                    74 => multi = false,
                    75 => multi = true,
                    54 => {}
                    _ => bail!("未対応の G コード: G{code}"),
                }
            }
            if body.is_empty() {
                continue;
            }
            if let Some(v) = body
                .strip_prefix('D')
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|v| *v >= 10)
            {
                ensure!(self.apertures.contains_key(&v), "未定義の D{v}");
                aperture = Some(v);
                continue;
            }
            let c = COORD
                .captures(body)
                .ok_or_else(|| anyhow::anyhow!("未対応のコマンド: {body}"))?;
            let next = [
                if let Some(v) = c.get(1) {
                    self.coord(v.as_str(), 0)?
                } else {
                    position[0]
                },
                if let Some(v) = c.get(2) {
                    self.coord(v.as_str(), 1)?
                } else {
                    position[1]
                },
            ];
            if let Some(v) = c.get(5) {
                operation = Some(v.as_str().parse::<usize>()?);
            }
            let op = operation.ok_or_else(|| anyhow::anyhow!("描画操作 D01/D02/D03 が未指定"))?;
            let mut path = vec![];
            if op == 1 {
                if interpolation == 2 || interpolation == 3 {
                    let io = c
                        .get(3)
                        .map(|v| self.coord(v.as_str(), 0))
                        .transpose()?
                        .unwrap_or(0.);
                    let jo = c
                        .get(4)
                        .map(|v| self.coord(v.as_str(), 1))
                        .transpose()?
                        .unwrap_or(0.);
                    ensure!(io != 0. || jo != 0., "円弧の I/J 中心がない");
                    path = arc_points(position, next, io, jo, interpolation == 2, multi);
                    self.stats.arcs += 1;
                } else {
                    path = vec![position, next];
                }
            }
            if region {
                match op {
                    2 => {
                        finish_contour(&mut contour, &mut contours)?;
                        contour.push(next);
                    }
                    1 => {
                        let skip = usize::from(!contour.is_empty());
                        contour.extend_from_slice(&path[skip..]);
                    }
                    _ => bail!("リージョン中のフラッシュ"),
                }
            } else if op == 1 || op == 3 {
                let ap = aperture
                    .and_then(|n| self.apertures.get(&n))
                    .ok_or_else(|| anyhow::anyhow!("描画アパーチャが未選択"))?;
                let shape = transform(
                    &ap.shape,
                    if mirror.contains('X') {
                        -factor
                    } else {
                        factor
                    },
                    if mirror.contains('Y') {
                        -factor
                    } else {
                        factor
                    },
                    angle,
                    0.,
                    0.,
                );
                let result = if op == 3 {
                    self.stats.flashes += 1;
                    translate(&shape, next)
                } else {
                    let result = if let Some(w) = ap.width {
                        stroke(&path, w * factor)
                    } else if ap.name == "R"
                        && shape.0.len() == 1
                        && shape.0[0].interiors().is_empty()
                    {
                        let parts = path
                            .windows(2)
                            .map(|p| {
                                let mut ends = translate(&shape, p[0]);
                                ends.0.extend(translate(&shape, p[1]).0);
                                MultiPolygon(vec![ends.convex_hull()])
                            })
                            .collect::<Vec<_>>();
                        unary_union(&parts)
                    } else {
                        bail!("未対応のストロークアパーチャ ({})", ap.name);
                    };
                    self.stats.draws += 1;
                    result
                };
                self.emit(result);
            }
            position = next;
        }
        ensure!(
            !region && ended,
            "リージョンまたはファイルが終端されていない"
        );
        self.flush();
        ensure!(!self.image.0.is_empty(), "面形状が得られない");
        let mut negative =
            image_negative != attrs.get("FilePolarity").is_some_and(|v| v == "Negative");
        if self.polarity != "auto" {
            negative = self.polarity == "negative";
        }
        let mut warnings = vec![];
        if negative {
            let [x0, y0, x1, y1] = bounds(&self.image);
            let p = self.padding;
            self.image = rectangle(x0 - p, y0 - p, x1 + p, y1 + p).difference(&self.image);
            warnings
                .push("ネガ層の外側は無限銅箔として解釈。基板外形との距離は検査対象外。".into());
        }
        self.stats.apertures = self.apertures.len();
        self.stats.negative = negative;
        self.stats.area_mm2 = self.image.unsigned_area();
        self.stats.bounds_mm = bounds(&self.image);
        Ok(ParsedLayer {
            copper: self.image,
            metadata: attrs,
            statistics: self.stats,
            warnings,
        })
    }
}
fn finish_contour(contour: &mut Vec<XY>, contours: &mut Vec<Shape>) -> Result<()> {
    if contour.len() > 1 {
        let a = contour[0];
        let b = *contour.last().unwrap();
        ensure!(
            contour.len() >= 4 && (a[0] - b[0]).hypot(a[1] - b[1]) <= 1e-7,
            "リージョンの輪郭が閉じていない"
        );
        // 自己接触する cut-in 輪郭をブール演算で面へ復元する。
        contours.push(
            MultiPolygon(vec![polygon(contour)])
                .union(&empty())
                .orient(Direction::Default),
        );
    }
    contour.clear();
    Ok(())
}
