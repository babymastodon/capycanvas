import importlib.util
from pathlib import Path
import unittest
import tempfile

spec = importlib.util.spec_from_file_location("profile_report", Path(__file__).with_name("windows-ui-motion-profile.py"))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class ProfileTests(unittest.TestCase):
    def test_marker_guid_events_correlate_to_both_qpc_write_brackets(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"events.csv"
            path.write_text("\n".join(f"UnknownEvent/Crimson,{stamp},windows-ui-motion.exe (10180),2260,1,{{75de640b-8863-4535-b326-443c2307f3e0}}" for stamp in (3493817,9518171)))
            meta = {"qpc_frequency":10000000,"motion_start_qpc":2445276494,"motion_end_qpc":2505276494}
            markers = {"begin":[2445276227,2445276473],"end":[2505519617,2505520011]}
            actual = report.contact_range(path, markers, meta)
            self.assertEqual(actual["event_timestamp_units"],"us")
            self.assertAlmostEqual(actual["start"],3.493832)
            self.assertAlmostEqual(actual["end"]-actual["start"],6.)
            self.assertAlmostEqual(actual["uncertainty_us"],12.7)
            markers["end"][0]+=1000000;markers["end"][1]+=1000000
            with self.assertRaises(ValueError):
                report.contact_range(path, markers, meta)

    def tables(self):
        return {"TblP": [["process", "pid", "hits"], ["CapyCanvas.exe", "42", "4"]],
                "TblME": [["module", "hits"], ["Microsoft.UI.Xaml.dll", "3"], ["CapyCanvas.exe", "1"]],
                "TblSI": [["function", "inclusive", "percent", "exclusive", "base"],
                          ["CapyCanvas.exe!resolveLayout", "4", "100%", "1", "0x10"],
                          ["Microsoft.UI.Xaml.dll!Measure", "3", "75%", "3", "0x20"]]}

    def test_disjoint_module_weights_and_overlapping_inclusive_functions(self):
        result = report.stack_summary(self.tables(), 42, 8., 2, 4)
        self.assertEqual(result["module_groups"]["Microsoft.UI.Xaml.dll"]["cpu_ms"], 6.)
        self.assertEqual(result["inclusive_functions"][0]["cpu_ms"], 8.)
        self.assertEqual(result["inclusive_functions"][0]["ms_per_input"], 4.)
        self.assertEqual(sum(row["cpu_ms"] for row in result["module_groups"].values()), 8.)
        self.assertEqual(result["resolved_leaf_share"], 1.)

    def test_wrong_event_or_thread_report_cannot_be_cpu_attribution(self):
        for process_id, count in ((99, 4), (42, 8), (42, 0)):
            with self.assertRaises(ValueError):
                report.stack_summary(self.tables(), process_id, 8., 2, count)

    def test_small_missing_stack_share_is_explicit_not_renormalized(self):
        tables = self.tables()
        tables["TblP"][1][2] = "99"
        tables["TblME"][1][1] = "98"
        tables["TblSI"][2][3] = "98"
        result = report.stack_summary(tables, 42, 100., 2, 100)
        self.assertEqual(result["missing_stack_samples"], 1)
        self.assertEqual(result["module_groups"]["unattributed stack"]["cpu_ms"], 1.)
        self.assertEqual(sum(row["cpu_ms"] for row in result["module_groups"].values()), 100.)

    def test_nonpositive_or_nonfinite_cpu_is_invalid(self):
        for cpu in (0., -1., float("nan"), float("inf")):
            with self.assertRaises(ValueError):
                report.stack_summary(self.tables(), 42, cpu, 2, 4)

    def test_html_parser_selects_disjoint_and_uni_inclusive_tables(self):
        reader = report.StackTables()
        reader.feed("<a id='TblME'></a><table><tr><th>module</th><th>hits</th></tr><tr><td>CapyCanvas.exe</td><td>4</td></tr></table>"
                    "<a id='TblMI'></a><table><tr><td>overlap</td></tr></table>"
                    "<a id='TblSI'></a><table><tr><td>CapyCanvas.exe!<a href='#f'>resolveLayout</a></td><td>4</td></tr></table>")
        self.assertEqual(set(reader.tables), {"TblME", "TblSI"})
        self.assertEqual(reader.tables["TblSI"][0], ["CapyCanvas.exe!resolveLayout", "4"])


if __name__ == "__main__":
    unittest.main()
