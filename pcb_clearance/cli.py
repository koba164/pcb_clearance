"""エージェントを必要としない一括検査 CLI。"""
import argparse
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import datetime
import glob
import hashlib
import math
import os
from pathlib import Path
import re
import sys
import time

from . import __version__
from .detector import find_gaps
from .parser import GerberError, GerberParser, metadata
from .report import save_reports, write_layer_images


LIMITATIONS = [
    "ネット情報は使わず、最終銅箔形状の絶縁ギャップを測定します。既に重なる銅箔の短絡や線幅は検査しません。",
    "円・円弧は1/4円64分割の近似値です。しきい値近傍は近似誤差の影響を受けます。数値比較の許容差は JSON に記録します。",
    "鋭角コーナー除外は既存スキルと同じヒューリスティックです。短いスリットが除外される可能性があります。--no-arc-filter で除外を無効化できます。",
    "近傍の線分ペアを島ペア別にまとめ、最小ギャップの実測点を代表表示します。集約半径は JSON に記録します。",
    "ガーバーの原点をそのまま使います。Altium の Board_Origin コメントや KiCad 原点から CAD 座標への自動変換は行いません。",
]


def classify(path, text):
    attrs = metadata(text)
    function = attrs.get("FileFunction")
    if function:
        return (function.split(",")[0] == "Copper", f"X2 FileFunction={function}")
    name = path.name.lower()
    if re.search(r"\.(gtl|gbl|g\d+|gp\d+)$", name) or re.search(r"(?:^|[._-])(?:f|b|in\d+)[._-]cu(?:[._-]|$)", name):
        return True, "銅箔のファイル名"
    if re.search(r"\.(gts|gbs|gto|gbo|gtp|gbp|gd\d+|gg\d+|gm\d+|drl|txt|zip|gko)$", name):
        return False, "レジスト・シルク・ペースト・ドリル・機械層など"
    return None, "銅箔層を自動判別できない名前（必要ならファイルを直接指定）"


def discover(inputs):
    selected, skipped = {}, []
    for value in inputs:
        literal = Path(value)
        hits = [literal] if literal.exists() else [Path(p) for p in sorted(glob.glob(value))]
        if not hits:
            raise GerberError(f"入力が見つからない: {value}")
        for hit in hits:
            directory = hit.is_dir()
            paths = sorted(hit.iterdir()) if directory else [hit]
            for path in paths:
                if not path.is_file():
                    continue
                # バイナリや非ガーバーも一覧には残すが、検査対象にはしない。
                text = path.read_bytes().decode("utf-8-sig", errors="replace")
                copper, reason = classify(path, text)
                if copper is True:
                    # 既知の銅箔ファイルは破損していても黙って検査対象から外さない。
                    selected[str(path.resolve())] = path.resolve()
                    continue
                if not re.search(r"%FS", text):
                    if not directory and len(hits) == 1:
                        raise GerberError(f"RS-274X の FS 指定がない: {path}")
                    skipped.append({"path": str(path.resolve()), "reason": "RS-274X ではない"})
                    continue
                if copper is True or (copper is None and not directory):
                    selected[str(path.resolve())] = path.resolve()
                else:
                    skipped.append({"path": str(path.resolve()), "reason": reason})
    if not selected:
        raise GerberError("銅箔ガーバーが見つからない。名前の不明な層はファイルを直接指定してください")
    return sorted(selected.values()), skipped


def log(message):
    """コンソールのない exe でも実行できるようにする。"""
    if sys.stderr is not None:
        print(message, file=sys.stderr, flush=True)


def worker(payload):
    path, options, outdir = payload
    started = time.monotonic()
    key = re.sub(r"[^A-Za-z0-9._-]", "_", path.name) + "-" + hashlib.sha256(str(path).encode()).hexdigest()[:8]
    result = {"name": path.name, "path": str(path), "key": key}
    try:
        log(f"[読込] {path.name}")
        text = path.read_text(encoding="utf-8-sig")
        polarity = options["polarity"]
        warnings = []
        if (polarity == "auto" and re.fullmatch(r"\.gp\d+", path.suffix.lower())
                and "FilePolarity" not in metadata(text) and "%IP" not in text):
            polarity = "negative"
            warnings.append("極性属性のない Altium GP 層をネガとして扱いました。必要なら --polarity positive で指定してください。")
        layer = GerberParser(text, polarity=polarity,
                             padding=max(1.0, options["threshold"] * 10)).parse()
        log(f"[測定] {path.name}")
        records, detection = find_gaps(layer.copper, options["threshold"], options["tolerance"],
                                      options["arc_filter"], layer.statistics["negative"], options["cluster_radius"])
        for number, rec in enumerate(records, 1):
            rec["id"] = number
        log(f"[画像] {path.name}: {len(records)} 箇所")
        overview = write_layer_images(layer.copper, records, outdir, key, options["window_mm"], path.name)
        result.update(metadata=layer.metadata, statistics=layer.statistics, warnings=layer.warnings + warnings,
                      detection=detection, violations=records, overview=overview)
    except (Exception, SystemExit) as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
        log(f"[エラー] {path.name}: {result['error']}")
    result["elapsed_seconds"] = round(time.monotonic() - started, 3)
    return result


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("有限の正数を指定してください")
    return number


def run_check(inputs, outdir=None, *, threshold=0.1, tolerance=1e-7, polarity="auto",
              arc_filter=True, cluster_radius=0.3, window_mm=2.0, jobs=0, on_progress=None):
    """CLI と GUI が共有する検査処理。進捗通知は呼び出し元のスレッドで行う。"""
    for name, number in (("クリアランス", threshold), ("許容差", tolerance),
                         ("集約半径", cluster_radius), ("画像範囲", window_mm)):
        if not math.isfinite(number) or number <= 0:
            raise GerberError(f"{name}は有限の正数を指定してください")
    if jobs < 0 or polarity not in ("auto", "positive", "negative"):
        raise GerberError("並列数または極性の指定が不正です")
    if tolerance >= threshold:
        raise GerberError("許容差はクリアランスより小さい値を指定してください")
    files, skipped = discover(inputs)
    outdir = (Path(outdir) if outdir is not None else
              Path("output") / datetime.now().strftime("clearance-%Y%m%d-%H%M%S-%f")).resolve()
    if outdir.exists() and (not outdir.is_dir() or any(outdir.iterdir())):
        raise GerberError(f"出力先には空のフォルダーを指定してください: {outdir}")
    outdir.mkdir(parents=True, exist_ok=True)
    options = dict(threshold=threshold, tolerance=tolerance, polarity=polarity,
                   arc_filter=arc_filter, cluster_radius=cluster_radius, window_mm=window_mm)
    jobs = min(len(files), jobs or min(4, os.cpu_count() or 1))

    def progress(stage, completed=0, layer=None):
        if on_progress:
            on_progress(dict(stage=stage, total=len(files), completed=completed,
                             layer=layer, outdir=str(outdir)))

    progress("started")
    if jobs == 1:
        results = []
        for path in files:
            results.append(worker((path, options, outdir)))
            progress("layer_done", len(results), results[-1])
    else:
        indexed = {}
        with ProcessPoolExecutor(max_workers=jobs) as pool:
            futures = {pool.submit(worker, (path, options, outdir)): path for path in
                       sorted(files, key=lambda p: -p.stat().st_size)}
            for future in as_completed(futures):
                path = futures[future]
                try:
                    indexed[path] = future.result()
                except Exception as exc:
                    indexed[path] = {"name": path.name, "path": str(path), "error": str(exc)}
                progress("layer_done", len(indexed), indexed[path])
        results = [indexed[path] for path in files]
    progress("report", len(files))
    report = dict(schema_version=1, application_version=__version__,
                  created_at=datetime.now().astimezone().isoformat(),
                  threshold_mm=threshold, comparison="gap_mm < threshold_mm - tolerance_mm",
                  tolerance_mm=tolerance, coordinate_system="Gerber, mm, X right, Y up",
                  cluster_radius_mm=cluster_radius, arc_filter=arc_filter,
                  violation_count=sum(len(r.get("violations", [])) for r in results),
                  error_count=sum("error" in r for r in results),
                  limitations=LIMITATIONS, skipped_inputs=skipped, layers=results)
    report["status"] = "error" if report["error_count"] else ("violations" if report["violation_count"] else "clear")
    save_reports(report, outdir)
    return report, outdir


def main(argv=None):
    parser = argparse.ArgumentParser(description="Altium / KiCad ガーバーの指定値未満の絶縁ギャップを座標・PNG付きで報告")
    parser.add_argument("inputs", nargs="+", help="ガーバーフォルダー、ファイル、またはワイルドカード")
    parser.add_argument("--min-clearance", "--threshold", type=positive, default=0.1, help="この値未満のギャップを検出 mm（同値を除く、既定0.1）")
    parser.add_argument("--outdir", type=Path, help="結果用の新規または空のフォルダー")
    parser.add_argument("--jobs", "-j", type=int, default=0, help="並列層数（既定は最大4、1で逐次）")
    parser.add_argument("--polarity", choices=["auto", "positive", "negative"], default="auto")
    parser.add_argument("--no-arc-filter", action="store_true", help="鋭角コーナー除外を無効化")
    parser.add_argument("--cluster-radius", type=positive, default=0.3, help="検出点を集約する半径 mm（既定0.3）")
    parser.add_argument("--window-mm", type=positive, default=2.0, help="拡大画像の範囲 mm（既定2）")
    parser.add_argument("--tolerance", type=positive, default=1e-7, help="しきい値と同値とみなす数値許容差 mm（既定0.0000001）")
    parser.add_argument("--version", action="version", version=__version__)
    args = parser.parse_args(argv)
    if args.jobs < 0:
        parser.error("--jobs は0以上が必要")
    try:
        report, outdir = run_check(args.inputs, args.outdir, threshold=args.min_clearance,
                                  tolerance=args.tolerance, polarity=args.polarity,
                                  arc_filter=not args.no_arc_filter, cluster_radius=args.cluster_radius,
                                  window_mm=args.window_mm, jobs=args.jobs)
        print(f"検出 {report['violation_count']} 箇所 / エラー {report['error_count']} 層 / 検査 {len(report['layers'])} 層")
        print(f"結果: {outdir / 'index.html'}")
        print(f"JSON: {outdir / 'report.json'}")
        return 2 if report["error_count"] else (1 if report["violation_count"] else 0)
    except (OSError, GerberError) as exc:
        print(f"入力・出力エラー: {exc}", file=sys.stderr)
        return 2
