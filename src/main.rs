use clap::Parser;
use pcb_clearance::engine::{Options, Progress, run_check};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Altium / KiCad ガーバーの指定値未満の絶縁ギャップを座標・PNG付きで報告"
)]
struct Args {
    #[arg(
        required = true,
        help = "ガーバーファイル、フォルダー（下位階層を含む）、ZIP、またはワイルドカード"
    )]
    inputs: Vec<PathBuf>,
    #[arg(long = "min-clearance", alias = "threshold", default_value_t = 0.1)]
    threshold: f64,
    #[arg(long)]
    outdir: Option<PathBuf>,
    #[arg(long, short = 'j', default_value_t = 0)]
    jobs: usize,
    #[arg(long,default_value="auto",value_parser=["auto","positive","negative"])]
    polarity: String,
    #[arg(long)]
    no_arc_filter: bool,
    #[arg(long, default_value_t = 0.3)]
    cluster_radius: f64,
    #[arg(long, default_value_t = 2.)]
    window_mm: f64,
    #[arg(long, default_value_t = 1e-7)]
    tolerance: f64,
}
fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let options = Options {
        threshold: args.threshold,
        tolerance: args.tolerance,
        polarity: args.polarity,
        arc_filter: !args.no_arc_filter,
        cluster_radius: args.cluster_radius,
        window_mm: args.window_mm,
        jobs: args.jobs,
    };
    match run_check(
        &args.inputs,
        args.outdir.as_deref(),
        &options,
        |event| match event {
            Progress::Started { total, .. } => eprintln!("[開始] {total} 層"),
            Progress::LayerDone {
                layer,
                completed,
                total,
            } => {
                if let Some(error) = layer.error {
                    eprintln!("[エラー] {}: {error}", layer.name);
                } else {
                    eprintln!(
                        "[{completed}/{total}] {}: {} 箇所",
                        layer.name,
                        layer.violations.len()
                    );
                }
            }
            Progress::Report => eprintln!("[レポート] 作成中"),
        },
    ) {
        Ok((report, outdir)) => {
            println!(
                "検出 {} 箇所 / エラー {} 層 / 検査 {} 層",
                report.violation_count,
                report.error_count,
                report.layers.len()
            );
            println!("結果: {}", outdir.join("index.html").display());
            println!("JSON: {}", outdir.join("report.json").display());
            std::process::ExitCode::from(report.exit_code() as u8)
        }
        Err(e) => {
            eprintln!("入力・出力エラー: {e:#}");
            std::process::ExitCode::from(2)
        }
    }
}
