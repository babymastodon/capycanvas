import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location("poc_report", Path(__file__).with_name("report.py"))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class ReportTests(unittest.TestCase):
    def test_drag_matching_never_crosses_a_monotonic_half_period(self):
        inputs = [{"qpc": 0, "segment": 0, "expected_value": 0}, {"qpc": 100, "segment": 0, "expected_value": 50}, {"qpc": 200, "segment": 0, "expected_value": 100}, {"qpc": 300, "segment": 1, "expected_value": 100}, {"qpc": 400, "segment": 1, "expected_value": 20}, {"qpc": 500, "segment": 1, "expected_value": 0}]
        frames = [{"qpc": 150, "divider_y": 50}, {"qpc": 450, "divider_y": 50}, {"qpc": 490, "divider_y": 0}]
        values, censored, unmatched = report.latency_drag(frames, inputs, "divider_y", 1000)
        self.assertEqual(values, [50])
        self.assertEqual(censored, 1)
        self.assertEqual(unmatched, 1)

    def test_drag_matching_uses_latest_earlier_input(self):
        inputs = [{"qpc": 0, "segment": 0, "expected_value": 0}, {"qpc": 90, "segment": 0, "expected_value": 50}, {"qpc": 100, "segment": 0, "expected_value": 50}, {"qpc": 200, "segment": 0, "expected_value": 100}]
        values, _, _ = report.latency_drag([{"qpc": 130, "divider_y": 50}], inputs, "divider_y", 1000)
        self.assertEqual(values, [30])

    def test_absent_drag_probes_are_counted_as_unmatched_within_the_segment(self):
        inputs = [{"qpc": 0, "segment": 0, "expected_value": 0}, {"qpc": 100, "segment": 0, "expected_value": 50}, {"qpc": 200, "segment": 0, "expected_value": 100}]
        frames = [{"qpc": -10, "width": None}, {"qpc": 110, "width": None}, {"qpc": 130, "width": 50}, {"qpc": 210, "width": None}]
        values, censored, unmatched = report.latency_drag(frames, inputs, "width", 1000)
        self.assertEqual((values, censored, unmatched), ([30], 0, 1))

    def test_counter_wrap_counts_observed_changes_only(self):
        frames = [{"qpc": index, "ui_frame": value} for index, value in enumerate([254, 255, 0, 0, 2])]
        self.assertEqual(report.changes(frames, "ui_frame", 0, 4), [1, 2, 4])

    def test_absent_probe_is_not_a_zero_coordinate(self):
        frames = [{"qpc": 0, "drawer_x": None}, {"qpc": 1, "drawer_x": 0}, {"qpc": 2, "drawer_x": -1}]
        self.assertEqual(report.changes(frames, "drawer_x", 0, 2), [2])

    def contact(self, directory, **overrides):
        meta = {"arm": "canvas-only", "workload": "paint-still", "profile": "normal", "contact": 0, "qpc_frequency": 1000, "launch_qpc": 100, "motion_start_qpc": 1000, "motion_end_qpc": 2000, "scale": 1.5, "refresh_hz": 60, "logical_cpus": 4, "no_capture": False, "vm": "capycanvas6-poc-warp"}
        meta.update(overrides)
        (directory / "meta.json").write_text(json.dumps(meta))
        (directory / "process.jsonl").write_text('\n'.join(json.dumps(row) for row in [{"qpc": 1000, "cpu_ms": 0, "private_bytes": 1048576}, {"qpc": 2000, "cpu_ms": 400, "private_bytes": 2097152}]))
        (directory / "frames.jsonl").write_text('\n'.join(json.dumps(row) for row in [{"qpc": 200, "last_present_qpc": 150, "ui_marker": 1, "ui_frame": 1, "state16": 0, "pen_samples": 0}, {"qpc": 1200, "last_present_qpc": 1150, "ui_marker": 1, "ui_frame": 1, "state16": 0, "pen_samples": 1}, {"qpc": 1300, "last_present_qpc": 1250, "ui_marker": 1, "ui_frame": 1, "state16": 0, "pen_samples": 2}]))
        (directory / "canvas.jsonl").write_text(json.dumps({"kind": "startup", "milestone": "first_canvas_present", "qpc": 140}) + '\n')
        (directory / "inputs.jsonl").write_text('\n'.join(json.dumps(row) for row in [{"qpc": 1100, "kind": "pen", "index": 0}, {"qpc": 1200, "kind": "pen", "index": 1}, {"qpc": 1400, "kind": "pen", "index": 2}]))
        return report.analyse_contact(directory)

    def test_display_mode_preserves_monitor_resolution_instead_of_client_size(self):
        mode = {"width": 2560, "height": 1600, "scale": 1.5, "refresh_hz": 150, "desktop_rect": [0, 0, 2560, 1600], "device": "DISPLAY1"}
        for no_capture in (False, True):
            with tempfile.TemporaryDirectory() as temporary:
                row = self.contact(Path(temporary), width=2400, height=1500, display_mode=mode, no_capture=no_capture)
            self.assertEqual(row["display_mode"], mode)
        with tempfile.TemporaryDirectory() as temporary:
            row = self.contact(Path(temporary), width=2400, height=1500)
        self.assertIsNone(row["display_mode"])

    def test_ink_matches_consumed_threshold_and_counts_unmatched(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.contact(Path(temporary))
        self.assertEqual(row["ink_latency_ms"]["p99"], 50)
        self.assertEqual(row["ink_latency_ms"]["count"], 2)
        self.assertEqual(row["ink_unmatched"], 1)
        self.assertEqual(row["startup_ui_ms"], 50)
        self.assertEqual(row["process_cpu_percent"], 10)

    def test_cursor_only_frames_do_not_count_as_desktop_updates(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.contact(directory)
            with (directory / "frames.jsonl").open('a') as stream:
                stream.write('\n'+json.dumps({"qpc": 1100, "last_present_qpc": 0, "pen_samples": 1}))
            row = report.analyse_contact(directory)
        self.assertEqual(row["ink_latency_ms"]["p50"], 50)

    def test_pool_percentiles_from_samples_not_contact_percentiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.contact(Path(temporary))
        one, two = dict(row, valid=True, validation_errors=[]), dict(row, valid=True, validation_errors=[])
        one["_samples"] = {"ink_latency_ms": [0] * 99 + [100]}
        two["_samples"] = {"ink_latency_ms": [200]}
        pooled = report.aggregate([one, two])[0]
        self.assertEqual(pooled["ink_latency_ms"]["p99"], 100)
        self.assertEqual(pooled["contacts"], 2)

    def test_missing_presentmon_does_not_claim_zero_idle_presents(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.contact(Path(temporary), workload="startup", motion_start_qpc=None, motion_end_qpc=None, idle_start_qpc=5000, idle_end_qpc=15000, presentmon_unavailable_reason="single-present baseline")
        self.assertIsNone(row["idle_presents"])
        self.assertIsNone(row["motion_rate_hz"])
        self.assertEqual(row["presentmon_unavailable_reason"], "single-present baseline")
        self.assertIn("idle presents remain unverified", report.render(report.aggregate([row])))

    def test_abba_order_requires_three_contacts_in_each_slot(self):
        rows = []
        for slot, arm in enumerate(("retained", "winui", "winui", "retained")):
            for contact in range(3):
                rows.append({"vm": "test", "profile": "normal", "workload": "divider", "no_capture": False, "arm": arm, "order_slot": slot, "motion_duration_s": 1, "motion_changes": 60})
        self.assertTrue(report.abba_checks(rows)[0]["order_valid"])
        self.assertFalse(report.abba_checks(rows[:-1])[0]["order_valid"])

    def test_capture_overhead_requires_identified_canvas_chain(self):
        rows = [{"vm": "test", "profile": "normal", "arm": "retained", "workload": "paint-still", "no_capture": value, "canvas_presentmon_display_hz": {"p50": rate}} for value, rate in ((False, 90), (True, 100))]
        self.assertAlmostEqual(report.overhead_checks(rows)[0]["overhead_fraction"], .1)
        rows[0]["canvas_presentmon_display_hz"]["p50"] = None
        self.assertIsNone(report.overhead_checks(rows)[0]["overhead_fraction"])

    def test_ui_thread_cpu_uses_cumulative_delta_inside_the_measured_range(self):
        trace = [{"kind": "thread_cpu", "thread": "ui", "qpc": tick, "cpu_ms": cpu} for tick, cpu in ((0, 100), (100, 110), (200, 130), (300, 500))]
        frames = [{"qpc": tick, "ui_frame": frame} for tick, frame in ((50, 0), (100, 1), (150, 2), (200, 3), (300, 4))]
        self.assertEqual(report.ui_cpu_per_change(trace, frames, 100, 200), 20 / 3)

    def test_idle_presents_are_read_outside_the_motion_window(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.contact(directory, idle_start_qpc=5000, idle_end_qpc=15000)
            (directory / "presents.csv").write_text("QPCTime,PresentMode,SwapChainAddress,MsBetweenPresents,MsBetweenDisplayChange,MsUntilDisplayed\n5500,Composed: Flip,0x123,16,16,2\n")
            row = report.analyse_contact(directory)
        self.assertEqual(row["idle_presents"], 1)

    def test_wheel_completion_never_treats_wheel_delta_as_a_position(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.contact(directory, workload="scroll-wheel")
            (directory / "inputs.jsonl").write_text(json.dumps({"qpc": 1100, "kind": "wheel", "index": 0, "expected_value": -120}))
            (directory / "frames.jsonl").write_text('\n'.join(json.dumps({"qpc": tick, "scroll_offset": value}) for tick, value in ((1000, 0), (1200, 10), (1300, 20), (1400, 20))))
            row = report.analyse_contact(directory)
        self.assertEqual(row["completion_latency_ms"]["p50"], 200)
        self.assertEqual(row["completion_target_basis"], "observed_final_offset")

    def test_startup_excludes_blank_pre_present_marker_decodes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.contact(directory, workload="startup", motion_start_qpc=None, motion_end_qpc=None)
            frames = [{"qpc": 120, "last_present_qpc": 110, "ui_marker": 0, "pen_samples": 0}, {"qpc": 180, "last_present_qpc": 170, "ui_marker": 1, "pen_samples": 0}, {"qpc": 230, "last_present_qpc": 220, "ui_marker": 1, "pen_samples": 0}]
            (directory / "frames.jsonl").write_text('\n'.join(json.dumps(row) for row in frames))
            (directory / "canvas.jsonl").write_text(json.dumps({"kind": "startup", "milestone": "first_canvas_present", "qpc": 200}) + '\n')
            row = report.analyse_contact(directory)
        self.assertEqual(row["startup_ui_ms"], 70)
        self.assertEqual(row["startup_canvas_ms"], 120)

    def test_canvas_startup_is_unmeasured_without_a_present_milestone(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.contact(directory)
            (directory / "canvas.jsonl").unlink()
            row = report.analyse_contact(directory)
        self.assertIsNone(row["startup_canvas_ms"])

    def test_probe_calibration_subtracts_constant_offset_before_matching(self):
        meta = {"workload": "divider", "calibration_start_qpc": 100, "calibration_end_qpc": 500, "calibration_expected_value": 100}
        rest = [{"qpc": tick, "last_present_qpc": 0, "divider_y": value} for tick, value in ((150, 108), (200, 108), (250, 108))]
        calibration = report.calibrate_probe(meta, rest, "divider_y")
        self.assertEqual(calibration["offset_px"], 8)
        inputs = [{"qpc": 500, "segment": 0, "expected_value": 100}, {"qpc": 600, "segment": 0, "expected_value": 150}, {"qpc": 700, "segment": 0, "expected_value": 200}]
        values, _, unmatched = report.latency_drag([{"qpc": 650, "divider_y": 158}], inputs, "divider_y", 1000, calibration["offset_px"])
        self.assertEqual(values, [50])
        self.assertEqual(unmatched, 0)

    def test_probe_calibration_accepts_one_pixel_jitter_but_fails_above_it(self):
        meta = {"workload": "resize-native", "calibration_start_qpc": 100, "calibration_end_qpc": 500, "calibration_expected_value": 1600}
        frames = [{"qpc": tick, "width": value} for tick, value in ((150, 1598), (200, 1598.5), (250, 1599))]
        calibration = report.calibrate_probe(meta, frames, "width")
        self.assertEqual(calibration["status"], "passed")
        self.assertEqual(calibration["offset_px"], -1.5)
        self.assertEqual(calibration["spread_px"], 1)
        frames[-1]["width"] = 1599.01
        self.assertEqual(report.calibrate_probe(meta, frames, "width")["status"], "failed")
        self.assertEqual(report.calibrate_probe(meta, [], "width")["status"], "failed")

    def test_capture_adequacy_thresholds_are_strict(self):
        meta = {"qpc_frequency": 1000, "refresh_hz": 120}
        capture = {"capture_start_qpc": 100, "capture_end_qpc": 1100, "desktop_frames": 98, "missing_desktop_frames": 2}
        frames = [{"capture_ms": 500 / 120} for _ in range(20)]
        quality = report.capture_adequacy(meta, frames, capture)
        self.assertEqual(quality["status"], "adequate")
        self.assertEqual(quality["missing_share"], .02)
        self.assertEqual(quality["desktop_rate_hz"], 98)
        capture["missing_desktop_frames"] = 3
        self.assertTrue(report.capture_adequacy(meta, frames, capture)["capture_limited"])
        capture["missing_desktop_frames"] = 2
        frames[-1]["capture_ms"] += 1
        self.assertTrue(report.capture_adequacy(meta, frames, capture)["capture_limited"])

    def test_limited_run_flags_survive_abba_and_overhead_comparisons(self):
        rows = []
        for slot, arm in enumerate(("retained", "winui", "winui", "retained")):
            for contact in range(3):
                rows.append({"vm": "test", "profile": "normal", "workload": "divider", "no_capture": False, "arm": arm, "order_slot": slot, "motion_duration_s": 1, "motion_changes": 60, "capture_quality": {"status": "capture-limited" if slot == 0 else "adequate"}})
        drift = report.abba_checks(rows)[0]["drift"]["retained"]
        self.assertEqual(drift["early_capture_status"], "capture-limited")
        self.assertEqual(drift["late_capture_status"], "adequate")
        self.assertEqual(drift["comparison_capture_status"], "capture-limited")
        paired = [{"vm": "test", "profile": "normal", "arm": "retained", "workload": "divider", "no_capture": off, "canvas_presentmon_display_hz": {"p50": hz}, "capture_quality": {"status": status}} for off, hz, status in ((False, 90, "capture-limited"), (True, 100, "capture-off"))]
        comparison = report.overhead_checks(paired)[0]
        self.assertEqual(comparison["capture_on_status"], "capture-limited")
        self.assertEqual(comparison["capture_off_status"], "capture-off")
        self.assertEqual(comparison["comparison_capture_status"], "capture-limited")

    def test_failed_calibration_writes_evidence_and_exits_nonzero(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / 'contact-000'
            directory.mkdir()
            self.contact(directory, workload="divider", calibration_start_qpc=100, calibration_end_qpc=1000, calibration_expected_value=100)
            (directory / "frames.jsonl").write_text('\n'.join(json.dumps({"qpc": tick, "last_present_qpc": 0, "divider_y": value}) for tick, value in ((200, 108), (300, 110))))
            result = subprocess.run([sys.executable, str(Path(report.__file__).resolve()), str(root), '--out', str(root / 'reports')], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            saved = json.loads((root / 'reports/results.json').read_text())
            self.assertFalse(saved['contacts'][0]['valid'])
            self.assertEqual(saved['contacts'][0]['probe_calibration']['spread_px'], 2)
            self.assertIn('Probe calibration failed', result.stderr)

    def delivery_fixture(self, directory, evidence=None):
        self.contact(directory)
        inputs = [{"qpc": tick, "timestamp": tick, "kind": "pen", "index": index, "x": 10 + index, "y": 20, "pressure": .5, "tilt_x": 10, "tilt_y": -10, "twist": 30} for index, tick in enumerate((1100, 1200, 1400))]
        trace = [{"kind": "startup", "milestone": "first_canvas_present", "qpc": 140}]
        for sample in inputs:
            index = sample['index']
            trace.append({"kind": "input_receipt", "index": index, "timestamp": 9000 + index, "qpc": sample['qpc'] + 25})
            consumed = {key: sample[key] for key in ('x', 'y', 'pressure', 'tilt_x', 'tilt_y', 'twist')}
            consumed['timestamp'] = 9000 + index
            trace.append({"kind": "pen_consumed", "index": index, "pen_samples": index + 1, "sample": consumed, "qpc": sample['qpc'] + 40})
        if evidence is None:
            evidence = {"validated": True, "injected": 3, "received": 3, "consumed": 3, "matching_basis": "ordered_indices_and_quantized_sample_tuples", "timestamp_identity": "rewritten", "rewritten_receipt_timestamps": 3, "rewritten_consumed_timestamps": 3}
        for name, rows in (('inputs.jsonl', inputs), ('canvas.jsonl', trace)):
            (directory / name).write_text('\n'.join(json.dumps(row) for row in rows))
        (directory / 'pen-delivery.json').write_text(json.dumps(evidence))
        return inputs, trace

    def test_rewritten_source_timestamps_use_validated_indices_and_injection_qpc(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.delivery_fixture(directory)
            row = report.analyse_contact(directory)
        self.assertEqual(row['input_receipt_latency_ms']['p99'], 25)
        self.assertEqual(row['input_receipt_latency_ms']['count'], 3)
        self.assertEqual(row['receipt_unmatched'], 0)
        self.assertEqual(row['receipt_matching']['basis'], 'source_index_validated_by_pen-delivery.json')
        self.assertEqual(row['receipt_matching']['timestamp_identity'], 'rewritten')
        self.assertEqual(row['receipt_matching']['rewritten_receipt_timestamps'], 3)

    def test_winrt_timestamp_units_remain_qualified_without_affecting_qpc_latency(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            evidence = {"validated": True, "injected": 3, "received": 3, "consumed": 3, "matching_basis": "ordered_indices_and_quantized_sample_tuples", "timestamp_identity": "not_comparable_different_units", "rewritten_receipt_timestamps": None, "rewritten_consumed_timestamps": None, "timestamp_comparable_samples": 0, "timestamp_noncomparable_samples": 3, "host_timestamp_bases": ["PointerPoint.Timestamp_microseconds"], "native_latest_identity": "rewritten", "native_latest_checked_samples": 2, "native_latest_rewritten_samples": 1}
            self.delivery_fixture(directory, evidence)
            row = report.analyse_contact(directory)
        self.assertEqual(row['input_receipt_latency_ms']['p99'], 25)
        self.assertEqual(row['receipt_matching']['timestamp_identity'], 'not_comparable_different_units')
        self.assertIsNone(row['receipt_matching']['rewritten_receipt_timestamps'])
        self.assertEqual(row['receipt_matching']['timestamp_comparable_samples'], 0)
        self.assertEqual(row['receipt_matching']['timestamp_noncomparable_samples'], 3)
        pooled = report.aggregate([row])[0]
        self.assertEqual(pooled['receipt_matching']['host_timestamp_bases'], ['PointerPoint.Timestamp_microseconds'])
        self.assertEqual(pooled['receipt_matching']['native_latest_identity'], ['rewritten'])
        self.assertEqual(pooled['receipt_matching']['native_latest_checked_samples'], 2)
        self.assertEqual(pooled['receipt_matching']['native_latest_rewritten_samples'], 1)
        self.assertIn('Latest native checked / rewritten', report.render([pooled]))
        self.assertIn('never by comparing WinRT microseconds with QPC ticks', report.render([pooled]))

    def test_frame_quality_excludes_startup_warmup_and_after_motion(self):
        for workload, field in (('divider', 'divider_y'), ('slowcanvas-divider', 'divider_y'), ('resize-native', 'width'), ('resize-programmatic', 'width')):
            with self.subTest(workload=workload), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.contact(directory, workload=workload)
                frames = []
                for tick, drift, black in ((200, 50, 20), (900, 30, 20), (1100, 10, 20), (1400, 2, 1), (1500, 0, 0), (2100, 100, 20)):
                    frames.append({'qpc': tick, 'last_present_qpc': tick, 'divider_y': 100, 'glass_y': 100+drift, 'fill_y': 100, 'width': 1600, 'black_samples': black})
                (directory / 'frames.jsonl').write_text('\n'.join(json.dumps(frame) for frame in frames))
                row = report.analyse_contact(directory)
                self.assertEqual(row['drift_max_px'], 2)
                self.assertEqual(row['drift_over_1_share'], .5)
                self.assertEqual(row['black_interior_frames'], 1)

    def test_absent_or_nonmotion_frame_quality_is_unobserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.contact(directory)
            (directory / 'frames.jsonl').write_text(json.dumps({'qpc': 1500, 'last_present_qpc': 1500, 'black_samples': None}))
            self.assertIsNone(report.analyse_contact(directory)['black_interior_frames'])
            self.contact(directory, workload='startup', motion_start_qpc=None, motion_end_qpc=None)
            with (directory / 'frames.jsonl').open('a') as stream:
                stream.write('\n'+json.dumps({'qpc': 1500, 'last_present_qpc': 1500, 'black_samples': 20, 'divider_y': 1, 'glass_y': 100, 'fill_y': 1}))
            row = report.analyse_contact(directory)
            self.assertIsNone(row['black_interior_frames'])
            self.assertIsNone(row['drift_max_px'])

    def test_count_only_delivery_evidence_never_enables_receipt_matching(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.delivery_fixture(directory, {"injected": 3, "received": 3, "consumed": 3})
            row = report.analyse_contact(directory)
        self.assertEqual(row['input_receipt_latency_ms']['count'], 0)
        self.assertEqual(row['receipt_unmatched'], 3)
        self.assertFalse(row['receipt_matching']['validated'])

    def test_duplicate_or_reordered_receipt_indices_invalidate_an_inconsistent_gate(self):
        for indices in ((0, 0, 2), (1, 0, 2)):
            with self.subTest(indices=indices), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                _, trace = self.delivery_fixture(directory)
                receipts = [record for record in trace if record.get('kind') == 'input_receipt']
                for record, index in zip(receipts, indices):
                    record['index'] = index
                (directory / 'canvas.jsonl').write_text('\n'.join(json.dumps(row) for row in trace))
                row = report.analyse_contact(directory)
                self.assertEqual(row['receipt_unmatched'], 3)
                self.assertEqual(row['input_receipt_latency_ms']['count'], 0)
                self.assertEqual(row['receipt_matching']['status'], 'inconsistent-gate-and-trace')

    def test_warmup_does_not_shift_validated_source_indices(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            inputs, _ = self.delivery_fixture(directory)
            inputs[0]['warmup'] = True
            (directory / 'inputs.jsonl').write_text('\n'.join(json.dumps(row) for row in inputs))
            row = report.analyse_contact(directory)
        self.assertEqual(row['input_receipt_latency_ms']['count'], 2)
        self.assertEqual(row['input_receipt_latency_ms']['p99'], 25)
        self.assertEqual(row['receipt_unmatched'], 0)

    def test_input_quality_uses_each_stream_nominal_rate_and_active_window(self):
        cases = (("pen", 240, 6, 1440), ("mouse", 240, 6, 1440), ("resize", 120, 6, 720), ("wheel", 60, 6, 360), ("key", 10, 6, 60), ("click", 2.5, 6, 15), ("touch", 250, .04, 10))
        for stream, nominal, seconds, count in cases:
            with self.subTest(stream=stream):
                evidence = {"streams": {stream: {"nominal_hz": nominal, "measured_seconds": seconds, "accepted_measured": count, "accepted_total": count + 10, "skipped_slots": 4, "skipped_measured_slots": 3, "terminal_samples": 1, "input_records": count + 11}}}
                quality = report.input_quality({"workload": "paint-still"}, evidence)
                self.assertEqual(quality['streams'][stream]['achieved_hz'], nominal)
                self.assertEqual(quality['status'], 'input-adequate')
                self.assertEqual(quality['streams'][stream]['skipped_measured_slots'], 3)
                self.assertEqual(quality['streams'][stream]['terminal_samples'], 1)

    def test_input_limited_threshold_is_strict_and_counts_override_claimed_rate(self):
        values = {"nominal_hz": 240, "measured_seconds": 6, "accepted_measured": 1296, "achieved_hz": 240}
        evidence = {"streams": {"pen": values}}
        quality = report.input_quality({"workload": "paint-still"}, evidence)
        self.assertFalse(quality['input_limited'])
        self.assertEqual(quality['streams']['pen']['achieved_hz'], 216)
        self.assertFalse(quality['streams']['pen']['recorded_rate_matches_counts'])
        values['accepted_measured'] = 1295
        self.assertTrue(report.input_quality({"workload": "paint-still"}, evidence)['input_limited'])
        self.assertEqual(report.input_quality({"workload": "startup"}, {"streams": {}})['status'], 'not-applicable')
        self.assertEqual(report.input_quality({"workload": "fake-motion"}, None)['status'], 'not-applicable')
        self.assertEqual(report.input_quality({"workload": "paint-still"}, {"streams": {}})['status'], 'input-unmeasured')

    def test_input_limit_flags_survive_abba_and_capture_overhead_comparisons(self):
        rows = []
        for slot, arm in enumerate(("retained", "winui", "winui", "retained")):
            for contact in range(3):
                rows.append({"vm": "test", "profile": "normal", "workload": "divider", "no_capture": False, "arm": arm, "order_slot": slot, "motion_duration_s": 1, "motion_changes": 60, "capture_quality": {"status": "adequate"}, "input_quality": {"status": "input-limited" if slot == 0 else "input-adequate"}})
        drift = report.abba_checks(rows)[0]['drift']['retained']
        self.assertEqual(drift['early_input_status'], 'input-limited')
        self.assertEqual(drift['late_input_status'], 'input-adequate')
        self.assertEqual(drift['comparison_input_status'], 'input-limited')
        paired = [{"vm": "test", "profile": "normal", "arm": "retained", "workload": "divider", "no_capture": off, "canvas_presentmon_display_hz": {"p50": hz}, "capture_quality": {"status": status}, "input_quality": {"status": inputs}} for off, hz, status, inputs in ((False, 90, "capture-limited", "input-limited"), (True, 100, "capture-off", "input-adequate"))]
        comparison = report.overhead_checks(paired)[0]
        self.assertEqual(comparison['comparison_input_status'], 'input-limited')
        self.assertEqual(comparison['comparison_capture_status'], 'capture-limited')
        self.assertEqual(report.rate_status(comparison['comparison_capture_status'], comparison['comparison_input_status']), 'capture-limited; input-limited')

    def test_observer_cpu_uses_recorded_final_deltas_without_summing_snapshots(self):
        records = [{"thread": "injection", "qpc": 100, "cpu_ms": 5, "wall_ms": 100, "priority": 15}, {"thread": "injection", "qpc": 200, "cpu_ms": 10, "wall_ms": 200, "priority": 15, "final": True}]
        values = report.observer_cpu(records)['injection']
        self.assertEqual(values['cpu_ms'], 10)
        self.assertEqual(values['one_core_percent'], 5)
        self.assertEqual(values['records'], 2)
        self.assertEqual(values['priority'], 15)
        self.assertTrue(values['final'])
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            first = self.contact(directory)
            first.update(valid=True, validation_errors=[])
            second = dict(first)
            first['observer_cpu'] = {'injection': values}
            second['observer_cpu'] = {'injection': dict(values, cpu_ms=20, wall_ms=100)}
            pooled = report.aggregate([first, second])[0]['observer_cpu']['injection']
        self.assertEqual(pooled['cpu_ms'], 30)
        self.assertEqual(pooled['wall_ms'], 300)
        self.assertEqual(pooled['one_core_percent'], 10)

    def valid_benchmark_contact(self, directory, workload='paint-still'):
        self.delivery_fixture(directory)
        meta = json.loads((directory / 'meta.json').read_text())
        meta['workload'] = workload
        (directory / 'meta.json').write_text(json.dumps(meta))
        complete = {'completed': True, 'closed_gracefully': True}
        run = {'exit_code': 0, 'presentmon_exit_code': 0, 'arm': meta['arm'], 'profile': meta['profile'], 'workload': workload}
        for name, value in (('completed.json', complete), ('run.json', run), ('done.json', {'ok': True, 'runs': [run]}), ('startup.json', {'first_ui_qpc': 150, 'first_canvas_qpc': 150, 'canvas_present_lower_bound_qpc': 140})):
            (directory / name).write_text(json.dumps(value))
        (directory / 'presents.csv').write_text('Application,ProcessID,SwapChainAddress,PresentMode,msBetweenPresents,msBetweenDisplayChange,msUntilDisplayed,QPCTime\ncanvas-only.exe,42,0x123,Composed: Flip,16,16,2,1150\n')
        return report.analyse_contact(directory)

    def test_actual_pinned_presentmon_lowercase_ms_headers_are_supported(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            row = self.valid_benchmark_contact(directory)
        self.assertTrue(row['valid'], row['validation_errors'])
        self.assertEqual(row['present_modes'], ['Composed: Flip'])
        self.assertEqual(row['present_display_intervals_ms']['p50'], 16)
        self.assertEqual(row['presentmon_swapchains']['0x123']['display_change_rate_hz'], 62.5)

    def test_failed_or_missing_envelopes_and_completion_invalidate_contacts(self):
        for filename, content in (('done.json', {'ok': False}), ('done.json', None), ('run.json', {'exit_code': 1}), ('run.json', None), ('completed.json', {'completed': False, 'closed_gracefully': True}), ('completed.json', None), ('failures.json', {'failures': [{'stage': 'data_gates', 'message': 'failed'}]})):
            with self.subTest(filename=filename, content=content), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.valid_benchmark_contact(directory)
                if content is None:
                    (directory / filename).unlink()
                else:
                    (directory / filename).write_text(json.dumps(content))
                row = report.analyse_contact(directory)
                self.assertFalse(row['valid'])
                pooled = report.aggregate([row])[0]
                self.assertEqual(pooled['failed_contacts'], 1)
                self.assertEqual(pooled['valid_contacts'], 0)
                self.assertIsNone(pooled['motion_rate_hz'])
                self.assertEqual(pooled['ink_latency_ms']['count'], 0)
                self.assertFalse(pooled['comparison_eligible'])

    def test_moving_run_requires_presentmon_but_startup_retains_explicit_exception(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.valid_benchmark_contact(directory)
            (directory / 'presents.csv').unlink()
            row = report.analyse_contact(directory)
            self.assertFalse(row['valid'])
            self.assertTrue(any('PresentMon' in error for error in row['validation_errors']))
            self.valid_benchmark_contact(directory, workload='startup')
            (directory / 'presents.csv').unlink()
            meta = json.loads((directory / 'meta.json').read_text())
            meta.update(presentmon_unavailable_reason='single-present interval baseline', motion_start_qpc=None, motion_end_qpc=None, idle_start_qpc=5000, idle_end_qpc=15000)
            (directory / 'meta.json').write_text(json.dumps(meta))
            row = report.analyse_contact(directory)
            self.assertTrue(row['valid'], row['validation_errors'])
            self.assertIsNone(row['idle_presents'])

    def test_csv_rows_do_not_hide_a_failed_collector_exit(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.valid_benchmark_contact(directory)
            path = directory / 'run.json'
            run = json.loads(path.read_text())
            run['presentmon_exit_code'] = 7
            path.write_text(json.dumps(run))
            row = report.analyse_contact(directory)
        self.assertFalse(row['valid'])
        self.assertTrue(any('collector exit' in error for error in row['validation_errors']))

    def test_invalid_contacts_are_excluded_from_pool_and_comparison_rates(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            valid = self.valid_benchmark_contact(directory)
        valid.update(motion_duration_s=1, motion_changes=60)
        invalid = dict(valid, valid=False, validation_errors=['PM failed'], motion_changes=900)
        pooled = report.aggregate([valid, invalid])[0]
        self.assertEqual(pooled['motion_rate_hz'], 60)
        self.assertEqual(pooled['valid_contacts'], 1)
        self.assertEqual(pooled['failed_contacts'], 1)
        self.assertFalse(pooled['comparison_eligible'])
        pair = [dict(pooled, no_capture=False, canvas_presentmon_display_hz={'p50': 60}), dict(pooled, no_capture=True, canvas_presentmon_display_hz={'p50': 100})]
        self.assertFalse(report.overhead_checks(pair)[0]['comparison_eligible'])
        self.assertIsNone(report.overhead_checks(pair)[0]['overhead_fraction'])

    def test_failed_batch_reports_diagnostics_but_exits_nonzero(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / 'contact-000'
            directory.mkdir()
            self.valid_benchmark_contact(directory)
            (directory / 'done.json').write_text(json.dumps({'ok': False}))
            result = subprocess.run([sys.executable, str(Path(report.__file__).resolve()), str(directory.parent), '--out', str(directory.parent / 'reports')], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('FAILED/excluded', result.stdout)
            saved = json.loads((directory.parent / 'reports/results.json').read_text())
            self.assertFalse(saved['contacts'][0]['valid'])
            self.assertEqual(saved['pooled'][0]['valid_contacts'], 0)

    def runtime_benchmark_contact(self, directory, workload='paint-still'):
        self.valid_benchmark_contact(directory, workload)
        for name in ('meta.json', 'run.json'):
            path = directory / name
            value = json.loads(path.read_text())
            value.update(vm='capycanvas6-poc-warp', presentmon_mode='runtime-only', presentmon_unavailable_reason=report.WARP_RUNTIME_REASON)
            if name == 'meta.json':
                value['process_id'] = 42
            path.write_text(json.dumps(value))
        run = json.loads((directory / 'run.json').read_text())
        (directory / 'done.json').write_text(json.dumps({'ok': True, 'runs': [run]}))
        (directory / 'presents.csv').write_text('Application,ProcessID,Runtime,SwapChainAddress,msBetweenPresents,QPCTime\ncanvas-only.exe,42,DXGI,0x123,16,1150\n')
        return report.analyse_contact(directory)

    def test_approved_warp_runtime_only_keeps_display_measurements_null(self):
        with tempfile.TemporaryDirectory() as temporary:
            row = self.runtime_benchmark_contact(Path(temporary))
        self.assertTrue(row['valid'], row['validation_errors'])
        self.assertTrue(row['presentmon_runtime_only_approved'])
        self.assertEqual(row['presentmon_display_status'], 'unverified')
        self.assertEqual(row['runtime_present_rows'], 1)
        self.assertIsNone(row['present_modes'])
        self.assertIsNone(row['canvas_presentmon_display_hz'])
        self.assertIsNone(row['present_display_intervals_ms'])
        self.assertIsNone(row['presentmon_swapchains']['0x123']['modes'])
        self.assertIsNone(row['presentmon_swapchains']['0x123']['display_change_rate_hz'])
        self.assertEqual(row['presentmon_swapchains']['0x123']['runtime_intervals_ms']['p50'], 16)
        pooled = report.aggregate([row])[0]
        self.assertIsNone(pooled['present_modes'])
        off = dict(pooled, no_capture=True)
        comparison = report.overhead_checks([pooled, off])[0]
        self.assertFalse(comparison['comparison_eligible'])
        self.assertIsNone(comparison['overhead_fraction'])

    def test_runtime_only_is_rejected_on_gpu_or_without_the_approved_reason(self):
        for changes in ({'vm': 'capycanvas6-poc-gpu'}, {'presentmon_unavailable_reason': None}, {'presentmon_unavailable_reason': 'single-present interval baseline'}):
            with self.subTest(changes=changes), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.runtime_benchmark_contact(directory)
                path = directory / 'meta.json'
                meta = json.loads(path.read_text()); meta.update(changes); path.write_text(json.dumps(meta))
                row = report.analyse_contact(directory)
                self.assertFalse(row['valid'])
                self.assertTrue(any('approved WARP-only exception' in error for error in row['validation_errors']))

    def test_presentmon_record_mode_must_match_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.runtime_benchmark_contact(directory)
            path = directory / 'run.json'
            run = json.loads(path.read_text()); run['presentmon_mode'] = 'full'; path.write_text(json.dumps(run))
            row = report.analyse_contact(directory)
        self.assertFalse(row['valid'])
        self.assertTrue(any('mode does not match' in error for error in row['validation_errors']))

    def test_runtime_only_requires_basic_process_and_time_columns(self):
        for header, values in (('Application,Runtime,SwapChainAddress,msBetweenPresents,QPCTime', 'canvas-only.exe,DXGI,0x123,16,1150'), ('Application,ProcessID,Runtime,SwapChainAddress,msBetweenPresents', 'canvas-only.exe,42,DXGI,0x123,16'), ('Application,ProcessID,Runtime,SwapChainAddress,msBetweenPresents,QPCTime', 'canvas-only.exe,42,Composed: Flip,0x123,16,1150')):
            with self.subTest(header=header, values=values), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.runtime_benchmark_contact(directory)
                (directory / 'presents.csv').write_text(header+'\n'+values+'\n')
                self.assertFalse(report.analyse_contact(directory)['valid'])

    def test_full_mode_rejects_missing_display_fields_and_runtime_relabelling(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.valid_benchmark_contact(directory)
            (directory / 'presents.csv').write_text('Application,ProcessID,Runtime,SwapChainAddress,msBetweenPresents,QPCTime\ncanvas-only.exe,42,DXGI,0x123,16,1150\n')
            row = report.analyse_contact(directory)
            self.assertFalse(row['valid'])
            self.assertTrue(any('full CSV missing' in error for error in row['validation_errors']))
            self.runtime_benchmark_contact(directory)
            (directory / 'presents.csv').write_text('Application,ProcessID,Runtime,SwapChainAddress,PresentMode,msBetweenPresents,msBetweenDisplayChange,msUntilDisplayed,QPCTime\ncanvas-only.exe,42,DXGI,0x123,Composed: Flip,16,16,2,1150\n')
            row = report.analyse_contact(directory)
            self.assertFalse(row['valid'])
            self.assertTrue(any('inconsistent with its recorded mode' in error for error in row['validation_errors']))

    def test_presentmon_rows_are_filtered_to_the_owned_process(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.runtime_benchmark_contact(directory)
            csv = directory / 'presents.csv'
            csv.write_text(csv.read_text()+'unrelated.exe,99,DXGI,0x999,1,1200\n')
            row = report.analyse_contact(directory)
            self.assertTrue(row['valid'], row['validation_errors'])
            self.assertEqual(row['runtime_present_rows'], 1)
            self.assertNotIn('0x999', row['presentmon_swapchains'])
            csv.write_text('Application,ProcessID,Runtime,SwapChainAddress,msBetweenPresents,QPCTime\nunrelated.exe,99,DXGI,0x999,1,1200\n')
            row = report.analyse_contact(directory)
            self.assertFalse(row['valid'])
            self.assertTrue(any('owned process ID' in error for error in row['validation_errors']))

    def test_runtime_startup_retains_single_present_empty_csv_exception(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.runtime_benchmark_contact(directory, workload='startup')
            (directory / 'presents.csv').unlink()
            meta = json.loads((directory / 'meta.json').read_text())
            meta.update(motion_start_qpc=None, motion_end_qpc=None, idle_start_qpc=5000, idle_end_qpc=15000)
            (directory / 'meta.json').write_text(json.dumps(meta))
            row = report.analyse_contact(directory)
        self.assertTrue(row['valid'], row['validation_errors'])
        self.assertIsNone(row['idle_presents'])
        self.assertIsNone(row['present_modes'])

    def test_invalid_jsonl_is_an_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "frames.jsonl"
            path.write_text('{broken}\n')
            with self.assertRaises(json.JSONDecodeError):
                report.read_jsonl(path)


if __name__ == "__main__":
    unittest.main()
