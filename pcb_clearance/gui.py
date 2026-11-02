"""ガーバーを選択し、バックグラウンドで検査して HTML を開く GUI。"""
from datetime import datetime
import multiprocessing
from pathlib import Path
import queue
import sys
import threading
import time
import tkinter as tk
from tkinter import filedialog, messagebox, ttk
import webbrowser

from .cli import discover, run_check
from .parser import GerberError


def default_output_root():
    """exe 配布時は隣の results、ソース実行時はプロジェクトの output を使う。"""
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent / "results"
    return Path(__file__).resolve().parent.parent / "output"


class ClearanceApp:
    def __init__(self, root):
        self.root = root
        self.inputs = []
        self.running = False
        self.total_layers = 0
        self.completed_layers = 0
        self.started_at = None
        self.phase_text = ""
        self.messages = queue.Queue()
        self.result_path = None
        self.initial_dir = str(Path.home())
        self.controls = []
        self.threshold = tk.StringVar(value="0.1")
        self.output_root = tk.StringVar(value=str(default_output_root()))
        self.auto_open = tk.BooleanVar(value=True)
        self.status = tk.StringVar(value="検査するデータを選択してください。")
        self.progress_text = tk.StringVar(value="未開始")
        root.title("PCB Clearance — ガーバークリアランス検査")
        root.geometry("900x680")
        root.minsize(760, 620)
        root.option_add("*Font", "{Yu Gothic UI} 10")
        style = ttk.Style(root)
        if "vista" in style.theme_names():
            style.theme_use("vista")
        root.columnconfigure(0, weight=1)
        root.rowconfigure(0, weight=1)
        frame = ttk.Frame(root, padding=22)
        frame.grid(sticky="nsew")
        frame.columnconfigure(0, weight=1)
        frame.rowconfigure(2, weight=1)
        ttk.Label(frame, text="ガーバー クリアランス検査", font=("Yu Gothic UI", 18, "bold")).grid(sticky="w")
        ttk.Label(frame, text="データを選択 → 検査開始 → 座標と画像を HTML で確認", padding=(0, 6, 0, 14)).grid(sticky="w")

        source = ttk.LabelFrame(frame, text="検査対象", padding=12)
        source.grid(row=2, sticky="nsew")
        source.columnconfigure(0, weight=1)
        source.rowconfigure(1, weight=1)
        actions = ttk.Frame(source)
        actions.grid(sticky="w", columnspan=2, pady=(0, 8))
        for text, command in (("ファイルを追加…", self.choose_files), ("フォルダーを追加…", self.choose_folder),
                              ("選択から外す", self.remove_selected)):
            button = ttk.Button(actions, text=text, command=command)
            button.pack(side="left", padx=(0, 8))
            self.controls.append(button)
        self.listbox = tk.Listbox(source, height=6, selectmode=tk.EXTENDED, activestyle="none", exportselection=False)
        self.listbox.grid(row=1, sticky="nsew")
        scrollbar = ttk.Scrollbar(source, orient="vertical", command=self.listbox.yview)
        scrollbar.grid(row=1, column=1, sticky="ns")
        self.listbox.configure(yscrollcommand=scrollbar.set)
        self.controls.append(self.listbox)
        ttk.Label(source, text="検査対象のファイルを表示しています。フォルダー内の銅箔層は自動選択します。").grid(row=2, sticky="w", columnspan=2, pady=(8, 0))

        settings = ttk.Frame(frame, padding=(0, 14))
        settings.grid(row=3, sticky="ew")
        settings.columnconfigure(1, weight=1)
        ttk.Label(settings, text="クリアランス").grid(row=0, column=0, sticky="w", padx=(0, 12))
        threshold_row = ttk.Frame(settings)
        threshold_row.grid(row=0, column=1, sticky="w")
        threshold_entry = ttk.Entry(threshold_row, textvariable=self.threshold, width=10)
        threshold_entry.pack(side="left")
        self.controls.append(threshold_entry)
        ttk.Label(threshold_row, text=" mm 未満を検出").pack(side="left", padx=(4, 0))
        ttk.Label(settings, text="結果の保存先").grid(row=1, column=0, sticky="w", pady=(10, 0), padx=(0, 12))
        output_entry = ttk.Entry(settings, textvariable=self.output_root)
        output_entry.grid(row=1, column=1, sticky="ew", pady=(10, 0))
        browse = ttk.Button(settings, text="参照…", command=self.choose_output)
        browse.grid(row=1, column=2, padx=(8, 0), pady=(10, 0))
        self.controls.extend((output_entry, browse))
        ttk.Label(settings, text="検査ごとに日時付きフォルダーを作成します。").grid(row=2, column=1, sticky="w", pady=(4, 0))
        auto = ttk.Checkbutton(settings, text="完了後、結果の HTML を自動で開く", variable=self.auto_open)
        auto.grid(row=3, column=1, sticky="w", pady=(8, 0))
        self.controls.append(auto)

        footer = ttk.Frame(frame)
        footer.grid(row=4, sticky="ew")
        self.start_button = ttk.Button(footer, text="検査開始", command=self.start_check, width=18)
        self.start_button.pack(side="left")
        self.controls.append(self.start_button)
        self.open_button = ttk.Button(footer, text="結果を開く", command=self.open_report, state="disabled")
        self.open_button.pack(side="left", padx=(10, 0))
        progress_row = ttk.Frame(frame)
        progress_row.grid(row=5, sticky="ew", pady=(16, 8))
        progress_row.columnconfigure(0, weight=1)
        self.progress = ttk.Progressbar(progress_row, mode="determinate", maximum=1)
        self.progress.grid(row=0, column=0, sticky="ew")
        self.activity = ttk.Progressbar(progress_row, mode="indeterminate", length=45)
        self.activity.grid(row=0, column=1, padx=(10, 8))
        self.activity.grid_remove()
        ttk.Label(progress_row, textvariable=self.status, width=42).grid(row=0, column=2, sticky="w", padx=(8, 0))
        ttk.Label(progress_row, textvariable=self.progress_text, width=18, anchor="e").grid(row=0, column=3, padx=(8, 0))
        self.details = tk.Text(frame, height=5, state="disabled", wrap="word", relief="flat", background="#f2f4f7")
        self.details.grid(row=6, sticky="ew", pady=(10, 0))
        root.protocol("WM_DELETE_WINDOW", self.close)
        root.after(100, self.poll)

    def add_inputs(self, paths):
        try:
            # フォルダーを実際の検査対象へ展開し、一覧と実行対象を一致させる。
            files, _ = discover(paths)
        except (OSError, GerberError) as exc:
            messagebox.showwarning("検査対象を追加できません", str(exc), parent=self.root)
            return
        for value in files:
            path = str(value)
            if path not in self.inputs:
                self.inputs.append(path)
                self.listbox.insert(tk.END, path)
        if self.inputs:
            self.status.set(f"対象 {len(self.inputs)} ファイル。検査開始を押してください。")

    def choose_files(self):
        paths = filedialog.askopenfilenames(parent=self.root, title="ガーバーファイルを選択（複数可）",
                                           initialdir=self.initial_dir,
                                           filetypes=[("ガーバーファイル", "*.gbr *.ger *.gtl *.gbl *.g? *.g?? *.gp*"), ("すべてのファイル", "*")])
        if paths:
            self.initial_dir = str(Path(paths[0]).parent)
            self.add_inputs(paths)

    def choose_folder(self):
        path = filedialog.askdirectory(parent=self.root, title="ガーバーフォルダーを選択", initialdir=self.initial_dir)
        if path:
            self.initial_dir = path
            self.add_inputs([path])

    def remove_selected(self):
        for index in reversed(self.listbox.curselection()):
            del self.inputs[index]
            self.listbox.delete(index)
        self.status.set(f"対象 {len(self.inputs)} 件。")

    def choose_output(self):
        path = filedialog.askdirectory(parent=self.root, title="結果の保存先を選択")
        if path:
            self.output_root.set(path)

    def append_detail(self, text):
        self.details.configure(state="normal")
        self.details.insert(tk.END, text + "\n")
        self.details.see(tk.END)
        self.details.configure(state="disabled")

    def start_check(self):
        if self.running:
            return
        if not self.inputs:
            messagebox.showwarning("検査対象", "ガーバーファイル、またはフォルダーを選択してください。", parent=self.root)
            return
        try:
            threshold = float(self.threshold.get())
            if not self.output_root.get().strip():
                raise ValueError("結果の保存先を選択してください。")
            from math import isfinite
            if not isfinite(threshold) or threshold <= 0:
                raise ValueError("クリアランスは0より大きい数値を指定してください。")
        except ValueError as exc:
            messagebox.showwarning("設定を確認してください", str(exc), parent=self.root)
            return
        outdir = Path(self.output_root.get()).expanduser() / datetime.now().strftime("clearance-%Y%m%d-%H%M%S-%f")
        self.running = True
        self.result_path = None
        for control in self.controls:
            control.configure(state="disabled")
        self.open_button.configure(state="disabled")
        self.details.configure(state="normal")
        self.details.delete("1.0", tk.END)
        self.details.configure(state="disabled")
        self.total_layers = self.completed_layers = 0
        self.started_at = time.monotonic()
        self.phase_text = "ガーバーを読み込んでいます…"
        self.progress.configure(maximum=1, value=0)
        self.progress_text.set("対象を確認中")
        # 全体の進捗とは別に、処理中であることだけをアニメーションで示す。
        self.activity.grid()
        self.activity.start(20)
        self.update_activity()
        threading.Thread(target=self.check_in_background, args=(list(self.inputs), outdir, threshold), daemon=True).start()

    def check_in_background(self, inputs, outdir, threshold):
        # Tk の操作は行わず、キュー経由でメインスレッドへ結果を渡す。
        try:
            report, target = run_check(inputs, outdir, threshold=threshold, on_progress=self.messages.put)
            self.messages.put(dict(stage="finished", report=report, outdir=str(target)))
        except Exception as exc:
            self.messages.put(dict(stage="failed", error=f"{type(exc).__name__}: {exc}"))

    def poll(self):
        try:
            while True:
                event = self.messages.get_nowait()
                stage = event["stage"]
                if stage == "started":
                    self.total_layers = event["total"]
                    self.completed_layers = 0
                    self.update_layer_progress()
                    self.phase_text = "銅箔を検査しています…"
                    self.append_detail(f"保存先: {event['outdir']}")
                elif stage == "layer_done":
                    layer = event["layer"]
                    text = f"エラー: {layer['error']}" if layer.get("error") else f"{len(layer['violations'])} 箇所"
                    self.append_detail(f"{layer['name']}: {text}")
                    self.total_layers = event["total"]
                    self.completed_layers = event["completed"]
                    self.update_layer_progress()
                elif stage == "report":
                    self.phase_text = "HTMLレポート作成中…"
                elif stage in ("finished", "failed"):
                    self.finish(event)
        except queue.Empty:
            pass
        if self.running:
            self.update_activity()
        self.root.after(100, self.poll)

    def update_layer_progress(self):
        """処理時間とは独立に、完了した層数だけで全体バーを更新する。"""
        self.progress.configure(maximum=max(1, self.total_layers), value=self.completed_layers)
        percent = round(100 * self.completed_layers / self.total_layers) if self.total_layers else 0
        self.progress_text.set(f"{self.completed_layers} / {self.total_layers} 層 ({percent}%)")

    def update_activity(self):
        """層の処理が長くても、経過時間と別のアニメーションで動作を示す。"""
        elapsed = max(0, int(time.monotonic() - self.started_at))
        minutes, seconds = divmod(elapsed, 60)
        self.status.set(f"{self.phase_text}  経過 {minutes:02d}:{seconds:02d}")

    def finish(self, event):
        self.running = False
        self.activity.stop()
        self.activity.grid_remove()
        for control in self.controls:
            control.configure(state="normal")
        if event["stage"] == "failed":
            self.status.set("検査に失敗しました。詳細をご確認ください。")
            self.append_detail(event["error"])
            messagebox.showerror("検査エラー", event["error"], parent=self.root)
            return
        report = event["report"]
        self.total_layers = self.completed_layers = len(report["layers"])
        self.update_layer_progress()
        self.result_path = Path(event["outdir"]) / "index.html"
        self.status.set(f"検査完了: 違反 {report['violation_count']} 箇所 / エラー {report['error_count']} 層")
        self.open_button.configure(state="normal")
        if self.auto_open.get():
            self.open_report()

    def open_report(self):
        if self.result_path and self.result_path.is_file():
            try:
                opened = webbrowser.open(self.result_path.resolve().as_uri())
                if not opened:
                    raise OSError("既定のブラウザーを起動できませんでした。")
            except (OSError, webbrowser.Error) as exc:
                messagebox.showerror("HTML を開けません", f"{exc}\n\n{self.result_path}", parent=self.root)

    def close(self):
        if self.running:
            messagebox.showinfo("検査中", "検査完了後に閉じてください。画面の最小化はできます。", parent=self.root)
            return
        self.root.destroy()


def main():
    multiprocessing.freeze_support()
    root = tk.Tk()
    ClearanceApp(root)
    root.mainloop()
