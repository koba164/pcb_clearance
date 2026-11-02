use pcb_clearance::engine::{Options, discover, run_check, run_check_selected};
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn gerber(distance: usize) -> String {
    format!("%FSLAX44Y44*%%MOMM*%%ADD10R,1X1*%D10*X0Y0D03*X{distance}Y0D03*M02*")
}
fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    zip_bytes_using(entries, CompressionMethod::Deflated)
}
fn zip_bytes_using(entries: &[(&str, Vec<u8>)], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(vec![]));
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn archive(path: &Path, entries: &[(&str, Vec<u8>)]) {
    fs::write(path, zip_bytes(entries)).unwrap();
}

#[test]
fn folder_includes_all_descendants_and_deduplicates_overlapping_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let deep = root.join("a/b");
    fs::create_dir_all(&deep).unwrap();
    fs::write(root.join("board.GTL"), gerber(10800)).unwrap();
    fs::write(deep.join("board.GTL"), gerber(12000)).unwrap();
    fs::write(deep.join("board.GTS"), gerber(12000)).unwrap();
    let (files, skipped) =
        discover(&[root.to_path_buf(), deep.clone(), deep.join("board.GTL")]).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(skipped.len(), 1);
    let report = run_check(
        &[root.to_path_buf()],
        Some(&root.join("results")),
        &Options {
            jobs: 2,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap()
    .0;
    assert_eq!(report.layers.len(), 2);
    assert_eq!(report.violation_count, 1);
    assert_eq!(report.error_count, 0);
    assert_ne!(report.layers[0].key, report.layers[1].key);
}

#[test]
fn folder_finds_zip_in_descendants_without_creating_extracted_files() {
    let tmp = tempfile::tempdir().unwrap();
    let deep = tmp.path().join("sub/deep");
    fs::create_dir_all(&deep).unwrap();
    let zip = deep.join("基板.ZIP");
    archive(
        &zip,
        &[
            ("output/front/board.GTL", gerber(10800).into_bytes()),
            ("output/back/board.GBL", gerber(12000).into_bytes()),
            ("output/mask/board.GTS", gerber(12000).into_bytes()),
        ],
    );
    let (files, skipped) = discover(&[tmp.path().to_path_buf(), zip.clone()]).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(skipped.len(), 1);
    assert!(skipped.iter().all(|s| s.path.contains("!")));
    assert!(
        files
            .iter()
            .all(|f| f.display().to_string().contains("基板.ZIP!"))
    );
    assert_eq!(fs::read_dir(&deep).unwrap().count(), 1);
    assert_eq!(files[0].read_bytes().unwrap(), gerber(12000).as_bytes());
}

#[test]
fn zip_reports_preserve_internal_paths_unique_images_and_parallel_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("基板.zip");
    archive(
        &zip,
        &[
            ("a/board.GTL", gerber(10800).into_bytes()),
            ("b/board.GTL", gerber(10800).into_bytes()),
        ],
    );
    let (report, out) = run_check(
        &[zip],
        Some(&tmp.path().join("out")),
        &Options {
            jobs: 2,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(report.layers.len(), 2);
    assert_eq!(report.violation_count, 2);
    assert_eq!(report.error_count, 0);
    assert_ne!(report.layers[0].key, report.layers[1].key);
    for layer in &report.layers {
        assert!(layer.name.starts_with("基板.zip!/"));
        assert!(layer.path.contains("基板.zip!"));
        assert!(out.join(&layer.overview).is_file());
        assert!(out.join(&layer.violations[0].image).is_file());
    }
    let html = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(html.contains("a/board.GTL"));
    assert!(html.contains("b/board.GTL"));
    let csv = fs::read_to_string(out.join("violations.csv")).unwrap();
    assert!(csv.contains("基板.zip!/a/board.GTL"));
}

#[test]
fn zip_filters_by_x2_and_does_not_open_nested_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("boards.zip");
    let attrs = "%TF.FileFunction,Copper,L1,Top*%";
    let copper = format!("{attrs}{}", gerber(10800));
    let mask = format!("%TF.FileFunction,Soldermask,Top*%{}", gerber(10800));
    let nested = zip_bytes(&[("hidden.GTL", copper.as_bytes().to_vec())]);
    fs::write(
        &zip,
        zip_bytes_using(
            &[
                ("deep/generic.gbr", copper.into_bytes()),
                ("mask.GTL", mask.into_bytes()),
                ("nested.zip", nested),
            ],
            CompressionMethod::Stored,
        ),
    )
    .unwrap();
    let (files, skipped) = discover(&[zip]).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "generic.gbr");
    assert_eq!(skipped.len(), 2);
    assert!(skipped.iter().any(|s| s.reason.contains("ZIP 内の ZIP")));
}

#[test]
fn zip_bad_known_copper_is_reported_as_a_layer_error() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("bad.zip");
    archive(&zip, &[("nested/bad.GTL", b"bad".to_vec())]);
    let (report, _) = run_check(
        &[zip],
        Some(&tmp.path().join("out")),
        &Options::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, "error");
    assert_eq!(report.error_count, 1);
}

#[test]
fn zip_selection_is_individual_and_changed_archive_cannot_select_another_member() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("boards.zip");
    archive(
        &zip,
        &[
            ("first.GTL", gerber(10800).into_bytes()),
            ("second.GTL", gerber(12000).into_bytes()),
        ],
    );
    let (files, _) = discover(std::slice::from_ref(&zip)).unwrap();
    let selected = vec![files[1].clone()];
    let (report, _) = run_check_selected(
        &selected,
        Some(&tmp.path().join("one")),
        &Options::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.layers.len(), 1);
    assert_eq!(report.violation_count, 0);
    assert!(report.layers[0].name.ends_with("second.GTL"));
    archive(&zip, &[("replacement.GTL", gerber(10800).into_bytes())]);
    assert!(
        files[0]
            .read_bytes()
            .unwrap_err()
            .to_string()
            .contains("内容が変わりました")
    );
}

#[test]
fn invalid_and_unsafe_archives_fail_without_extracting() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("bad.zip");
    fs::write(&zip, b"bad").unwrap();
    assert!(discover(std::slice::from_ref(&zip)).is_err());
    archive(&zip, &[("../escape.GTL", gerber(10800).into_bytes())]);
    assert!(discover(&[zip]).is_err());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[test]
fn wildcard_keeps_explicit_unknown_gerber_names() {
    let tmp = tempfile::tempdir().unwrap();
    for name in ["one.custom", "two.custom"] {
        fs::write(tmp.path().join(name), gerber(12000)).unwrap();
    }
    assert_eq!(discover(&[tmp.path().join("*.custom")]).unwrap().0.len(), 2);
}

#[cfg(feature = "gui")]
#[test]
fn gui_zip_add_remove_and_failed_add_keep_the_displayed_selection() {
    use pcb_clearance::gui::InputSelection;
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("boards.zip");
    archive(
        &zip,
        &[
            ("first.GTL", gerber(10800).into_bytes()),
            ("nested/second.GTL", gerber(12000).into_bytes()),
        ],
    );
    let mut input = InputSelection::default();
    input.add(std::slice::from_ref(&zip)).unwrap();
    input.add(&[zip]).unwrap();
    assert_eq!(input.files.len(), 2);
    input.selected.insert(0);
    input.remove_selected();
    assert_eq!(input.files.len(), 1);
    assert!(input.files[0].display().to_string().contains("second.GTL"));
    let bad = tmp.path().join("bad.zip");
    fs::write(&bad, b"bad").unwrap();
    assert!(input.add(&[bad]).is_err());
    assert_eq!(input.files.len(), 1);
}
