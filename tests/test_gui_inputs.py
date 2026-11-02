"""フォルダー追加時に、表示一覧と実際の検査ファイルが一致することを確認する。"""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from pcb_clearance.gui import ClearanceApp


GERBER = "%FSLAX44Y44*%%MOMM*%%ADD10C,1*%D10*X0Y0D03*M02*"


class InputTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        for extension in ("GTL", "GBL", "GTS", "GTO"):
            (self.folder / f"board.{extension}").write_text(GERBER)
        self.app = ClearanceApp.__new__(ClearanceApp)
        self.app.root = Mock()
        self.app.inputs = []
        self.app.listbox = Mock()
        self.app.status = Mock()

    def test_folder_expands_to_copper_files_in_display_and_inputs(self):
        self.app.add_inputs([str(self.folder)])
        expected = sorted(str((self.folder / f"board.{ext}").resolve()) for ext in ("GTL", "GBL"))
        self.assertEqual(self.app.inputs, expected)
        displayed = [call.args[1] for call in self.app.listbox.insert.call_args_list]
        self.assertEqual(displayed, expected)
        self.assertIn("2 ファイル", self.app.status.set.call_args.args[0])

    def test_repeated_folder_and_file_additions_do_not_duplicate_rows(self):
        self.app.add_inputs([str(self.folder)])
        self.app.add_inputs([str(self.folder)])
        self.app.add_inputs([str(self.folder / "board.GTL")])
        self.assertEqual(len(self.app.inputs), 2)
        self.assertEqual(self.app.listbox.insert.call_count, 2)

    def test_individual_file_from_folder_can_be_removed(self):
        self.app.add_inputs([str(self.folder)])
        self.app.listbox.curselection.return_value = (0,)
        self.app.remove_selected()
        self.assertEqual(self.app.inputs, [str((self.folder / "board.GTL").resolve())])

    def test_empty_folder_reports_problem_without_losing_selection(self):
        self.app.add_inputs([str(self.folder / "board.GTL")])
        empty = self.folder / "empty"
        empty.mkdir()
        with patch("pcb_clearance.gui.messagebox.showwarning") as warning:
            self.app.add_inputs([str(empty)])
        warning.assert_called_once()
        self.assertEqual(self.app.inputs, [str((self.folder / "board.GTL").resolve())])
        self.assertEqual(self.app.listbox.insert.call_count, 1)


if __name__ == "__main__":
    unittest.main()
