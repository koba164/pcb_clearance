use geo::{Area, Intersects, MultiPolygon, Point};
use pcb_clearance::overlay::GeometryOps;
use pcb_clearance::{
    detector::find_gaps,
    engine::{Options, Progress, discover, run_check},
    geometry::*,
    parser::{GerberParser, ParsedLayer, bounds},
};
use std::{fs, path::PathBuf};

fn gerber(body: &str, defs: &str, unit: &str, format: &str, attrs: &str) -> String {
    format!("%FS{format}*%\n%MO{unit}*%\n{attrs}\n{defs}\nG01*\nD10*\n{body}\nM02*\n")
}
fn standard(body: &str) -> String {
    gerber(body, "%ADD10R,1X1*%", "MM", "LAX44Y44", "")
}
fn parse(text: &str) -> ParsedLayer {
    GerberParser::new(text, "auto", 1.).parse().unwrap()
}
fn covers(shape: &Shape, x: f64, y: f64) -> bool {
    shape.intersects(&Point::new(x, y))
}

#[test]
fn previous_engine_compatibility_fixtures() {
    // 移植前の実測値を保存し、斜め配線や円弧・マクロの幾何誤差を検証する。
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/compatibility.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let layer = parse(case["gerber"].as_str().unwrap());
        assert!(
            (layer.copper.unsigned_area() - case["area_mm2"].as_f64().unwrap()).abs() < 1e-5,
            "面積: {name}"
        );
        assert_eq!(
            layer.statistics.negative,
            case["negative"].as_bool().unwrap(),
            "{name}"
        );
        let found = find_gaps(
            &layer.copper,
            0.1,
            1e-7,
            true,
            layer.statistics.negative,
            0.3,
        )
        .0;
        if case["aggregation_unstable"].as_bool() == Some(true) {
            // 等幅の円環では微小な誤差が集約順を変えるため、検出範囲を双方向で照合する。
            let expected = case["locations_mm"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()])
                .collect::<Vec<_>>();
            let actual = found.iter().map(|r| [r.x_mm, r.y_mm]).collect::<Vec<_>>();
            for (from, to) in [(&expected, &actual), (&actual, &expected)] {
                for p in from {
                    assert!(
                        to.iter()
                            .any(|q| (p[0] - q[0]).hypot(p[1] - q[1]) <= 0.30001),
                        "検出範囲: {name}: {p:?}"
                    );
                }
            }
        } else {
            assert_eq!(
                found.len(),
                case["violation_count"].as_u64().unwrap() as usize,
                "検出数: {name}"
            );
        }
        if let Some(gap) = case["minimum_gap_mm"].as_f64() {
            let actual = found.iter().map(|r| r.gap_mm).fold(f64::INFINITY, f64::min);
            assert!(
                (gap - actual).abs() < 1e-5,
                "距離: {name}: {gap} != {actual}"
            );
        }
    }
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}
fn gaps(shape: &Shape) -> Vec<pcb_clearance::detector::Violation> {
    find_gaps(shape, 0.1, 1e-7, true, false, 0.3).0
}

#[test]
fn ordered_clear_polarity() {
    let layer = parse(&gerber(
        "X0Y0D03*%LPC*%D11*X0Y0D03*%LPD*%D12*X0Y0D03*",
        "%ADD10R,4X4*%%ADD11R,2X2*%%ADD12R,1X1*%",
        "MM",
        "LAX44Y44",
        "",
    ));
    close(layer.copper.unsigned_area(), 13.);
    assert!(covers(&layer.copper, 0., 0.));
    assert!(!covers(&layer.copper, 0.75, 0.));
}
#[test]
fn negative_file_polarity_comment_and_ip() {
    let text = gerber(
        "X0Y0D03*",
        "%ADD10R,1X1*%",
        "MM",
        "LAX44Y44",
        "G04 #@! TF.FilePolarity,Negative*",
    );
    let layer = parse(&text);
    assert!(layer.statistics.negative);
    assert!(!covers(&layer.copper, 0., 0.));
    assert!(covers(&layer.copper, 1., 0.));
    assert!(
        !parse(&text.replace("G01*", "%IPNEG*%G01*"))
            .statistics
            .negative
    );
}
#[test]
fn inches_and_trailing_zero_modal_operations() {
    let layer = parse(&gerber(
        "X10000Y20000D03*",
        "%ADD10R,1X1*%",
        "IN",
        "LAX44Y44",
        "",
    ));
    for (a, b) in bounds(&layer.copper)
        .into_iter()
        .zip([12.7, 38.1, 38.1, 63.5])
    {
        close(a, b);
    }
    let layer = parse(&gerber(
        "X1Y2D03*X2*",
        "%ADD10R,1X1*%",
        "MM",
        "TAX24Y24",
        "",
    ));
    assert_eq!(layer.statistics.flashes, 2);
    assert_eq!(bounds(&layer.copper), [9.5, 19.5, 20.5, 20.5]);
}
#[test]
fn standard_aperture_hole_and_rectangular_sweep() {
    let layer = parse(&gerber("X0Y0D03*", "%ADD10C,2X1*%", "MM", "LAX44Y44", ""));
    assert!(!covers(&layer.copper, 0., 0.));
    assert!(covers(&layer.copper, 0.75, 0.));
    let layer = parse(&gerber(
        "X0Y0D02*X20000Y10000D01*",
        "%ADD10R,2X1*%",
        "MM",
        "LAX44Y44",
        "",
    ));
    assert_eq!(bounds(&layer.copper), [-1., -0.5, 3., 1.5]);
    close(layer.copper.unsigned_area(), 6.);
    assert!(covers(&layer.copper, -1., -0.5));
}
#[test]
fn multiple_region_contours_and_cut_in_hole() {
    let body = "G36*X0Y0D02*X40000D01*Y40000D01*X0D01*Y0D01*X10000Y10000D02*X20000D01*Y20000D01*X10000D01*Y10000D01*D02*G37*";
    close(parse(&standard(body)).copper.unsigned_area(), 16.);
    let body = "G36*X0Y0D02*X40000D01*Y40000D01*X0D01*Y20000D01*X10000D01*Y10000D01*X30000D01*Y30000D01*X10000D01*Y20000D01*X0D01*Y0D01*G37*";
    let layer = parse(&standard(body));
    close(layer.copper.unsigned_area(), 12.);
    assert!(!covers(&layer.copper, 2., 2.));
}
#[test]
fn arc_full_circle_and_single_quadrant() {
    for body in [
        "G75*G03*X10000Y0D02*X10000Y0I-10000J0D01*",
        "G74*G03*X10000Y0D02*X0Y10000I10000J0D01*",
    ] {
        let layer = parse(&gerber(body, "%ADD10C,0.2*%", "MM", "LAX44Y44", ""));
        assert_eq!(layer.statistics.arcs, 1);
        assert!(layer.copper.unsigned_area() > 0.3);
    }
}
#[test]
fn macro_thermal_and_transformed_variables() {
    let shape = macro_shape("thermal", &["7,0,0,2,1.84,0.15,45".into()], &[]).unwrap();
    assert!(!covers(&shape, 0., 0.));
    assert!(covers(&shape, 0.95, 0.));
    let layer = parse(&gerber(
        "X0Y0D03*",
        "%AMT*7,0,0,2,1.84,0.15,45*%%ADD10T*%",
        "MM",
        "LAX44Y44",
        "%TF.FilePolarity,Negative*%",
    ));
    let found = find_gaps(&layer.copper, 0.1, 1e-7, true, true, 0.3).0;
    assert!(!found.is_empty());
    assert!((found.iter().map(|r| r.gap_mm).fold(f64::INFINITY, f64::min) - 0.08).abs() < 0.0001);
    let defs = "%AMT*$2=$1/2*21,1,$1,$1,0,0,0*21,0,$2,$2,0,0,0*%%ADD10T,2*%";
    close(
        parse(&gerber(
            "%LMX*%%LR90*%%LS2*%X0Y0D03*",
            defs,
            "MM",
            "LAX44Y44",
            "",
        ))
        .copper
        .unsigned_area(),
        12.,
    );
}
#[test]
fn unsupported_commands_and_incomplete_input_fail_closed() {
    for command in ["%SRX2Y2I10J10*%", "%ABD11*%", "%OFA1B1*%", "G91*", "G12*"] {
        assert!(
            GerberParser::new(&standard(&format!("{command}X0Y0D03*")), "auto", 1.)
                .parse()
                .is_err(),
            "{command}"
        );
    }
    for text in [
        standard("X0Y0D03*").replace("M02*", ""),
        standard("G36*X0Y0D02*X10000Y0D01*X10000Y10000D01*G37*"),
    ] {
        assert!(GerberParser::new(&text, "auto", 1.).parse().is_err());
    }
}
#[test]
fn below_equal_and_above_threshold() {
    for (gap, expected) in [
        (0.08, true),
        (0.0999, true),
        (0.09999995, false),
        (0.1, false),
        (0.10000005, false),
        (0.10001, false),
        (0.12, false),
    ] {
        let copper = rectangle(0., 0., 1., 2.).union(&rectangle(1. + gap, 0., 2. + gap, 2.));
        let found = gaps(&copper);
        assert_eq!(!found.is_empty(), expected, "gap={gap}");
        if expected {
            close(found[0].gap_mm, gap);
        }
    }
}
fn slit(gap: f64, depth: f64) -> Shape {
    MultiPolygon(vec![polygon(&[
        [0., 0.],
        [3., 0.],
        [3., 3.],
        [1.5 + gap, 3.],
        [1.5 + gap, 3. - depth],
        [1.5, 3. - depth],
        [1.5, 3.],
        [0., 3.],
    ])])
}
#[test]
fn same_copper_slit_and_corner_filter() {
    let found = gaps(&slit(0.08, 2.));
    assert!(!found.is_empty());
    assert_eq!(found[0].kind, "self_approach");
    close(found[0].gap_mm, 0.08);
    for filter in [true, false] {
        assert!(
            find_gaps(&slit(0.1, 2.), 0.1, 1e-7, filter, false, 0.3)
                .0
                .is_empty()
        );
    }
    let (filtered, stats) = find_gaps(&slit(0.08, 0.1), 0.1, 1e-7, true, false, 0.3);
    assert!(filtered.is_empty());
    assert!(stats.corner_filtered_pairs > 0);
    assert!(
        !find_gaps(&slit(0.08, 0.1), 0.1, 1e-7, false, false, 0.3)
            .0
            .is_empty()
    );
}
#[test]
fn copper_bridge_is_not_gap_and_circular_pads() {
    let copper = rectangle(0., 0., 4., 4.)
        .difference(&rectangle(1., 1., 1.9, 3.).union(&rectangle(2., 1., 3., 3.)));
    assert!(gaps(&copper).is_empty());
    close(
        gaps(&circle(1.).union(&translate(&circle(1.), [1.08, 0.])))[0].gap_mm,
        0.08,
    );
}

#[test]
fn repeated_overlay_does_not_open_touching_copper_at_board_scale() {
    use pcb_clearance::overlay::unary_union;
    // 実基板で偽ギャップが生じた接続形状を縮約した回帰テスト。
    let region = MultiPolygon(vec![polygon(&[
        [51.5, 76.1],
        [51.5, 76.],
        [51.3, 76.],
        [51.3, 76.1],
        [50.8, 76.1],
        [50.8, 76.6],
        [51.8, 76.6],
        [51.8, 76.1],
    ])]);
    let trace = stroke(&[[51.2, 75.], [51.2, 76.2918]], 0.2);
    let copper = unary_union(&[
        region,
        trace,
        rectangle(0., 0., 1., 1.),
        rectangle(95., 153., 96., 154.),
    ]);
    assert!(gaps(&copper).is_empty());
    assert!(covers(&copper, 51.3, 76.05));
}

#[test]
fn indexed_inside_test_handles_nested_islands_and_negative_frame() {
    let outer = rectangle(-2., -2., 6., 6.);
    let cut = rectangle(0., 0., 4., 4.).difference(&rectangle(1., 1., 3., 3.));
    let copper = outer.difference(&cut);
    // 内側の銅箔と外側の銅箔には1 mmの空隙があり、0.1 mm判定では検出しない。
    assert!(find_gaps(&copper, 0.1, 1e-7, false, true, 0.3).0.is_empty());
    let copper = outer
        .difference(&rectangle(0., 0., 4., 4.))
        .union(&rectangle(0.08, 1., 2., 3.));
    let found = find_gaps(&copper, 0.1, 1e-7, true, true, 0.3).0;
    assert!(!found.is_empty());
    close(found[0].gap_mm, 0.08);
}
#[test]
fn shared_engine_reports_images_progress_and_empty_output_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("入力");
    fs::create_dir(&input).unwrap();
    for (ext, distance) in [("GTL", 10800), ("GBL", 12000), ("GTS", 0)] {
        fs::write(
            input.join(format!("基板.{ext}")),
            standard(&format!("X0Y0D03*X{distance}Y0D03*")),
        )
        .unwrap();
    }
    for jobs in [1, 2] {
        let out = tmp.path().join(format!("out{jobs}"));
        let mut events = vec![];
        let (report, dir) = run_check(
            std::slice::from_ref(&input),
            Some(&out),
            &Options {
                jobs,
                ..Default::default()
            },
            |p| events.push(p),
        )
        .unwrap();
        assert_eq!(report.status, "violations");
        assert_eq!(report.exit_code(), 1);
        assert_eq!(report.layers.len(), 2);
        assert_eq!(report.skipped_inputs.len(), 1);
        assert!(matches!(events[0], Progress::Started { total: 2, .. }));
        assert!(matches!(
            events[2],
            Progress::LayerDone { completed: 2, .. }
        ));
        assert!(matches!(events[3], Progress::Report));
        let mut count = 0;
        for l in &report.layers {
            assert!(dir.join(&l.overview).is_file());
            for r in &l.violations {
                count += 1;
                let png = fs::read(dir.join(&r.image)).unwrap();
                assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
                close(
                    r.gap_mm,
                    (r.point_a_mm[0] - r.point_b_mm[0]).hypot(r.point_a_mm[1] - r.point_b_mm[1]),
                );
            }
        }
        assert!(count > 0);
        assert!(
            fs::read_to_string(dir.join("index.html"))
                .unwrap()
                .contains("0.1 mm 未満")
        );
        let csv = fs::read(dir.join("violations.csv")).unwrap();
        assert_eq!(&csv[..3], &[0xef, 0xbb, 0xbf]);
        assert_eq!(csv::Reader::from_reader(&csv[3..]).records().count(), count);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("report.json")).unwrap()).unwrap();
        assert_eq!(saved["comparison"], "gap_mm < threshold_mm - tolerance_mm");
        assert!(
            run_check(
                std::slice::from_ref(&input),
                Some(&out),
                &Options::default(),
                |_| {}
            )
            .is_err()
        );
    }
}
#[test]
fn exact_threshold_clear_and_bad_copper_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("board.GTL");
    fs::write(&source, standard("X0Y0D03*X11000Y0D03*")).unwrap();
    let report = run_check(
        std::slice::from_ref(&source),
        Some(&tmp.path().join("clear")),
        &Options::default(),
        |_| {},
    )
    .unwrap()
    .0;
    assert_eq!(report.status, "clear");
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.violation_count, 0);
    fs::write(&source, "bad").unwrap();
    let report = run_check(
        &[source],
        Some(&tmp.path().join("error")),
        &Options::default(),
        |_| {},
    )
    .unwrap()
    .0;
    assert_eq!(report.status, "error");
    assert_eq!(report.exit_code(), 2);
    assert_eq!(report.error_count, 1);
}
#[test]
fn invalid_options_rejected_before_output() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    for threshold in [0., -1., f64::NAN, f64::INFINITY, 1e-8] {
        assert!(
            run_check(
                &[PathBuf::from("missing.GTL")],
                Some(&out),
                &Options {
                    threshold,
                    ..Default::default()
                },
                |_| {}
            )
            .is_err()
        );
        assert!(!out.exists());
    }
    assert!(
        Options {
            polarity: "bad".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}
#[test]
fn x2_overrides_extension_and_discovery_expands_and_deduplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("board.GTL"),
        gerber(
            "X0Y0D03*",
            "%ADD10R,1X1*%",
            "MM",
            "LAX44Y44",
            "%TF.FileFunction,Soldermask,Top*%",
        ),
    )
    .unwrap();
    fs::write(root.join("board-In1_Cu.gbr"), standard("X0Y0D03*")).unwrap();
    fs::write(
        root.join("generic.gbr"),
        gerber(
            "X0Y0D03*",
            "%ADD10R,1X1*%",
            "MM",
            "LAX44Y44",
            "G04 #@! TF.FileFunction,Copper,L2,Inr*",
        ),
    )
    .unwrap();
    let (files, skipped) = discover(&[root.to_path_buf(), root.join("board-In1_Cu.gbr")]).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(skipped.len(), 1);
    assert_eq!(
        files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        ["board-In1_Cu.gbr", "generic.gbr"]
    );
    assert!(discover(&[root.join("*.gbr")]).unwrap().0.len() == 2);
}
#[test]
fn cli_exit_codes_and_invalid_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("board.GTL");
    for (distance, expected) in [(11000, 0), (10800, 1)] {
        fs::write(&source, standard(&format!("X0Y0D03*X{distance}Y0D03*"))).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pcb-clearance"))
            .arg(&source)
            .args(["--outdir"])
            .arg(tmp.path().join(distance.to_string()))
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for args in [["--jobs", "-1"], ["--threshold", "NaN"]] {
        let status = std::process::Command::new(env!("CARGO_BIN_EXE_pcb-clearance"))
            .arg(&source)
            .args(args)
            .output()
            .unwrap()
            .status;
        assert_eq!(status.code(), Some(2));
    }
}

#[cfg(feature = "gui")]
#[test]
fn gui_input_list_and_progress_state() {
    use pcb_clearance::gui::{InputSelection, ProgressState};
    let tmp = tempfile::tempdir().unwrap();
    for ext in ["GTL", "GBL", "GTS", "GTO"] {
        fs::write(
            tmp.path().join(format!("board.{ext}")),
            standard("X0Y0D03*"),
        )
        .unwrap();
    }
    let mut selection = InputSelection::default();
    selection.add(&[tmp.path().to_path_buf()]).unwrap();
    assert_eq!(selection.files.len(), 2);
    selection
        .add(&[tmp.path().to_path_buf(), tmp.path().join("board.GTL")])
        .unwrap();
    assert_eq!(selection.files.len(), 2);
    selection.selected.insert(0);
    selection.remove_selected();
    assert_eq!(selection.files[0].file_name().unwrap(), "board.GTL");
    let empty = tmp.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(selection.add(&[empty]).is_err());
    assert_eq!(selection.files.len(), 1);
    let mut progress = ProgressState::default();
    progress.apply(&Progress::Started {
        total: 6,
        outdir: tmp.path().to_path_buf(),
    });
    assert_eq!(progress.label(), "0 / 6 層 (0%)");
    progress.completed = 1;
    assert_eq!(progress.label(), "1 / 6 層 (17%)");
    progress.completed = 6;
    progress.apply(&Progress::Report);
    assert_eq!(progress.fraction(), 1.);
    assert!(progress.phase.contains("HTML"));
}
