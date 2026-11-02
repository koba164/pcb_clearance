pub use crate::input::{InputFile, Skipped, absolute, classify, discover};
use crate::{
    VERSION,
    detector::{Detection, Violation, find_gaps},
    parser::{GerberParser, Statistics, metadata},
    report,
};
use anyhow::{Result, ensure};
use chrono::Local;
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, mpsc},
    time::Instant,
};

pub const LIMITATIONS: &[&str] = &[
    "ネット情報は使わず、最終銅箔形状の絶縁ギャップを測定します。既に重なる銅箔の短絡や線幅は検査しません。",
    "円・円弧は1/4円64分割の近似値です。形状演算は固定格子 0.0000000001 mm、絶対座標 ±100000 mm 以内です。しきい値近傍は近似誤差の影響を受けます。数値比較の許容差は JSON に記録します。",
    "鋭角コーナー除外はヒューリスティックです。短いスリットが除外される可能性があります。--no-arc-filter で除外を無効化できます。",
    "近傍の線分ペアを島ペア別にまとめ、最小ギャップの実測点を代表表示します。集約半径は JSON に記録します。",
    "ガーバーの原点をそのまま使います。CAD 座標への自動変換は行いません。",
];
#[derive(Clone, Debug)]
pub struct Options {
    pub threshold: f64,
    pub tolerance: f64,
    pub polarity: String,
    pub arc_filter: bool,
    pub cluster_radius: f64,
    pub window_mm: f64,
    pub jobs: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            threshold: 0.1,
            tolerance: 1e-7,
            polarity: "auto".into(),
            arc_filter: true,
            cluster_radius: 0.3,
            window_mm: 2.,
            jobs: 0,
        }
    }
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        for (name, n) in [
            ("クリアランス", self.threshold),
            ("許容差", self.tolerance),
            ("集約半径", self.cluster_radius),
            ("画像範囲", self.window_mm),
        ] {
            ensure!(
                n.is_finite() && n > 0.,
                "{name}は有限の正数を指定してください"
            );
        }
        ensure!(
            self.tolerance < self.threshold,
            "許容差はクリアランスより小さい値を指定してください"
        );
        ensure!(
            ["auto", "positive", "negative"].contains(&self.polarity.as_str()),
            "極性の指定が不正です"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerResult {
    pub name: String,
    pub path: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statistics: Option<Statistics>,
    pub metadata: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    pub detection: Detection,
    pub violations: Vec<Violation>,
    pub overview: String,
    pub elapsed_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: usize,
    pub application_version: String,
    pub created_at: String,
    pub threshold_mm: f64,
    pub comparison: String,
    pub tolerance_mm: f64,
    pub coordinate_system: String,
    pub cluster_radius_mm: f64,
    pub arc_filter: bool,
    pub violation_count: usize,
    pub error_count: usize,
    pub limitations: Vec<String>,
    pub skipped_inputs: Vec<Skipped>,
    pub layers: Vec<LayerResult>,
    pub status: String,
}
impl Report {
    pub fn exit_code(&self) -> i32 {
        if self.error_count > 0 {
            2
        } else {
            i32::from(self.violation_count > 0)
        }
    }
}
#[derive(Clone, Debug)]
pub enum Progress {
    Started {
        total: usize,
        outdir: PathBuf,
    },
    LayerDone {
        total: usize,
        completed: usize,
        layer: Box<LayerResult>,
    },
    Report,
}
static GP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^gp\d+$").unwrap());
pub fn default_output_root() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output")
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("results")
    }
}
pub fn dated_output(root: &Path) -> PathBuf {
    root.join(
        Local::now()
            .format("clearance-%Y%m%d-%H%M%S-%6f")
            .to_string(),
    )
}
fn worker(path: &InputFile, options: &Options, outdir: &Path) -> LayerResult {
    let started = Instant::now();
    let name = path.display_name();
    let clean = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let hash = format!("{:x}", Sha256::digest(path.identity().as_bytes()));
    let key = format!("{clean}-{}", &hash[..8]);
    let mut result = LayerResult {
        name,
        path: path.display().to_string(),
        key,
        error: None,
        statistics: None,
        metadata: BTreeMap::new(),
        warnings: vec![],
        detection: Detection::default(),
        violations: vec![],
        overview: String::new(),
        elapsed_seconds: 0.,
    };
    let run = || -> Result<_> {
        let text = String::from_utf8(path.read_bytes()?)?
            .trim_start_matches('\u{feff}')
            .to_string();
        let mut polarity = options.polarity.as_str();
        let mut extra = vec![];
        if polarity == "auto"
            && path
                .extension()
                .is_some_and(|e| GP.is_match(&e.to_string_lossy()))
            && !metadata(&text).contains_key("FilePolarity")
            && !text.contains("%IP")
        {
            polarity = "negative";
            extra.push("極性属性のない Altium GP 層をネガとして扱いました。必要なら --polarity positive で指定してください。".into());
        }
        let mut layer =
            GerberParser::new(&text, polarity, (options.threshold * 10.).max(1.)).parse()?;
        let (mut records, detection) = find_gaps(
            &layer.copper,
            options.threshold,
            options.tolerance,
            options.arc_filter,
            layer.statistics.negative,
            options.cluster_radius,
        );
        let overview = report::write_layer_images(
            &layer.copper,
            &mut records,
            outdir,
            &result.key,
            options.window_mm,
            &result.name,
        )?;
        layer.warnings.extend(extra);
        Ok((layer, records, detection, overview))
    };
    // 不正形状に起因するライブラリのパニックも、その層の検査エラーとして記録する。
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(Ok((layer, records, detection, overview))) => {
            result.metadata = layer.metadata;
            result.statistics = Some(layer.statistics);
            result.warnings = layer.warnings;
            result.violations = records;
            result.detection = detection;
            result.overview = overview;
        }
        Ok(Err(e)) => result.error = Some(format!("{e:#}")),
        Err(e) => {
            result.error = Some(format!(
                "形状処理に失敗しました: {}",
                e.downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| e.downcast_ref::<&str>().copied())
                    .unwrap_or("パニック")
            ))
        }
    }
    result.elapsed_seconds = (started.elapsed().as_secs_f64() * 1000.).round() / 1000.;
    result
}
pub fn run_check(
    inputs: &[PathBuf],
    outdir: Option<&Path>,
    options: &Options,
    on_progress: impl FnMut(Progress),
) -> Result<(Report, PathBuf)> {
    options.validate()?;
    let (files, skipped) = discover(inputs)?;
    run_prepared(files, skipped, outdir, options, on_progress)
}
pub fn run_check_selected(
    files: &[InputFile],
    outdir: Option<&Path>,
    options: &Options,
    on_progress: impl FnMut(Progress),
) -> Result<(Report, PathBuf)> {
    options.validate()?;
    ensure!(!files.is_empty(), "検査対象が選択されていません");
    let files = files
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    run_prepared(files, vec![], outdir, options, on_progress)
}
fn run_prepared(
    files: Vec<InputFile>,
    skipped: Vec<Skipped>,
    outdir: Option<&Path>,
    options: &Options,
    mut on_progress: impl FnMut(Progress),
) -> Result<(Report, PathBuf)> {
    let outdir = outdir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dated_output(Path::new("output")));
    if outdir.exists() {
        ensure!(
            outdir.is_dir() && fs::read_dir(&outdir)?.next().is_none(),
            "出力先には空のフォルダーを指定してください: {}",
            outdir.display()
        );
    }
    fs::create_dir_all(&outdir)?;
    let outdir = absolute(&outdir)?;
    let jobs = files.len().min(if options.jobs == 0 {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(4)
    } else {
        options.jobs
    });
    on_progress(Progress::Started {
        total: files.len(),
        outdir: outdir.clone(),
    });
    let mut results = Vec::with_capacity(files.len());
    if jobs == 1 {
        for path in &files {
            let layer = worker(path, options, &outdir);
            results.push(layer.clone());
            on_progress(Progress::LayerDone {
                total: files.len(),
                completed: results.len(),
                layer: Box::new(layer),
            });
        }
    } else {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
        let (tx, rx) = mpsc::channel();
        let mut order = (0..files.len()).collect::<Vec<_>>();
        order.sort_by_cached_key(|&i| std::cmp::Reverse(files[i].size()));
        let mut indexed = BTreeMap::new();
        std::thread::scope(|scope| {
            let files = &files;
            let target = &outdir;
            scope.spawn(move || {
                pool.install(|| {
                    order.par_iter().for_each_with(tx, |tx, &i| {
                        let _ = tx.send((i, worker(&files[i], options, target)));
                    })
                })
            });
            for (i, layer) in rx {
                indexed.insert(i, layer.clone());
                on_progress(Progress::LayerDone {
                    total: files.len(),
                    completed: indexed.len(),
                    layer: Box::new(layer),
                });
            }
        });
        results = indexed.into_values().collect();
    }
    on_progress(Progress::Report);
    let violation_count = results.iter().map(|r| r.violations.len()).sum();
    let error_count = results.iter().filter(|r| r.error.is_some()).count();
    let report = Report {
        schema_version: 1,
        application_version: VERSION.into(),
        created_at: Local::now().to_rfc3339(),
        threshold_mm: options.threshold,
        comparison: "gap_mm < threshold_mm - tolerance_mm".into(),
        tolerance_mm: options.tolerance,
        coordinate_system: "Gerber, mm, X right, Y up".into(),
        cluster_radius_mm: options.cluster_radius,
        arc_filter: options.arc_filter,
        violation_count,
        error_count,
        limitations: LIMITATIONS.iter().map(|s| s.to_string()).collect(),
        skipped_inputs: skipped,
        layers: results,
        status: if error_count > 0 {
            "error"
        } else if violation_count > 0 {
            "violations"
        } else {
            "clear"
        }
        .into(),
    };
    report::save_reports(&report, &outdir)?;
    Ok((report, outdir))
}
