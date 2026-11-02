use crate::engine::{
    InputFile, Options, Progress, Report, dated_output, default_output_root, discover,
    run_check_selected,
};
use eframe::egui;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

enum Event {
    Progress(Progress),
    Finished(Box<Report>, PathBuf),
    Failed(String),
}
#[derive(Default)]
pub struct InputSelection {
    pub files: Vec<InputFile>,
    pub selected: BTreeSet<usize>,
}
impl InputSelection {
    pub fn add(&mut self, paths: &[PathBuf]) -> anyhow::Result<()> {
        let (files, _) = discover(paths)?;
        for file in files {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
        Ok(())
    }
    pub fn remove_selected(&mut self) {
        let mut i = 0;
        self.files.retain(|_| {
            let keep = !self.selected.contains(&i);
            i += 1;
            keep
        });
        self.selected.clear();
    }
}
#[derive(Default)]
pub struct ProgressState {
    pub total: usize,
    pub completed: usize,
    pub phase: String,
}
impl ProgressState {
    pub fn apply(&mut self, p: &Progress) {
        match p {
            Progress::Started { total, .. } => {
                self.total = *total;
                self.completed = 0;
                self.phase = "銅箔を検査しています…".into();
            }
            Progress::LayerDone {
                total, completed, ..
            } => {
                self.total = *total;
                self.completed = *completed;
            }
            Progress::Report => self.phase = "HTMLレポート作成中…".into(),
        }
    }
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.
        } else {
            self.completed as f32 / self.total as f32
        }
    }
    pub fn label(&self) -> String {
        format!(
            "{} / {} 層 ({}%)",
            self.completed,
            self.total,
            (self.fraction() * 100.).round()
        )
    }
}
struct ClearanceApp {
    input: InputSelection,
    threshold: String,
    output: String,
    auto_open: bool,
    running: bool,
    status: String,
    details: Vec<String>,
    result: Option<PathBuf>,
    rx: Option<mpsc::Receiver<Event>>,
    started: Option<Instant>,
    progress: ProgressState,
    initial_dir: PathBuf,
}
impl ClearanceApp {
    fn new(ctx: &egui::Context) -> Self {
        configure_style(ctx);
        // 日本語フォントを OS から読み、ファイル名と説明を日本語で表示する。
        let font = [
            "C:/Windows/Fonts/meiryo.ttc",
            "C:/Windows/Fonts/YuGothR.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok());
        if let Some(data) = font {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("japanese".into(), egui::FontData::from_owned(data).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "japanese".into());
            ctx.set_fonts(fonts);
        }
        Self {
            input: InputSelection::default(),
            threshold: "0.1".into(),
            output: default_output_root().display().to_string(),
            auto_open: true,
            running: false,
            status: "検査するデータを選択してください。".into(),
            details: vec![],
            result: None,
            rx: None,
            started: None,
            progress: ProgressState::default(),
            initial_dir: std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }
    fn add(&mut self, paths: Vec<PathBuf>) {
        if let Some(p) = paths.first() {
            self.initial_dir = if p.is_dir() {
                p.clone()
            } else {
                p.parent().unwrap_or(p).to_path_buf()
            };
        }
        match self.input.add(&paths) {
            Ok(()) => {
                self.status = format!(
                    "対象 {} ファイル。検査開始を押してください。",
                    self.input.files.len()
                )
            }
            Err(e) => self.status = format!("検査対象を追加できません: {e:#}"),
        }
    }
    fn clear_all(&mut self) {
        if self.running {
            return;
        }
        self.input = InputSelection::default();
        self.details.clear();
        self.result = None;
        self.rx = None;
        self.started = None;
        self.progress = ProgressState::default();
        self.status = "検査するデータを選択してください。".into();
    }
    fn start(&mut self, ctx: &egui::Context) {
        if self.running {
            return;
        }
        let prepare = || -> anyhow::Result<_> {
            anyhow::ensure!(
                !self.input.files.is_empty(),
                "ガーバーファイル、またはフォルダーを選択してください。"
            );
            anyhow::ensure!(
                !self.output.trim().is_empty(),
                "結果の保存先を選択してください。"
            );
            let options = Options {
                threshold: self.threshold.trim().parse()?,
                ..Default::default()
            };
            options.validate()?;
            Ok((options, dated_output(&PathBuf::from(self.output.trim()))))
        };
        let (options, outdir) = match prepare() {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("設定を確認してください: {e:#}");
                return;
            }
        };
        self.running = true;
        self.result = None;
        self.details.clear();
        self.started = Some(Instant::now());
        self.progress = ProgressState {
            phase: "ガーバーを読み込んでいます…".into(),
            ..Default::default()
        };
        let inputs = self.input.files.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let send = |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_check_selected(&inputs, Some(&outdir), &options, |p| {
                    send(Event::Progress(p))
                })
            }));
            match result {
                Ok(Ok((report, target))) => send(Event::Finished(Box::new(report), target)),
                Ok(Err(e)) => send(Event::Failed(format!("{e:#}"))),
                Err(_) => send(Event::Failed("検査スレッドが異常終了しました".into())),
            }
        });
    }
    fn open(&mut self) {
        if let Some(path) = &self.result
            && let Err(e) = open::that(path)
        {
            self.status = format!("HTML を開けません: {e}\n{}", path.display());
        }
    }
    fn poll(&mut self) {
        let events = self
            .rx
            .as_ref()
            .map(|r| r.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Progress(p) => {
                    self.progress.apply(&p);
                    match p {
                        Progress::Started { outdir, .. } => {
                            self.details.push(format!("保存先: {}", outdir.display()))
                        }
                        Progress::LayerDone { layer, .. } => self.details.push(format!(
                            "{}: {}",
                            layer.name,
                            layer
                                .error
                                .as_ref()
                                .map(|e| format!("エラー: {e}"))
                                .unwrap_or_else(|| format!("{} 箇所", layer.violations.len()))
                        )),
                        _ => {}
                    }
                }
                Event::Finished(report, target) => {
                    self.running = false;
                    self.progress.total = report.layers.len();
                    self.progress.completed = report.layers.len();
                    self.result = Some(target.join("index.html"));
                    self.status = format!(
                        "検査完了: 違反 {} 箇所 / エラー {} 層",
                        report.violation_count, report.error_count
                    );
                    if self.auto_open {
                        self.open();
                    }
                }
                Event::Failed(e) => {
                    self.running = false;
                    self.status = "検査に失敗しました。詳細をご確認ください。".into();
                    self.details.push(e);
                }
            }
        }
    }
}
impl ClearanceApp {
    fn render(&mut self, ctx: &egui::Context) {
        self.poll();
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(100));
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.details
                    .push("検査完了後に閉じてください。画面の最小化はできます。".into());
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("ガーバー クリアランス検査");
            ui.label("データを選択 → 検査開始 → 座標と画像を HTML で確認");
            ui.add_space(12.);
            ui.add_enabled_ui(!self.running, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("ファイルを追加…").clicked()
                        && let Some(files) = rfd::FileDialog::new()
                            .set_title("ガーバーファイルを選択（複数可）")
                            .set_directory(&self.initial_dir)
                            .pick_files()
                    {
                        self.add(files);
                    }
                    if ui.button("フォルダーを追加…").clicked()
                        && let Some(folder) = rfd::FileDialog::new()
                            .set_title("ガーバーフォルダーを選択")
                            .set_directory(&self.initial_dir)
                            .pick_folder()
                    {
                        self.add(vec![folder]);
                    }
                    if ui.button("ZIPを追加…").clicked()
                        && let Some(files) = rfd::FileDialog::new()
                            .set_title("ガーバーのZIPを選択（複数可）")
                            .set_directory(&self.initial_dir)
                            .add_filter("ZIP", &["zip"])
                            .pick_files()
                    {
                        self.add(files);
                    }
                    if ui.button("選択から外す").clicked() {
                        self.input.remove_selected();
                        self.status = format!("対象 {} 件。", self.input.files.len());
                    }
                });
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::ScrollArea::both()
                        .id_salt("inputs")
                        .max_height(220.)
                        .min_scrolled_height(120.)
                        .show(ui, |ui| {
                            for (i, p) in self.input.files.iter().enumerate() {
                                let selected = self.input.selected.contains(&i);
                                if ui
                                    .selectable_label(selected, p.display().to_string())
                                    .clicked()
                                {
                                    if selected {
                                        self.input.selected.remove(&i);
                                    } else {
                                        self.input.selected.insert(i);
                                    }
                                }
                            }
                        });
                });
                ui.label("フォルダー・ZIP内は下位フォルダーも含めて銅箔層を自動選択します。");
                ui.add_space(12.);
                ui.horizontal(|ui| {
                    ui.label("クリアランス");
                    ui.add(egui::TextEdit::singleline(&mut self.threshold).desired_width(85.));
                    ui.label("mm 未満を検出");
                });
                ui.horizontal(|ui| {
                    ui.label("結果の保存先");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.output)
                            .desired_width((ui.available_width() - 80.).max(100.)),
                    );
                    if ui.button("参照…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_folder()
                    {
                        self.output = p.display().to_string();
                    }
                });
                ui.label("検査ごとに日時付きフォルダーを作成します。");
                ui.checkbox(&mut self.auto_open, "完了後、結果の HTML を自動で開く");
            });
            ui.add_space(12.);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.running, egui::Button::new("検査開始"))
                    .clicked()
                {
                    self.start(ctx);
                }
                if ui
                    .add_enabled(
                        !self.running && self.result.is_some(),
                        egui::Button::new("結果を開く"),
                    )
                    .clicked()
                {
                    self.open();
                }
                if ui
                    .add_enabled(!self.running, egui::Button::new("すべてクリア"))
                    .clicked()
                {
                    self.clear_all();
                    ctx.request_repaint();
                }
            });
            ui.add_space(12.);
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(self.progress.fraction())
                        .text(self.progress.label())
                        .desired_width(300.),
                );
                if self.running {
                    ui.spinner();
                    let elapsed = self.started.unwrap().elapsed().as_secs();
                    ui.label(format!(
                        "{} 経過 {:02}:{:02}",
                        self.progress.phase,
                        elapsed / 60,
                        elapsed % 60
                    ));
                }
            });
            if !self.running {
                ui.label(&self.status);
            }
            egui::ScrollArea::vertical()
                .id_salt("details")
                .max_height(140.)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.details {
                        ui.label(line);
                    }
                });
        });
    }
}
fn configure_style(ctx: &egui::Context) {
    // OS のテーマに左右されず、白い背景と濃い文字で表示する。
    ctx.set_theme(egui::Theme::Light);
    let mut visuals = egui::Visuals::light();
    let text = egui::Color32::from_rgb(16, 16, 16);
    visuals.override_text_color = Some(text);
    visuals.panel_fill = egui::Color32::from_rgb(250, 250, 250);
    visuals.window_fill = egui::Color32::WHITE;
    visuals.extreme_bg_color = egui::Color32::WHITE;
    visuals.selection.bg_fill = egui::Color32::from_rgb(185, 218, 250);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, text);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.fg_stroke.color = text;
    }
    visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(120));
    visuals.widgets.inactive.bg_fill = egui::Color32::from_gray(232);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_gray(232);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(140));
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(22.));
    });
}
impl eframe::App for ClearanceApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render(ctx);
    }
}
pub fn run() -> eframe::Result {
    eframe::run_native(
        "PCB Clearance — ガーバークリアランス検査",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([950., 720.])
                .with_min_inner_size([800., 650.]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(ClearanceApp::new(&cc.egui_ctx)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draw(app: &mut ClearanceApp, ctx: &egui::Context) {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(950., 720.),
                )),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        );
        assert!(!output.shapes.is_empty());
    }
    #[test]
    fn initial_and_invalid_settings_render_without_native_window() {
        let ctx = egui::Context::default();
        let mut app = ClearanceApp::new(&ctx);
        draw(&mut app, &ctx);
        app.start(&ctx);
        assert!(!app.running);
        assert!(app.status.contains("選択"));
        draw(&mut app, &ctx);
    }
    #[test]
    fn background_check_finishes_and_clear_resets_view() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("基板.GTL");
        std::fs::write(
            &source,
            "%FSLAX44Y44*%%MOMM*%%ADD10R,1X1*%D10*X0Y0D03*X10800Y0D03*M02*",
        )
        .unwrap();
        let ctx = egui::Context::default();
        let mut app = ClearanceApp::new(&ctx);
        app.auto_open = false;
        app.input.add(&[source]).unwrap();
        app.output = tmp.path().join("結果").display().to_string();
        app.start(&ctx);
        assert!(app.running);
        let timeout = Instant::now() + Duration::from_secs(10);
        while app.running && Instant::now() < timeout {
            app.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.running);
        assert!(app.result.as_ref().is_some_and(|p| p.is_file()));
        assert_eq!(app.progress.fraction(), 1.);
        assert!(app.status.contains("違反 1 箇所"));
        draw(&mut app, &ctx);
        let saved_report = app.result.clone().unwrap();
        let settings = (app.threshold.clone(), app.output.clone(), app.auto_open);
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Failed("前回の通知".into())).unwrap();
        app.rx = Some(rx);
        app.input.selected.insert(0);
        app.clear_all();
        app.poll();
        assert!(app.input.files.is_empty());
        assert!(app.input.selected.is_empty());
        assert!(app.details.is_empty());
        assert!(app.result.is_none());
        assert!(app.rx.is_none());
        assert!(app.started.is_none());
        assert_eq!(app.progress.total, 0);
        assert_eq!(app.progress.completed, 0);
        assert!(app.status.contains("選択してください"));
        assert_eq!(
            (app.threshold.clone(), app.output.clone(), app.auto_open),
            settings
        );
        assert!(saved_report.is_file());
        draw(&mut app, &ctx);
    }
    #[test]
    fn clear_is_blocked_during_check_and_failure_preserves_progress() {
        let ctx = egui::Context::default();
        let mut app = ClearanceApp::new(&ctx);
        app.running = true;
        app.started = Some(Instant::now());
        app.progress.total = 6;
        app.progress.completed = 1;
        let (tx, rx) = mpsc::channel();
        app.rx = Some(rx);
        app.clear_all();
        assert!(app.running);
        assert!(app.rx.is_some());
        assert_eq!(app.progress.completed, 1);
        tx.send(Event::Failed("保存できません".into())).unwrap();
        app.poll();
        assert!(!app.running);
        assert_eq!(app.progress.completed, 1);
        assert!(app.status.contains("失敗"));
        assert_eq!(app.details.last().unwrap(), "保存できません");
        draw(&mut app, &ctx);
    }
}
