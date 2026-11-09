#!/usr/bin/env python3
import argparse
import collections
import csv
import json
import math
from pathlib import Path


WARP_RUNTIME_REASON = "Approved WARP-only exception: PresentMode and display timing are unverified; PresentMon records runtime events only"

CALIBRATED_WORKLOADS = {"divider", "resize-programmatic", "resize-native", "scrub-while-painting"}
RATE_FIELDS = ("motion_rate_hz", "fraction_refresh", "all_ui_changes_hz", "canvas_change_hz", "canvas_presentmon_display_hz")

PROBES = {"divider": "divider_y", "resize-programmatic": "width", "resize-native": "width", "scroll-wheel": "scroll_offset", "scroll-touch": "scroll_offset", "drawer": "drawer_x", "zen": "zen_opacity", "scrub-while-painting": "slider_x", "fake-motion": "divider_y"}


def base_workload(name):
    return {"stall-drawer": "drawer", "stall-scroll": "scroll-wheel", "slowcanvas-divider": "divider", "slowcanvas-drawer": "drawer"}.get(name, name)


def read_json(path, default=None):
    return json.loads(path.read_text(encoding="utf-8-sig")) if path.exists() else default


def read_jsonl(path):
    if not path.exists():
        return []
    rows = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8-sig").splitlines(), 1):
        if line.strip():
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{path}:{line_number}: expected JSON object")
            rows.append(value)
    return rows


def quantiles(values):
    values = sorted(x for x in values if x is not None and math.isfinite(x))
    def q(fraction):
        if not values:
            return None
        index = (len(values) - 1) * fraction
        lo = int(index)
        return values[lo] + (values[min(lo + 1, len(values) - 1)] - values[lo]) * (index - lo)
    return {"p50": q(.5), "p95": q(.95), "p99": q(.99), "max": max(values) if values else None, "count": len(values)}


def frame_time(frame):
    return frame.get("last_present_qpc") or frame["qpc"]


def changes(frames, field, start, end):
    changed = []
    previous = None
    for frame in frames:
        value = frame.get(field)
        tick = frame_time(frame)
        if value is not None and previous is not None and value != previous and start <= tick <= end:
            changed.append(tick)
        previous = value
    return changed


def calibrate_probe(meta, frames, field):
    if base_workload(meta["workload"]) not in CALIBRATED_WORKLOADS or meta.get("no_capture"):
        return {"status": "not-applicable", "offset_px": None, "spread_px": None, "samples": 0}
    start, end, expected = (meta.get(key) for key in ("calibration_start_qpc", "calibration_end_qpc", "calibration_expected_value"))
    offsets = [frame[field] - expected for frame in frames if start is not None and end is not None and expected is not None and start <= frame["qpc"] < end and frame.get(field) is not None]
    spread = max(offsets) - min(offsets) if offsets else None
    offset = quantiles(offsets)["p50"]
    error = "No resting probe observations before the first measured move" if not offsets else "Resting probe offset spread exceeds 1 px" if spread > 1 else None
    return {"status": "failed" if error else "passed", "offset_px": offset, "spread_px": spread, "samples": len(offsets), "start_qpc": start, "end_qpc": end, "expected_value": expected, "error": error}


def capture_adequacy(meta, frames, capture):
    if meta.get("no_capture"):
        return {"status": "capture-off", "capture_limited": False, "duration_s": None, "desktop_rate_hz": None, "refresh_hz": meta.get("refresh_hz"), "fraction_refresh": None, "missing_share": None, "capture_ms": quantiles([])}
    frequency, refresh = meta["qpc_frequency"], meta.get("refresh_hz")
    start = capture.get("capture_start_qpc", meta.get("capture_start_qpc", meta.get("window_qpc")))
    end = capture.get("capture_end_qpc", meta.get("capture_end_qpc", meta.get("idle_end_qpc") or meta.get("motion_end_qpc")))
    duration = (end - start) / frequency if start is not None and end is not None and end > start else None
    desktop, missing = capture.get("desktop_frames"), capture.get("missing_desktop_frames")
    share = missing / (desktop + missing) if desktop is not None and missing is not None and desktop + missing else None
    times = quantiles([frame["capture_ms"] for frame in frames if frame.get("capture_ms") is not None])
    threshold = 500 / refresh if refresh and refresh > 0 else None
    limited = True if (share is not None and share > .02) or (threshold is not None and times["p95"] is not None and times["p95"] > threshold) else False if share is not None and threshold is not None and times["p95"] is not None else None
    rate = desktop / duration if desktop is not None and duration else None
    return {"status": "capture-limited" if limited else "adequate" if limited is False else "unknown", "capture_limited": limited, "duration_s": duration, "desktop_rate_hz": rate, "refresh_hz": refresh, "fraction_refresh": rate / refresh if rate is not None and refresh else None, "missing_share": share, "capture_ms": times}


def input_quality(meta, evidence):
    if evidence is None:
        status = "not-applicable" if base_workload(meta["workload"]) in ("startup", "fake-motion") else "input-unmeasured"
        return {"status": status, "input_limited": None, "streams": {}}
    streams = {}
    for name, values in evidence.get("streams", {}).items():
        seconds, nominal, accepted = (values.get(key) for key in ("measured_seconds", "nominal_hz", "accepted_measured"))
        measured = seconds is not None and seconds > 0 and nominal is not None and nominal > 0 and accepted is not None
        achieved = accepted / seconds if measured else None
        limited = achieved < .9 * nominal if measured else None
        stream = dict(values)
        stream.update(achieved_hz=achieved, input_limited=limited, status="input-limited" if limited else "input-adequate" if limited is False else "input-unmeasured")
        if values.get("achieved_hz") is not None and achieved is not None:
            stream["recorded_rate_matches_counts"] = math.isclose(values["achieved_hz"], achieved, rel_tol=1e-9, abs_tol=1e-9)
        streams[name] = stream
    statuses = {stream["status"] for stream in streams.values()}
    status = "input-limited" if "input-limited" in statuses else "input-unmeasured" if "input-unmeasured" in statuses else "input-adequate" if statuses else "not-applicable" if base_workload(meta["workload"]) in ("startup", "fake-motion") else "input-unmeasured"
    return {"status": status, "input_limited": True if status == "input-limited" else False if status in ("input-adequate", "not-applicable") else None, "streams": streams}


def combined_input_status(rows):
    statuses = {row.get("input_quality", {}).get("status", "input-unmeasured") for row in rows}
    return "input-limited" if "input-limited" in statuses else "input-unmeasured" if "input-unmeasured" in statuses or not statuses else "input-adequate" if "input-adequate" in statuses else "not-applicable"


def rate_status(capture, inputs):
    return capture + ("; " + inputs if inputs != "not-applicable" else "")


def observer_cpu(records):
    grouped = collections.defaultdict(list)
    for record in records:
        if record.get("thread") is not None:
            grouped[record["thread"]].append(record)
    output = {}
    for thread, rows in grouped.items():
        rows.sort(key=lambda row: row["qpc"])
        final = rows[-1]
        cpu, wall = final.get("cpu_ms"), final.get("wall_ms")
        output[thread] = {"cpu_ms": cpu, "wall_ms": wall, "one_core_percent": cpu / wall * 100 if cpu is not None and wall else None, "priority": final.get("priority"), "records": len(rows), "final": final.get("final", False)}
    return output


def combined_capture_status(rows):
    statuses = {row.get("capture_quality", {}).get("status", "unknown") for row in rows}
    if "capture-limited" in statuses:
        return "capture-limited"
    if "unknown" in statuses or not statuses:
        return "unknown"
    return "capture-off" if statuses == {"capture-off"} else "adequate"


def latency_drag(frames, inputs, field, frequency, offset=0):
    segments = collections.defaultdict(list)
    for sample in inputs:
        if sample.get("expected_value") is not None and sample.get("segment") is not None:
            segments[sample["segment"]].append(sample)
    values, censored, unmatched = [], 0, 0
    for samples in segments.values():
        samples.sort(key=lambda row: row["qpc"])
        low = min(row.get("segment_min", row["expected_value"]) for row in samples)
        high = max(row.get("segment_max", row["expected_value"]) for row in samples)
        for frame in frames:
            tick, value = frame_time(frame), frame.get(field)
            if not samples[0]["qpc"] <= tick <= samples[-1]["qpc"]:
                continue
            if value is None:
                unmatched += 1
                continue
            value -= offset
            if high == low or value <= low + .05 * (high - low) or value >= high - .05 * (high - low):
                censored += 1
                continue
            match = next((sample for sample in reversed(samples) if sample["qpc"] <= tick and abs(sample["expected_value"] - value) <= 1), None)
            if match is None:
                unmatched += 1
            else:
                values.append((tick - match["qpc"]) * 1000 / frequency)
    return values, censored, unmatched


def validated_receipts(inputs, trace, evidence):
    injections = [sample for sample in inputs if sample.get("kind") == "pen"]
    information = {"validated": False, "basis": None, "timestamp_identity": evidence.get("timestamp_identity"), "rewritten_receipt_timestamps": evidence.get("rewritten_receipt_timestamps"), "rewritten_consumed_timestamps": evidence.get("rewritten_consumed_timestamps"), "timestamp_comparable_samples": evidence.get("timestamp_comparable_samples"), "timestamp_noncomparable_samples": evidence.get("timestamp_noncomparable_samples"), "host_timestamp_bases": evidence.get("host_timestamp_bases", []), "native_latest_identity": evidence.get("native_latest_identity"), "native_latest_checked_samples": evidence.get("native_latest_checked_samples"), "native_latest_rewritten_samples": evidence.get("native_latest_rewritten_samples")}
    if not injections:
        information["status"] = "not-applicable"
        return {}, information
    expected = list(range(len(injections)))
    receipts = [record for record in trace if record.get("kind") == "input_receipt"]
    consumed = [record for record in trace if record.get("kind") == "pen_consumed"]
    gate_passed = evidence.get("validated") is True and evidence.get("matching_basis") == "ordered_indices_and_quantized_sample_tuples" and all(evidence.get(key) == len(injections) for key in ("injected", "received", "consumed"))
    trace_matches = all([record.get("index") for record in records] == expected for records in (injections, receipts, consumed)) and all(record.get("qpc") is not None for record in receipts)
    if not gate_passed or not trace_matches:
        information["status"] = "unvalidated-delivery" if not gate_passed else "inconsistent-gate-and-trace"
        return {}, information
    information.update(validated=True, status="validated", basis="source_index_validated_by_pen-delivery.json")
    return {record["index"]: record["qpc"] for record in receipts}, information


def first_reached(frames, field, sample, threshold):
    return next((frame_time(frame) for frame in frames if frame_time(frame) >= sample["qpc"] and frame.get(field) is not None and frame[field] >= threshold), None)


def motion_windows(meta, frames, inputs, field):
    start, end = meta.get("motion_start_qpc"), meta.get("motion_end_qpc")
    if start is None or end is None:
        return []
    name, frequency = base_workload(meta["workload"]), meta["qpc_frequency"]
    if name in ("drawer", "zen"):
        windows = []
        clicks = [row for row in inputs if row["kind"] == "click"]
        for index, click in enumerate(clicks):
            limit = clicks[index + 1]["qpc"] if index + 1 < len(clicks) else end
            target = click.get("expected_value")
            selected = [frame for frame in frames if click["qpc"] <= frame_time(frame) <= limit]
            complete = next((frame_time(left) for left, right in zip(selected, selected[1:]) if target is not None and left.get(field) == target and right.get(field) == target), None)
            if complete is not None:
                windows.append((click["qpc"], complete))
        return windows
    if name.startswith("scroll-"):
        events = [row for row in inputs if row["kind"] in ("wheel", "touch-down", "touch-move", "touch-up")]
        return [(events[0]["qpc"], min(end, events[-1]["qpc"] + int(.3 * frequency)))] if events else []
    return [(start + int(.25 * frequency), end)] if field else []


def process_usage(rows, start=None, end=None):
    selected = [row for row in rows if (start is None or row["qpc"] >= start) and (end is None or row["qpc"] <= end)]
    return selected


def ui_cpu_per_change(trace, frames, start, end):
    rows = sorted((row for row in trace if row.get("kind") == "thread_cpu" and row.get("thread") == "ui" and "cpu_ms" in row and start <= row["qpc"] <= end), key=lambda row: row["qpc"])
    if len(rows) < 2:
        return None
    count = len(changes(frames, "ui_frame", rows[0]["qpc"], rows[-1]["qpc"]))
    return (rows[-1]["cpu_ms"] - rows[0]["cpu_ms"]) / count if count else None


def runtime_only_approved(meta):
    return meta.get("vm") == "capycanvas6-poc-warp" and meta.get("presentmon_mode") == "runtime-only" and meta.get("presentmon_unavailable_reason") == WARP_RUNTIME_REASON


def presentmon(path, mode="full"):
    if not path.exists():
        return [], []
    with path.open(encoding="utf-8-sig", newline="") as stream:
        rows = list(csv.DictReader(stream))
    canonical = {key.casefold(): key for key in ("Application", "ProcessID", "Runtime", "PresentMode", "SwapChainAddress", "MsBetweenPresents", "MsBetweenDisplayChange", "MsUntilDisplayed", "QPCTime", "CPUStartQPC")}
    rows = [{canonical.get(key.casefold(), key): value for key, value in row.items()} for row in rows]
    display = {"PresentMode", "MsBetweenDisplayChange", "MsUntilDisplayed"}
    required = {"SwapChainAddress", "MsBetweenPresents", "QPCTime"} | ({"Application", "ProcessID", "Runtime"} if mode == "runtime-only" else display)
    if rows and not required.issubset(rows[0]):
        raise ValueError(f"{path}: PresentMon {mode} CSV missing {sorted(required - rows[0].keys())}")
    if mode == "runtime-only" and rows:
        if display.intersection(rows[0]):
            raise ValueError(f"{path}: runtime-only CSV contains display fields inconsistent with its recorded mode")
        for row in rows:
            try:
                valid = bool(row["Application"]) and bool(row["SwapChainAddress"]) and row["Runtime"] in ("DXGI", "D3D9", "Other") and int(row["ProcessID"]) > 0 and int(row["QPCTime"]) > 0 and float(row["MsBetweenPresents"]) >= 0
            except (ValueError, TypeError):
                valid = False
            if not valid:
                raise ValueError(f"{path}: runtime-only CSV lacks valid process/runtime/time evidence")
    return rows, sorted({row["PresentMode"] for row in rows if row.get("PresentMode")})


def contact_validation(directory, meta, calibration, receipt_matching, inputs, presents, first_ui, first_canvas, presentmon_error):
    errors = []
    if calibration["status"] == "failed":
        errors.append("Probe calibration failed: " + calibration["error"])
    completed = read_json(directory / "completed.json", {})
    if completed.get("completed") is not True or completed.get("closed_gracefully") is not True:
        errors.append("Observer completion/graceful-close evidence is missing or failed")
    if (directory / "failures.json").exists():
        errors.append("Observer failures.json is present")
    def envelope(name):
        path = next((ancestor / name for ancestor in (directory, *directory.parents) if (ancestor / name).is_file()), None)
        return read_json(path, {}) if path else {}
    run, done = envelope("run.json"), envelope("done.json")
    mode = meta.get("presentmon_mode", "full")
    runtime = runtime_only_approved(meta)
    if mode not in ("full", "runtime-only") or mode == "runtime-only" and not runtime:
        errors.append("PresentMon runtime-only mode requires the approved WARP-only exception")
    if run.get("presentmon_mode", "full") != mode:
        errors.append("PresentMon run.json mode does not match contact metadata")
    if runtime and run.get("presentmon_unavailable_reason") != WARP_RUNTIME_REASON:
        errors.append("PresentMon run.json does not record the approved WARP-only exception")
    if presentmon_error:
        errors.append(presentmon_error)
    identity_keys = ("arm", "workload", "profile")
    if not run or run.get("exit_code") != 0 or any(run.get(key) != meta.get(key) for key in identity_keys):
        errors.append("Benchmark run.json is missing, failed or belongs to another contact")
    if run.get("presentmon_exit_code") != 0:
        errors.append("PresentMon collector exit evidence is missing or failed")
    if done.get("ok") is not True:
        errors.append("Benchmark done.json is missing or records a failed batch")
    if not any(all(record.get(key) == meta.get(key) for key in identity_keys) and record.get("exit_code") == 0 and record.get("presentmon_mode", "full") == mode for record in done.get("runs", [])):
        errors.append("Benchmark done.json does not include a completed matching run")
    startup_exception = meta["workload"] == "startup" and bool(meta.get("presentmon_unavailable_reason"))
    if not presents and not startup_exception:
        errors.append("PresentMon has no measured-window interval evidence")
    if presents and not runtime and not all(row.get("PresentMode") and row.get("SwapChainAddress") for row in presents):
        errors.append("PresentMon rows lack present-mode or swap-chain evidence")
    if not meta.get("no_capture"):
        startup = read_json(directory / "startup.json", {})
        if first_ui is None or first_canvas is None or any(startup.get(key) is None for key in ("first_ui_qpc", "first_canvas_qpc", "canvas_present_lower_bound_qpc")):
            errors.append("Displayed startup marker evidence is missing or incomplete")
    pen_count = sum(sample.get("kind") == "pen" for sample in inputs)
    if base_workload(meta["workload"]) in ("paint-still", "scrub-while-painting") and not pen_count:
        errors.append("Pen workload has no accepted pen input")
    if pen_count and not receipt_matching["validated"]:
        errors.append("Pen delivery has no validated ordered sample identity evidence")
    return errors


def analyse_contact(directory):
    meta = read_json(directory / "meta.json")
    if not meta:
        raise ValueError(f"{directory}: missing meta.json")
    frequency = meta["qpc_frequency"]
    if frequency <= 0:
        raise ValueError("QPC frequency must be positive")
    acquired_frames = read_jsonl(directory / "frames.jsonl")
    frames = sorted((frame for frame in acquired_frames if frame.get("last_present_qpc", 1) > 0), key=frame_time)
    all_inputs = read_jsonl(directory / "inputs.jsonl")
    inputs = [sample for sample in all_inputs if not sample.get("warmup", False)]
    processes = read_jsonl(directory / "process.jsonl")
    capture = read_json(directory / "capture.json", {})
    workload = base_workload(meta["workload"])
    field = PROBES.get(workload)
    calibration = calibrate_probe(meta, acquired_frames, field)
    quality = capture_adequacy(meta, acquired_frames, capture)
    injection = input_quality(meta, read_json(directory / "injection.json"))
    observer_threads = observer_cpu(read_jsonl(directory / "observer-cpu.jsonl"))
    for stream in injection["streams"].values():
        stream["capture_status"] = quality["status"]
    windows = motion_windows(meta, frames, inputs, field)
    quality_windows = windows or ([(meta["motion_start_qpc"], meta["motion_end_qpc"])] if meta.get("motion_start_qpc") is not None and meta.get("motion_end_qpc") is not None else [])
    quality_frames = [frame for frame in frames if any(start <= frame_time(frame) <= end for start, end in quality_windows)]
    duration = sum(end - start for start, end in windows) / frequency
    motion_ticks = [tick for start, end in windows for tick in changes(frames, field, start, end)] if field else []
    intervals = [(b - a) * 1000 / frequency for start, end in windows for a, b in zip(changes(frames, field, start, end), changes(frames, field, start, end)[1:])] if field else []
    start = meta.get("motion_start_qpc") or meta["launch_qpc"]
    end = meta.get("motion_end_qpc") or meta.get("idle_end_qpc") or (processes[-1]["qpc"] if processes else start)
    elapsed = (end - start) / frequency
    ui_ticks = changes(frames, "ui_frame", start, end)
    canvas_ticks = changes(frames, "pen_samples", start, end)
    ink, ink_unmatched, receipt, receipt_unmatched = [], 0, [], 0
    trace = read_jsonl(directory / "canvas.jsonl")
    delivery = read_json(directory / "pen-delivery.json", {})
    receipts, receipt_matching = validated_receipts(all_inputs, trace, delivery)
    for sample in (row for row in inputs if row["kind"] == "pen"):
        tick = first_reached(frames, "pen_samples", sample, sample["index"] + 1)
        if tick is None:
            ink_unmatched += 1
        else:
            ink.append((tick - sample["qpc"]) * 1000 / frequency)
        received = receipts.get(sample["index"])
        if received is None or received < sample["qpc"]:
            receipt_unmatched += 1
        else:
            receipt.append((received - sample["qpc"]) * 1000 / frequency)
    latency, completion, censored, unmatched = [], [], 0, 0
    completion_target_basis = None
    if workload in ("divider", "resize-programmatic", "resize-native", "scrub-while-painting", "scroll-touch"):
        if calibration["status"] != "failed":
            latency, censored, unmatched = latency_drag(frames, inputs, field, frequency, calibration["offset_px"] or 0)
    if workload == "typing":
        for sample in (row for row in inputs if row["kind"] == "key"):
            tick = first_reached(frames, "state16", sample, sample["expected_state"])
            if tick is None:
                unmatched += 1
            else:
                latency.append((tick - sample["qpc"]) * 1000 / frequency)
    if workload in ("drawer", "zen", "scroll-wheel"):
        for sample in (row for row in inputs if row["kind"] in ("click", "wheel")):
            before = next((frame.get(field) for frame in reversed(frames) if frame_time(frame) < sample["qpc"]), None)
            moved = next((frame_time(frame) for frame in frames if frame_time(frame) >= sample["qpc"] and frame.get(field) is not None and frame[field] != before), None)
            if moved is None:
                unmatched += 1
            else:
                latency.append((moved - sample["qpc"]) * 1000 / frequency)
        if workload != "scroll-wheel":
            completion = [(last - first) * 1000 / frequency for first, last in windows]
        else:
            notches = [row for row in inputs if row["kind"] == "wheel"]
            if notches:
                last = notches[-1]
                selected = [frame for frame in frames if frame_time(frame) >= last["qpc"] and frame.get(field) is not None]
                target = last.get("expected_scroll_offset")
                completion_target_basis = "injected_target" if target is not None else "observed_final_offset"
                if target is None and selected:
                    target = selected[-1][field]
                settled = next((frame_time(left) for left, right in zip(selected, selected[1:]) if target is not None and abs(left[field] - target) <= 1 and abs(right[field] - target) <= 1), None)
                if settled is not None:
                    completion = [(settled - last["qpc"]) * 1000 / frequency]
    inertia = []
    if workload == "scroll-touch":
        releases = [row for row in inputs if row["kind"] == "touch-up"]
        for index, sample in enumerate(releases):
            limit = releases[index + 1]["qpc"] - int(.04 * frequency) if index + 1 < len(releases) else end
            ticks = changes(frames, field, sample["qpc"], limit)
            if ticks:
                inertia.append((ticks[-1] - sample["qpc"]) * 1000 / frequency)
    drift = [max(frame[key] for key in ("divider_y", "glass_y", "fill_y")) - min(frame[key] for key in ("divider_y", "glass_y", "fill_y")) for frame in quality_frames if all(frame.get(key) is not None for key in ("divider_y", "glass_y", "fill_y"))]
    black = [frame["black_samples"] for frame in quality_frames if frame.get("black_samples") is not None]
    valid_process = process_usage(processes, start, end)
    def cpu(rows):
        if len(rows) < 2 or rows[-1]["qpc"] == rows[0]["qpc"]:
            return None
        return (rows[-1]["cpu_ms"] - rows[0]["cpu_ms"]) / ((rows[-1]["qpc"] - rows[0]["qpc"]) / frequency * 1000) * 100 / meta.get("logical_cpus", 1)
    pm_file = directory / "presents.csv"
    if not pm_file.exists():
        pm_file = directory.parent / "presents.csv"
    runtime = runtime_only_approved(meta)
    presentmon_error = None
    try:
        all_presents, modes = presentmon(pm_file, "runtime-only" if runtime else "full")
    except ValueError as error:
        all_presents, modes, presentmon_error = [], [], str(error)
    if meta.get("process_id") is not None:
        owned = [row for row in all_presents if str(row.get("ProcessID")) == str(meta["process_id"])]
        if all_presents and not owned:
            presentmon_error = "PresentMon CSV has no rows for the owned process ID"
        all_presents = owned
    def present_tick(row):
        for key in ("QPCTime", "CPUStartQPC"):
            if row.get(key) not in (None, "", "NA"):
                return float(row[key])
        return None
    presents = [row for row in all_presents if present_tick(row) is not None and start <= present_tick(row) <= end]
    modes = None if runtime else sorted({row["PresentMode"] for row in presents if row.get("PresentMode")})
    chains = collections.defaultdict(list)
    for row in presents:
        chains[row["SwapChainAddress"]].append(row)
    chain_metrics = {}
    for chain, rows in chains.items():
        displayed = [] if runtime else [float(row["MsBetweenDisplayChange"]) for row in rows if row.get("MsBetweenDisplayChange") not in (None, "", "NA", "0", "0.0000") and float(row["MsBetweenDisplayChange"]) > 0]
        chain_metrics[chain] = {"present_count": len(rows), "runtime_intervals_ms": quantiles([float(row["MsBetweenPresents"]) for row in rows if row.get("MsBetweenPresents") not in (None, "", "NA")]), "display_change_rate_hz": 1000 / (sum(displayed) / len(displayed)) if displayed else None, "capture_status": quality["status"], "display_intervals_ms": None if runtime else quantiles(displayed), "modes": None if runtime else sorted({row["PresentMode"] for row in rows})}
    canvas_chain = meta.get("canvas_swapchain") or next((row.get("swapchain") for row in trace if row.get("kind") in ("adapter", "startup", "surface") and row.get("swapchain")), None)
    canvas_pm_rate = next((value["display_change_rate_hz"] for chain, value in chain_metrics.items() if canvas_chain is not None and int(chain, 16) == int(str(canvas_chain), 16)), None)
    first_ui = next((frame_time(frame) for frame in frames if frame_time(frame) >= meta["launch_qpc"] and frame.get("ui_marker") not in (None, 0)), None)
    canvas_present = next((row["qpc"] for row in trace if row.get("kind") == "startup" and row.get("milestone") == "first_canvas_present"), None)
    first_canvas = next((frame_time(frame) for frame in frames if canvas_present is not None and frame_time(frame) >= max(meta["launch_qpc"], canvas_present) and frame.get("pen_samples") is not None and (meta["arm"] != "canvas-only" or frame.get("ui_marker") not in (None, 0))), None)
    idle_start, idle_end = meta.get("idle_start_qpc"), meta.get("idle_end_qpc")
    idle_presents = None
    if idle_start is not None and idle_end is not None:
        timed = [row for row in all_presents if row.get("QPCTime") or row.get("CPUStartQPC")]
        idle_presents = sum(idle_start <= float(row.get("QPCTime") or row["CPUStartQPC"]) <= idle_end for row in timed) if timed else None
    validation_errors = contact_validation(directory, meta, calibration, receipt_matching, all_inputs, presents, first_ui, first_canvas, presentmon_error)
    decoded = [frame for frame in frames if frame.get("ui_marker") not in (None, 0) and frame.get("pen_samples") is not None]
    sample_frames = decoded[:2] + [frame for frame in decoded if frame.get("pen_samples", 0) > 0][:1] + decoded[-1:]
    samples = [{key: frame.get(key) for key in ("qpc", "ui_marker", "state16", "ui_frame", "pen_samples")} for frame in {frame["qpc"]: frame for frame in sample_frames}.values()]
    return {"directory": str(directory), "presentmon_mode": meta.get("presentmon_mode", "full"), "presentmon_runtime_only_approved": runtime, "presentmon_display_status": "unverified" if runtime else "verified" if presents else "unverified", "runtime_present_rows": len(presents), "presentmon_unavailable_reason": meta.get("presentmon_unavailable_reason"), "valid": not validation_errors, "validation_errors": validation_errors, "probe_calibration": calibration, "capture_quality": quality, "input_quality": injection, "observer_cpu": observer_threads, "rate_capture_status": {key: quality["status"] for key in RATE_FIELDS}, "rate_input_status": {key: injection["status"] for key in RATE_FIELDS}, "vm": meta.get("vm", directory.parts[-4] if len(directory.parts) > 3 else "unknown"), "arm": meta["arm"], "profile": meta["profile"], "workload": meta["workload"], "no_capture": meta.get("no_capture", False), "contact": meta.get("contact"), "adapter": next((row for row in trace if row.get("kind") == "adapter"), meta.get("adapter")), "display_mode": meta.get("display_mode"), "order_slot": meta.get("order_slot"), "refresh_hz": meta.get("refresh_hz"), "motion_duration_s": duration, "motion_changes": len(motion_ticks), "motion_rate_hz": len(motion_ticks) / duration if duration else None, "fraction_refresh": len(motion_ticks) / duration / meta["refresh_hz"] if duration and meta.get("refresh_hz") else None, "motion_intervals_ms": quantiles(intervals), "all_ui_changes": len(ui_ticks) if frames else None, "all_ui_changes_hz": len(ui_ticks) / elapsed if elapsed > 0 and frames else None, "canvas_change_hz": len(canvas_ticks) / elapsed if elapsed > 0 and frames else None, "ui_latency_ms": quantiles(latency), "completion_latency_ms": quantiles(completion), "completion_target_basis": completion_target_basis, "inertia_ms": quantiles(inertia), "ink_latency_ms": quantiles(ink), "input_receipt_latency_ms": quantiles(receipt), "receipt_matching": receipt_matching, "unmatched": unmatched, "censored": censored, "ink_unmatched": ink_unmatched, "receipt_unmatched": receipt_unmatched, "drift_max_px": max(drift) if drift else None, "drift_over_1_share": sum(value > 1 for value in drift) / len(drift) if drift else None, "black_interior_frames": sum(value > 0 for value in black) if black else None, "process_cpu_percent": cpu(valid_process), "cpu_basis": "guest capacity" if meta.get("logical_cpus") else "one core", "private_mib": max((row["private_bytes"] / 1048576 for row in processes), default=None), "ui_cpu_ms_per_change": ui_cpu_per_change(trace, frames, start, end), "startup_ui_ms": (first_ui - meta["launch_qpc"]) * 1000 / frequency if first_ui is not None else None, "startup_canvas_ms": (first_canvas - meta["launch_qpc"]) * 1000 / frequency if first_canvas is not None else None, "idle_presents": idle_presents, "idle_cpu_percent": cpu(process_usage(processes, idle_start, idle_end)) if idle_start else None, "present_modes": modes, "presentmon_swapchains": chain_metrics, "canvas_presentmon_display_hz": canvas_pm_rate, "present_display_intervals_ms": None if runtime else quantiles([float(row["MsBetweenDisplayChange"]) for row in presents if row.get("MsBetweenDisplayChange") not in (None, "", "NA", "0")]), "capture": capture, "marker_sample": samples, "_samples": {"motion_intervals_ms": intervals, "ui_latency_ms": latency, "completion_latency_ms": completion, "ink_latency_ms": ink, "input_receipt_latency_ms": receipt, "inertia_ms": inertia}}


def aggregate(contacts):
    groups = collections.defaultdict(list)
    for contact in contacts:
        groups[(contact["vm"], contact["profile"], contact["arm"], contact["workload"], contact["no_capture"])].append(contact)
    result = []
    samples = ("motion_intervals_ms", "ui_latency_ms", "completion_latency_ms", "ink_latency_ms", "input_receipt_latency_ms", "inertia_ms")
    for identity, all_rows in sorted(groups.items()):
        rows = [row for row in all_rows if row.get("valid", False)]
        duration = sum(row["motion_duration_s"] for row in rows)
        changes_count = sum(row["motion_changes"] for row in rows)
        pooled = dict(zip(("vm", "profile", "arm", "workload", "no_capture"), identity))
        status, input_status = combined_capture_status(all_rows), combined_input_status(all_rows)
        excluded = [{"directory": row["directory"], "reasons": row["validation_errors"]} for row in all_rows if not row.get("valid", False)]
        pooled.update(contacts=len(all_rows), valid_contacts=len(rows), failed_contacts=len(excluded), excluded_contacts=excluded, comparison_eligible=not excluded and bool(rows), capture_quality={"status": status, "capture_limited": True if status == "capture-limited" else None if status == "unknown" else False, "contacts": [row.get("capture_quality", {}) for row in all_rows]}, rate_capture_status={key: status for key in RATE_FIELDS}, rate_input_status={key: input_status for key in RATE_FIELDS}, input_quality={"status": input_status, "input_limited": input_status == "input-limited", "contacts": [row["input_quality"] for row in all_rows]})
        pooled.update(motion_rate_hz=changes_count / duration if duration else None, motion_duration_s=duration, motion_changes=changes_count, abba_slots=[row["order_slot"] for row in rows])
        refresh = rows[0].get("refresh_hz") if rows else None
        pooled["fraction_refresh"] = pooled["motion_rate_hz"] / refresh if pooled["motion_rate_hz"] is not None and refresh else None
        for key in samples:
            pooled[key] = quantiles([value for row in rows for value in row["_samples"].get(key, [])])
        for key in ("censored", "unmatched", "ink_unmatched", "receipt_unmatched", "black_interior_frames"):
            values = [row[key] for row in rows if row[key] is not None]
            pooled[key] = sum(values) if values else None
        for key in ("all_ui_changes_hz", "canvas_change_hz", "process_cpu_percent", "private_mib", "startup_ui_ms", "startup_canvas_ms", "ui_cpu_ms_per_change", "canvas_presentmon_display_hz", "idle_presents", "idle_cpu_percent", "drift_max_px", "drift_over_1_share"):
            pooled[key] = quantiles([row[key] for row in rows])
        pooled["presentmon_modes"] = sorted({row["presentmon_mode"] for row in all_rows})
        pooled["present_modes"] = sorted({mode for row in rows for mode in (row["present_modes"] or [])}) if any(row["present_modes"] is not None for row in rows) else None
        pooled["observer_cpu"] = {}
        for thread in sorted({thread for row in rows for thread in row.get("observer_cpu", {})}):
            values = [row["observer_cpu"][thread] for row in rows if thread in row.get("observer_cpu", {})]
            cpu_values, wall_values = [value["cpu_ms"] for value in values if value["cpu_ms"] is not None], [value["wall_ms"] for value in values if value["wall_ms"] is not None]
            cpu, wall = sum(cpu_values) if cpu_values else None, sum(wall_values) if wall_values else None
            pooled["observer_cpu"][thread] = {"cpu_ms": cpu, "wall_ms": wall, "one_core_percent": cpu / wall * 100 if cpu is not None and wall else None, "contacts_reported": len(values), "final_contacts": sum(value.get("final", False) for value in values), "priorities": sorted({str(value["priority"]) for value in values if value.get("priority") is not None})}
        pooled["receipt_matching"] = {"bases": sorted({row["receipt_matching"]["basis"] for row in all_rows if row["receipt_matching"].get("basis")}), "statuses": sorted({row["receipt_matching"]["status"] for row in all_rows}), "timestamp_identity": sorted({row["receipt_matching"]["timestamp_identity"] for row in all_rows if row["receipt_matching"].get("timestamp_identity")})}
        pooled["receipt_matching"]["native_latest_identity"] = sorted({row["receipt_matching"]["native_latest_identity"] for row in all_rows if row["receipt_matching"].get("native_latest_identity")})
        pooled["receipt_matching"]["host_timestamp_bases"] = sorted({basis for row in all_rows for basis in row["receipt_matching"].get("host_timestamp_bases", [])})
        for key in ("rewritten_receipt_timestamps", "rewritten_consumed_timestamps", "timestamp_comparable_samples", "timestamp_noncomparable_samples", "native_latest_checked_samples", "native_latest_rewritten_samples"):
            values = [row["receipt_matching"][key] for row in all_rows if row["receipt_matching"].get(key) is not None]
            pooled["receipt_matching"][key] = sum(values) if values else None
        pooled["presentmon_unavailable_reasons"] = sorted({row["presentmon_unavailable_reason"] for row in all_rows if row.get("presentmon_unavailable_reason")})
        pooled["capture"] = {key: sum(row["capture"].get(key, 0) or 0 for row in all_rows) for key in ("desktop_frames", "cursor_only_frames", "coalesced_frames", "missing_desktop_frames", "marker_errors")}
        pooled["capture_failures"] = [row["capture"]["capture_failure"] for row in all_rows if row["capture"].get("capture_failure")]
        result.append(pooled)
    return result


def abba_checks(contacts):
    groups = collections.defaultdict(list)
    for contact in contacts:
        if not contact["no_capture"] and contact["arm"] != "canvas-only":
            groups[(contact["vm"], contact["profile"], contact["workload"])].append(contact)
    checks = []
    for identity, rows in sorted(groups.items()):
        slots = collections.defaultdict(list)
        for row in rows:
            slots[row["order_slot"]].append(row)
        valid = set(slots) == {0, 1, 2, 3} and all(len(slots[slot]) == 3 and {row["arm"] for row in slots[slot]} == {arm} for slot, arm in enumerate(("retained", "winui", "winui", "retained")))
        eligible = valid and all(row.get("valid", True) for row in rows)
        drift = {}
        for arm, first, last in (("retained", 0, 3), ("winui", 1, 2)):
            def rate(slot):
                included = [row for row in slots[slot] if row.get("valid", True)]
                duration = sum(row["motion_duration_s"] for row in included)
                return sum(row["motion_changes"] for row in included) / duration if duration else None
            early, late = rate(first), rate(last)
            early_status, late_status = combined_capture_status(slots[first]), combined_capture_status(slots[last])
            drift[arm] = {"early_motion_hz": early, "early_capture_status": early_status, "early_input_status": combined_input_status(slots[first]), "late_motion_hz": late, "late_capture_status": late_status, "late_input_status": combined_input_status(slots[last]), "relative_change": late / early - 1 if eligible and early and late is not None else None, "comparison_capture_status": combined_capture_status(slots[first] + slots[last]), "comparison_input_status": combined_input_status(slots[first] + slots[last]), "failed_contacts": sum(not row.get("valid", True) for row in slots[first] + slots[last])}
        checks.append(dict(zip(("vm", "profile", "workload"), identity), order_valid=valid, comparison_eligible=eligible, slot_contact_counts={str(slot): len(values) for slot, values in slots.items()}, drift=drift))
    return checks


def overhead_checks(pooled):
    groups = collections.defaultdict(dict)
    for row in pooled:
        if row["workload"] in ("paint-still", "divider"):
            groups[(row["vm"], row["profile"], row["arm"], row["workload"])][row["no_capture"]] = row
    output = []
    for identity, modes in sorted(groups.items()):
        on = modes.get(False, {}).get("canvas_presentmon_display_hz", {}).get("p50")
        off = modes.get(True, {}).get("canvas_presentmon_display_hz", {}).get("p50")
        eligible = set(modes) == {False, True} and all(row.get("comparison_eligible", True) for row in modes.values()) and on is not None and off is not None
        output.append(dict(zip(("vm", "profile", "arm", "workload"), identity), comparison_eligible=eligible, capture_on_canvas_hz=on, capture_on_status=modes.get(False, {}).get("capture_quality", {}).get("status", "unknown"), capture_on_input_status=modes.get(False, {}).get("input_quality", {}).get("status", "input-unmeasured"), capture_off_canvas_hz=off, capture_off_status=modes.get(True, {}).get("capture_quality", {}).get("status", "unknown"), capture_off_input_status=modes.get(True, {}).get("input_quality", {}).get("status", "input-unmeasured"), comparison_capture_status=combined_capture_status(list(modes.values())), comparison_input_status=combined_input_status(list(modes.values())), overhead_fraction=1 - on / off if eligible and off and on is not None else None))
    return output


def render(rows, contacts=(), abba=(), overhead=()):
    def number(value):
        return "—" if value is None else f"{value:.2f}"
    def rate(value, status):
        return number(value) + " (" + status + ")" if value is not None else "—"
    lines = ["# Windows UI POC results", "", "VM A/B diagnostics; these do not qualify hardware tier targets. Null metrics are unmeasured, not zero. Failed/incomplete contacts remain visible as diagnostics and are excluded from pooled benchmark metrics and comparisons.", "", "| Profile | Arm | Workload | Capture | Contacts | Motion Hz | Interval p99 ms | UI latency p99 ms | Ink p99 ms | Capture missing/coalesced |", "| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for row in rows:
        capture = row["capture"]
        lines.append(f"| {row['profile']} | {row['arm']} | {row['workload']} | {'off' if row['no_capture'] else 'on'} | {row['contacts']} ({row['valid_contacts']} valid, {row['failed_contacts']} FAILED/excluded) | {rate(row['motion_rate_hz'], rate_status(row['capture_quality']['status'], row['input_quality']['status']))} | {number(row['motion_intervals_ms']['p99'])} | {number(row['ui_latency_ms']['p99'])} | {number(row['ink_latency_ms']['p99'])} | {capture['missing_desktop_frames']}/{capture['coalesced_frames']} |")
    lines.extend(["", "| Arm | Workload | Diagnostic desktop frames | Cursor only | Missing desktop | Coalesced | Marker errors |", "| --- | --- | ---: | ---: | ---: | ---: | ---: |"])
    for row in rows:
        if row["no_capture"]:
            continue
        capture = row["capture"]
        lines.append(f"| {row['arm']} | {row['workload']} | {capture['desktop_frames']} | {capture['cursor_only_frames']} | {capture['missing_desktop_frames']} | {capture['coalesced_frames']} | {capture['marker_errors']} |")
    lines.extend(["", "| Arm | Workload | All UI Hz | Canvas marker Hz | Canvas PM Hz | Motion / refresh |", "| --- | --- | ---: | ---: | ---: | ---: |"])
    for row in rows:
        status = rate_status(row["capture_quality"]["status"], row["input_quality"]["status"])
        lines.append(f"| {row['arm']} | {row['workload']} | {rate(row['all_ui_changes_hz']['p50'], status)} | {rate(row['canvas_change_hz']['p50'], status)} | {rate(row['canvas_presentmon_display_hz']['p50'], status)} | {rate(row['fraction_refresh'], status)} |")
    lines.extend(["", "| Profile | Arm | Workload | Contact | Desktop Hz / refresh Hz | Missing share | Capture p50 / p95 ms |", "| --- | --- | --- | ---: | ---: | ---: | ---: |"])
    for contact in contacts:
        quality = contact["capture_quality"]
        if quality["status"] == "capture-off":
            continue
        lines.append(f"| {contact['profile']} | {contact['arm']} | {contact['workload']} | {contact['contact']} | {rate(quality['desktop_rate_hz'], rate_status(quality['status'], contact['input_quality']['status']))} / {number(quality['refresh_hz'])} | {number(quality['missing_share'])} | {number(quality['capture_ms']['p50'])} / {number(quality['capture_ms']['p95'])} |")
    input_contacts = [contact for contact in contacts if contact["input_quality"]["streams"]]
    if input_contacts:
        lines.extend(["", "| Arm | Workload | Contact | Stream | Nominal Hz | Achieved Hz | Active seconds | Accepted measured / total | Skipped measured / total | Terminal / all OS inputs |", "| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |"])
        for contact in input_contacts:
            for stream, values in contact["input_quality"]["streams"].items():
                status = rate_status(contact["capture_quality"]["status"], values["status"])
                lines.append(f"| {contact['arm']} | {contact['workload']} | {contact['contact']} | {stream} | {number(values.get('nominal_hz'))} | {rate(values['achieved_hz'], status)} | {number(values.get('measured_seconds'))} | {values.get('accepted_measured', '—')} / {values.get('accepted_total', '—')} | {values.get('skipped_measured_slots', '—')} / {values.get('skipped_slots', '—')} | {values.get('terminal_samples', '—')} / {values.get('input_records', '—')} |")
    thread_rows = [(row, thread, values) for row in rows for thread, values in row["observer_cpu"].items()]
    if thread_rows:
        lines.extend(["", "Observer CPU totals sum the recorded per-thread deltas across contacts; percentages use the same elapsed intervals.", "", "| Arm | Workload | Observer thread | CPU ms | Wall ms | One-core % | Priority | Contacts / final |", "| --- | --- | --- | ---: | ---: | ---: | --- | ---: |"])
        for row, thread, values in thread_rows:
            lines.append(f"| {row['arm']} | {row['workload']} | {thread} | {number(values['cpu_ms'])} | {number(values['wall_ms'])} | {number(values['one_core_percent'])} | {', '.join(values['priorities'])} | {values['contacts_reported']} / {values['final_contacts']} |")
    calibrated = [contact for contact in contacts if contact["probe_calibration"]["status"] != "not-applicable"]
    if calibrated:
        lines.extend(["", "| Arm | Workload | Contact | Calibration | Offset px | Spread px | Rest samples |", "| --- | --- | ---: | --- | ---: | ---: | ---: |"])
        for contact in calibrated:
            calibration = contact["probe_calibration"]
            lines.append(f"| {contact['arm']} | {contact['workload']} | {contact['contact']} | {calibration['status']} | {number(calibration['offset_px'])} | {number(calibration['spread_px'])} | {calibration['samples']} |")
    if abba:
        lines.extend(["", "| Profile | Workload | ABBA valid | Comparison eligible | Arm | Early Hz | Late Hz | Relative change |", "| --- | --- | --- | --- | --- | ---: | ---: | ---: |"])
        for check in abba:
            for arm, drift in check["drift"].items():
                lines.append(f"| {check['profile']} | {check['workload']} | {check['order_valid']} | {check['comparison_eligible']} | {arm} | {rate(drift['early_motion_hz'], rate_status(drift['early_capture_status'], drift['early_input_status']))} | {rate(drift['late_motion_hz'], rate_status(drift['late_capture_status'], drift['late_input_status']))} | {rate(drift['relative_change'], rate_status(drift['comparison_capture_status'], drift['comparison_input_status']))} |")
    if overhead:
        lines.extend(["", "| Profile | Arm | Workload | Comparison eligible | Capture-on canvas Hz | Capture-off canvas Hz | Overhead fraction |", "| --- | --- | --- | --- | ---: | ---: | ---: |"])
        for check in overhead:
            lines.append(f"| {check['profile']} | {check['arm']} | {check['workload']} | {check['comparison_eligible']} | {rate(check['capture_on_canvas_hz'], rate_status(check['capture_on_status'], check['capture_on_input_status']))} | {rate(check['capture_off_canvas_hz'], rate_status(check['capture_off_status'], check['capture_off_input_status']))} | {rate(check['overhead_fraction'], rate_status(check['comparison_capture_status'], check['comparison_input_status']))} |")
    receipt_rows = [row for row in rows if row["receipt_matching"]["statuses"] != ["not-applicable"]]
    if receipt_rows:
        lines.extend(["", "| Arm | Workload | Receipt p99 ms | Matching status / basis | Timestamp identity / declared basis | Comparable / other samples | Native QPC rewritten receipt / consumed | Latest native identity | Latest native checked / rewritten | Receipt unmatched |", "| --- | --- | ---: | --- | --- | ---: | ---: | --- | ---: | ---: |"])
        for row in receipt_rows:
            matching = row["receipt_matching"]
            status = ", ".join(matching["statuses"])
            basis = ", ".join(matching["bases"]) or "unvalidated"
            identity = ", ".join(matching["timestamp_identity"]) or "unmeasured"
            latest_identity = ", ".join(matching.get("native_latest_identity", [])) or "unmeasured"
            source_basis = ", ".join(matching.get("host_timestamp_bases", [])) or "basis not recorded"
            lines.append(f"| {row['arm']} | {row['workload']} | {number(row['input_receipt_latency_ms']['p99'])} | {status} / {basis} | {identity} / {source_basis} | {number(matching.get('timestamp_comparable_samples'))} / {number(matching.get('timestamp_noncomparable_samples'))} | {number(matching['rewritten_receipt_timestamps'])} / {number(matching['rewritten_consumed_timestamps'])} | {latest_identity} | {number(matching.get('native_latest_checked_samples'))} / {number(matching.get('native_latest_rewritten_samples'))} | {row['receipt_unmatched']} |")
    invalid = [contact for contact in contacts if not contact["valid"]]
    if invalid:
        lines.extend(["", "| Arm | Workload | Contact | Validation |", "| --- | --- | ---: | --- |"])
        for contact in invalid:
            reasons = "; ".join(contact["validation_errors"]).replace("|", "\\|")
            lines.append(f"| {contact['arm']} | {contact['workload']} | {contact['contact']} | FAILED/excluded: {reasons} |")
    shown = set()
    marker_rows = []
    for contact in contacts:
        identity = contact["arm"], contact["workload"], contact["profile"]
        if identity in shown or not contact["marker_sample"]:
            continue
        shown.add(identity)
        for sample in contact["marker_sample"]:
            marker_rows.append(f"| {contact['arm']} | {contact['workload']} | {sample['qpc']} | {sample['ui_marker']:#08x} | {sample['state16']} | {sample['ui_frame']} | {sample['pen_samples']} |")
    if marker_rows:
        lines.extend(["", "| Arm | Workload | Capture QPC | UI marker | state16 | ui_frame8 | Pen consumed |", "| --- | --- | ---: | ---: | ---: | ---: | ---: |", *marker_rows])
    lines.extend(["", "Motion rates use captured target geometry. All UI changes use the marker counter separately. Latencies use desktop LastPresentTime; receipt latency uses source indices only after pen-delivery.json validates ordered indices and sample tuples. Injection time always comes from inputs.jsonl QPC; Windows source timestamps are diagnostic: rewrite identity is assessed only for declared native PerformanceCount samples, never by comparing WinRT microseconds with QPC ticks. Latest-native identity separately checks tuple-matched batch endpoints when ordered native history was unavailable; its counts exclude ordered-history comparisons. Drift and black-interior counts use the measured motion windows and omit startup, warmup and settling frames. Process CPU uses guest capacity when logical_cpus is available, otherwise one core.", "", "A contact is capture-limited when missing / (desktop + missing) exceeds 2% or capture p95 exceeds half a refresh period. Every derived rate carries its capture and input status; comparisons retain these warnings. Input is limited when accepted measured data slots / nominal active seconds falls below 90% of the recorded stream nominal Hz. Terminal up events count toward pen delivery but not scheduled-slot rates. Probe calibration uses resting observations and fails above 1 px spread. Full distributions, per-contact values, censored/unmatched counts, provenance and decoded marker samples are in results.json.", ""])
    reasons = sorted({reason for row in rows for reason in row.get("presentmon_unavailable_reasons", [])})
    if reasons:
        lines.extend(["", "PresentMon limits: " + "; ".join(reasons) + ". PresentMode and display timing remain unverified for the approved WARP runtime-only exception; display rates and display-overhead comparisons stay null. Startup idle presents remain unverified when interval rows are absent.", ""])
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    parser.add_argument("--out", type=Path, default=Path("artifacts/windows-ui-poc"))
    args = parser.parse_args()
    directories = sorted(path.parent for path in args.results.rglob("meta.json") if (path.parent / "process.jsonl").exists())
    if not directories:
        raise SystemExit("No observer contacts found")
    contacts = [analyse_contact(directory) for directory in directories]
    pooled = aggregate(contacts)
    args.out.mkdir(parents=True, exist_ok=True)
    abba, overhead = abba_checks(contacts), overhead_checks(pooled)
    data = {"classification": "VM A/B diagnostics", "abba": abba, "capture_overhead": overhead, "pooled": pooled, "contacts": [{key: value for key, value in row.items() if key != "_samples"} for row in contacts]}
    (args.out / "results.json").write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    for vm in sorted({row["vm"] for row in pooled}):
        (args.out / f"results-{vm}.md").write_text(render([row for row in pooled if row["vm"] == vm], [row for row in contacts if row["vm"] == vm], [row for row in abba if row["vm"] == vm], [row for row in overhead if row["vm"] == vm]))
    print(render(pooled, contacts, abba, overhead))
    failed = [row for row in contacts if not row["valid"]]
    if failed:
        raise SystemExit("Invalid/incomplete contacts: " + "; ".join(row["directory"] + ": " + ", ".join(row["validation_errors"]) for row in failed))
    if any(row.get("capture_failure") for row in (contact["capture"] for contact in contacts)):
        raise SystemExit("Capture failed; see results.json")


if __name__ == "__main__":
    main()
