"""既知寸法の合成ガーバーで極性・単位・境界値・出力を検証する。"""
import csv
import json
import math
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from shapely.geometry import Point, Polygon, box
from shapely.ops import unary_union

from pcb_clearance.cli import discover, main, run_check
from pcb_clearance.detector import find_gaps
from pcb_clearance.geometry import macro_shape
from pcb_clearance.parser import GerberError, GerberParser


def gerber(body, definitions="%ADD10R,1X1*%", unit="MM", fs="LAX44Y44", attrs=""):
    return f"%FS{fs}*%\n%MO{unit}*%\n{attrs}\n{definitions}\nG01*\nD10*\n{body}\nM02*\n"


class ParserTests(unittest.TestCase):
    def test_clear_polarity_is_ordered(self):
        text = gerber("X0Y0D03*%LPC*%D11*X0Y0D03*%LPD*%D12*X0Y0D03*",
                      "%ADD10R,4X4*%%ADD11R,2X2*%%ADD12R,1X1*%")
        copper = GerberParser(text).parse().copper
        self.assertAlmostEqual(copper.area, 13)
        self.assertTrue(copper.covers(Point(0, 0)))
        self.assertFalse(copper.covers(Point(0.75, 0)))

    def test_negative_file_polarity_comment_and_ip(self):
        text = gerber("X0Y0D03*", attrs="G04 #@! TF.FilePolarity,Negative*")
        layer = GerberParser(text).parse()
        self.assertTrue(layer.statistics["negative"])
        self.assertFalse(layer.copper.covers(Point(0, 0)))
        self.assertTrue(layer.copper.covers(Point(1, 0)))
        both = GerberParser(text.replace("G01*", "%IPNEG*%G01*")).parse()
        self.assertFalse(both.statistics["negative"])

    def test_inch_aperture_and_coordinates(self):
        layer = GerberParser(gerber("X10000Y20000D03*", unit="IN")).parse()
        self.assertEqual(tuple(round(v, 4) for v in layer.copper.bounds), (12.7, 38.1, 38.1, 63.5))

    def test_trailing_zero_suppression_and_modal_operations(self):
        layer = GerberParser(gerber("X1Y2D03*X2*", fs="TAX24Y24")).parse()
        self.assertEqual(layer.statistics["flashes"], 2)
        self.assertEqual(layer.copper.bounds, (9.5, 19.5, 20.5, 20.5))

    def test_standard_aperture_hole_is_transparent(self):
        layer = GerberParser(gerber("X0Y0D03*", "%ADD10C,2X1*%")).parse()
        self.assertFalse(layer.copper.covers(Point(0, 0)))
        self.assertTrue(layer.copper.covers(Point(0.75, 0)))

    def test_rectangular_sweep_retains_square_ends(self):
        layer = GerberParser(gerber("X0Y0D02*X20000Y10000D01*", "%ADD10R,2X1*%")).parse()
        self.assertEqual(layer.copper.bounds, (-1, -0.5, 3, 1.5))
        self.assertAlmostEqual(layer.copper.area, 6)
        self.assertTrue(layer.copper.covers(Point(-1, -0.5)))

    def test_multiple_region_contours_are_unioned(self):
        body = ("G36*X0Y0D02*X40000D01*Y40000D01*X0D01*Y0D01*"
                "X10000Y10000D02*X20000D01*Y20000D01*X10000D01*Y10000D01*D02*G37*")
        layer = GerberParser(gerber(body)).parse()
        self.assertAlmostEqual(layer.copper.area, 16)

    def test_kicad_cut_in_preserves_hole(self):
        body = ("G36*X0Y0D02*X40000D01*Y40000D01*X0D01*Y20000D01*"
                "X10000D01*Y10000D01*X30000D01*Y30000D01*X10000D01*Y20000D01*"
                "X0D01*Y0D01*G37*")
        layer = GerberParser(gerber(body)).parse()
        self.assertAlmostEqual(layer.copper.area, 12)
        self.assertFalse(layer.copper.covers(Point(2, 2)))

    def test_arc_full_circle_and_single_quadrant(self):
        for commands in ("G75*G03*X10000Y0D02*X10000Y0I-10000J0D01*",
                         "G74*G03*X10000Y0D02*X0Y10000I10000J0D01*"):
            layer = GerberParser(gerber(commands, "%ADD10C,0.2*%")).parse()
            self.assertEqual(layer.statistics["arcs"], 1)
            self.assertGreater(layer.copper.area, 0.3)

    def test_macro_thermal_has_four_gaps(self):
        shape = macro_shape("thermal", ["7,0,0,2,1.84,0.15,45"], [])
        self.assertFalse(shape.covers(Point(0, 0)))
        self.assertTrue(shape.covers(Point(0.95, 0)))
        layer = GerberParser(gerber("X0Y0D03*", "%AMT*7,0,0,2,1.84,0.15,45*%%ADD10T*%",
                                  attrs="%TF.FilePolarity,Negative*%")).parse()
        found, _ = find_gaps(layer.copper, negative=True)
        self.assertTrue(found)
        self.assertAlmostEqual(min(r["gap_mm"] for r in found), 0.08, delta=0.0001)

    def test_macro_variables_clear_and_aperture_transform(self):
        defs = "%AMT*$2=$1/2*21,1,$1,$1,0,0,0*21,0,$2,$2,0,0,0*%%ADD10T,2*%"
        layer = GerberParser(gerber("%LMX*%%LR90*%%LS2*%X0Y0D03*", defs)).parse()
        self.assertAlmostEqual(layer.copper.area, 12)

    def test_unhandled_commands_fail_closed(self):
        for command in ("%SRX2Y2I10J10*%", "%ABD11*%", "%OFA1B1*%", "G91*", "G12*"):
            with self.subTest(command=command), self.assertRaises(GerberError):
                GerberParser(gerber(command + "X0Y0D03*")).parse()

    def test_missing_end_and_open_region_fail(self):
        for text in (gerber("X0Y0D03*").replace("M02*", ""),
                     gerber("G36*X0Y0D02*X10000Y0D01*X10000Y10000D01*G37*")):
            with self.assertRaises(GerberError):
                GerberParser(text).parse()


class DetectorTests(unittest.TestCase):
    def test_below_equal_and_above_threshold(self):
        for gap, expected in ((0.08, True), (0.1, True), (0.10001, False), (0.12, False)):
            copper = unary_union([box(0, 0, 1, 2), box(1 + gap, 0, 2 + gap, 2)])
            found, _ = find_gaps(copper)
            with self.subTest(gap=gap):
                self.assertEqual(bool(found), expected)
                if found:
                    self.assertAlmostEqual(found[0]["gap_mm"], gap)

    def test_same_copper_slit(self):
        copper = Polygon([(0, 0), (3, 0), (3, 3), (1.6, 3), (1.6, 1), (1.5, 1), (1.5, 3), (0, 3)])
        found, _ = find_gaps(copper)
        self.assertTrue(found)
        self.assertEqual(found[0]["kind"], "self_approach")
        self.assertAlmostEqual(found[0]["gap_mm"], 0.1)

    def test_copper_bridge_is_not_insulating_gap(self):
        copper = box(0, 0, 4, 4).difference(unary_union([box(1, 1, 1.9, 3), box(2, 1, 3, 3)]))
        found, _ = find_gaps(copper)
        self.assertFalse(found)

    def test_corner_filter_and_short_slit_option(self):
        copper = Polygon([(0, 0), (3, 0), (3, 3), (1.6, 3), (1.6, 2.9), (1.5, 2.9), (1.5, 3), (0, 3)])
        filtered, stats = find_gaps(copper)
        unfiltered, _ = find_gaps(copper, arc_filter=False)
        self.assertFalse(filtered)
        self.assertGreater(stats["corner_filtered_pairs"], 0)
        self.assertTrue(unfiltered)

    def test_circular_pads_gap(self):
        copper = unary_union([Point(0, 0).buffer(0.5, quad_segs=64), Point(1.08, 0).buffer(0.5, quad_segs=64)])
        found, _ = find_gaps(copper)
        self.assertAlmostEqual(found[0]["gap_mm"], 0.08)


class CLITests(unittest.TestCase):
    def test_shared_engine_progress_without_console(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "board.GTL"
            source.write_text(gerber("X0Y0D03*X10800Y0D03*"))
            events = []
            with patch("sys.stderr", None):
                report, outdir = run_check([str(source)], root / "out", jobs=1, on_progress=events.append)
            self.assertEqual(report["status"], "violations")
            self.assertEqual([e["stage"] for e in events], ["started", "layer_done", "report"])
            self.assertEqual(events[1]["completed"], 1)
            self.assertTrue((outdir / "index.html").is_file())

    def test_shared_engine_rejects_invalid_threshold_before_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for threshold in (0, -1, math.nan, math.inf):
                with self.subTest(threshold=threshold), self.assertRaises(GerberError):
                    run_check(["missing.GTL"], root / "out", threshold=threshold)
            self.assertFalse((root / "out").exists())

    def test_report_coordinates_images_and_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inputs = root / "input"
            inputs.mkdir()
            for ext, distance in (("GTL", 11000), ("GBL", 12000)):
                (inputs / f"board.{ext}").write_text(gerber(f"X0Y0D03*X{distance}Y0D03*"))
            (inputs / "board.GTS").write_text(gerber("X0Y0D03*"))
            target = root / "result"
            self.assertEqual(main([str(inputs), "--outdir", str(target), "-j", "2"]), 1)
            report = json.loads((target / "report.json").read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "violations")
            self.assertEqual(len(report["layers"]), 2)
            self.assertEqual(len(report["skipped_inputs"]), 1)
            records = [r for layer in report["layers"] for r in layer["violations"]]
            self.assertTrue(records)
            for rec in records:
                self.assertTrue((target / rec["image"]).is_file())
                self.assertAlmostEqual(rec["gap_mm"], math.dist(rec["point_a_mm"], rec["point_b_mm"]))
            self.assertTrue((target / "index.html").is_file())
            with (target / "violations.csv").open(encoding="utf-8-sig", newline="") as stream:
                self.assertEqual(len(list(csv.DictReader(stream))), len(records))
            self.assertEqual(main([str(inputs / "board.GBL"), "--outdir", str(root / "clear"), "-j", "1"]), 0)
            self.assertEqual(main([str(inputs), "--outdir", str(target)]), 2)

    def test_bad_known_copper_is_an_error_not_a_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inputs = root / "in"
            inputs.mkdir()
            (inputs / "bad.GTL").write_text("bad")
            target = root / "out"
            self.assertEqual(main([str(inputs), "--outdir", str(target)]), 2)
            report = json.loads((target / "report.json").read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "error")
            self.assertEqual(report["error_count"], 1)

    def test_x2_overrides_extension_and_kicad_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "board.GTL").write_text(gerber("X0Y0D03*", attrs="%TF.FileFunction,Soldermask,Top*%"))
            (root / "board-In1_Cu.gbr").write_text(gerber("X0Y0D03*"))
            (root / "generic.gbr").write_text(gerber("X0Y0D03*", attrs="G04 #@! TF.FileFunction,Copper,L2,Inr*"))
            files, _ = discover([str(root)])
            self.assertEqual({p.name for p in files}, {"board-In1_Cu.gbr", "generic.gbr"})


if __name__ == "__main__":
    unittest.main()
