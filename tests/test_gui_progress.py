"""ウィンドウを開かず、完了通知と待機中の進捗表示を確認する。"""
from pathlib import Path
import queue
import unittest
from unittest.mock import Mock, patch

from pcb_clearance.gui import ClearanceApp


class ProgressTests(unittest.TestCase):
    def setUp(self):
        self.app = ClearanceApp.__new__(ClearanceApp)
        self.app.root = Mock()
        self.app.messages = queue.Queue()
        self.app.running = True
        self.app.started_at = 100.0
        self.app.total_layers = self.app.completed_layers = 0
        self.app.phase_text = "ガーバーを読み込んでいます…"
        self.app.progress = Mock()
        self.app.progress_text = Mock()
        self.app.status = Mock()
        self.app.activity = Mock()
        self.app.append_detail = Mock()
        self.app.controls = []
        self.app.open_button = Mock()
        self.app.auto_open = Mock()
        self.app.auto_open.get.return_value = False
        self.app.open_report = Mock()

    def poll(self, elapsed):
        with patch("pcb_clearance.gui.time.monotonic", return_value=100 + elapsed):
            self.app.poll()

    def start(self):
        self.app.messages.put(dict(stage="started", total=6, outdir="result"))
        self.poll(0)

    def complete_layer(self, number):
        self.app.messages.put(dict(stage="layer_done", total=6, completed=number,
                                   layer=dict(name=f"L{number}", violations=[])))
        self.poll(number * 10)

    def test_completed_layer_increases_bar_but_waiting_does_not(self):
        self.start()
        self.assertEqual(self.app.progress_text.set.call_args.args[0], "0 / 6 層 (0%)")
        self.complete_layer(1)
        self.assertEqual(self.app.progress_text.set.call_args.args[0], "1 / 6 層 (17%)")
        self.assertEqual(self.app.progress.configure.call_args.kwargs["value"], 1)
        updates = self.app.progress.configure.call_count
        self.poll(75)
        self.assertEqual(self.app.progress.configure.call_count, updates)
        self.assertIn("経過 01:15", self.app.status.set.call_args.args[0])
        self.app.activity.stop.assert_not_called()
        self.complete_layer(2)
        self.assertEqual(self.app.progress_text.set.call_args.args[0], "2 / 6 層 (33%)")
        self.assertEqual(self.app.progress.configure.call_args.kwargs["value"], 2)
        self.app.progress.start.assert_not_called()

    def test_all_layers_full_bar_while_report_is_still_running(self):
        self.start()
        for number in range(1, 7):
            self.complete_layer(number)
        self.app.messages.put(dict(stage="report"))
        self.poll(80)
        self.assertEqual(self.app.progress_text.set.call_args.args[0], "6 / 6 層 (100%)")
        self.assertIn("HTMLレポート作成中", self.app.status.set.call_args.args[0])
        self.app.activity.stop.assert_not_called()
        self.app.messages.put(dict(stage="finished", outdir="result",
                                   report=dict(layers=[{}] * 6, violation_count=5, error_count=0)))
        self.poll(90)
        self.assertFalse(self.app.running)
        self.app.activity.stop.assert_called_once()
        self.app.activity.grid_remove.assert_called_once()
        self.assertEqual(self.app.result_path, Path("result/index.html"))

    def test_failure_stops_activity_and_retains_completed_layers(self):
        self.start()
        self.complete_layer(1)
        self.app.messages.put(dict(stage="failed", error="保存できません"))
        with patch("pcb_clearance.gui.messagebox.showerror"):
            self.poll(20)
        self.assertFalse(self.app.running)
        self.assertEqual(self.app.progress_text.set.call_args.args[0], "1 / 6 層 (17%)")
        self.app.activity.stop.assert_called_once()
        self.assertIn("検査に失敗", self.app.status.set.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
