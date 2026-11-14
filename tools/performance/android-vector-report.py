#!/usr/bin/env python3
"""Summarize android-vector-benchmark.py output: presented canvas frames, GPU
completions, drag-start and command response times per unit and run.

Canvas presents join the SurfaceView's latchBuffer slices to SurfaceFlinger's
actual display-frame present times. Renderer rates count GPU-completed canvas
submissions; neither is physical input-to-photon latency.
"""
import argparse
import csv
import io
import json
import math
import pathlib
import re
import subprocess

MOTION = ("move-", "scale-", "rotate-", "points-", "erase-", "width-", "pan", "pinch", "rotate-view", "thumb-pan", "thumb-stroke", "nothumb-pan", "nothumb-stroke")


def quantile(values, q):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * q) - 1)] if values else None


def describe(values):
    return {"n": len(values), "p50": quantile(values, .5), "p95": quantile(values, .95),
            "p99": quantile(values, .99), "max": max(values) if values else None}


def presents(processor, trace, package):
    query = f"""SELECT s.name latch, f.ts + f.dur presented_at
FROM slice s JOIN ancestor_slice(s.id) a ON a.name GLOB 'commit [0-9]*'
JOIN actual_frame_timeline_slice f ON f.layer_name IS NULL AND CAST(f.name AS INT) = CAST(SUBSTR(a.name, 8) AS INT)
WHERE s.name GLOB 'latchBuffer *SurfaceView*{package}/*' ORDER BY presented_at"""
    out = subprocess.run([processor, "query", str(trace), query], capture_output=True, text=True, check=True).stdout
    rows = list(csv.DictReader(io.StringIO(out[out.index('"latch"'):]))) if '"latch"' in out else []
    return {int(r["latch"].rsplit(" ", 1)[1]): int(r["presented_at"]) for r in rows}


def window(report):
    motion = report["motion"]
    injected = motion.get("injected")
    if injected and len(injected) > 2:
        return injected[1][0], injected[-2][0]
    if motion.get("values"):
        return motion["values"][0][0], motion["values"][-1][0]
    return motion["begin_ns"], motion["end_ns"]


def motion_run(report, shown):
    begin, end = window(report)
    offset = report["motion"]["begin_boot_ns"] - report["motion"]["begin_ns"]
    seconds = (end - begin) / 1e9
    done = sorted(row[2] for row in report["completions"] if begin <= row[1] <= end)
    inside = sorted({shown[row[0]] for row in report["completions"] if begin <= row[1] <= end and row[0] in shown})
    result = {"seconds": seconds,
              "renderer_per_s": len(done) / seconds,
              "renderer_gap_ms": describe([(b - a) / 1e6 for a, b in zip(done, done[1:])]),
              "presented_per_s": (len(inside) - 1) * 1e9 / (inside[-1] - inside[0]) if len(inside) > 1 else None,
              "presented_gap_ms": describe([(b - a) / 1e6 for a, b in zip(inside, inside[1:])])}
    events = [dict(zip(report["input_fields"], row)) for row in report["inputs"]]
    moves = sorted((e for e in events if e["phase"] == 2), key=lambda e: e["event_ns"])
    frames = [dict(zip(report["frame_fields"], row)) for row in report["frames"]]
    moving = [f for f in frames if begin <= f["start_ns"] <= end]
    result["owner_cpu_ms"] = describe([f["owner_thread_cpu_ns"] / 1e6 for f in moving])
    result["input_cpu_ms"] = describe([e["cpu_input_ns"] / 1e6 for e in events if begin <= e["arrival_ns"] <= end])
    result["settled_after_release_ms"] = (report["settled_ns"] - report["motion"]["end_ns"]) / 1e6
    injected = report["motion"].get("injected")
    if injected and moves:
        down = injected[0][0]
        processed = moves[0]["worker_start_ns"] + moves[0]["cpu_input_ns"]
        first = next((row for row in sorted(report["completions"], key=lambda r: r[1]) if row[1] >= processed), None)
        if first:
            result["press_to_first_moving_gpu_ms"] = (first[2] - down) / 1e6
            result["press_to_first_moving_present_ms"] = (shown[first[0]] - offset - down) / 1e6 if first[0] in shown else None
    return result


def response_run(report, shown):
    motion = report["motion"]
    offset = motion["begin_boot_ns"] - motion["begin_ns"]
    injected = motion.get("injected")
    start = motion.get("notice_answered_ns") or (injected[-1][0] if injected else motion["begin_ns"])
    rows = sorted(report["completions"], key=lambda r: r[1])
    raster = max([row[3] for row in rows if row[1] < start] or [int(report["renderer_before"]["frames"])])
    first = next((row for row in rows if row[1] >= start and row[3] > raster), None)
    last = [row for row in rows if start <= row[1] <= report["settled_ns"]]
    result = {"first_update_ms": (first[2] - start) / 1e6 if first else None,
              "complete_ms": (max(report["settled_ns"], last[-1][2] if last else 0) - start) / 1e6,
              "settled_ms": (report["settled_ns"] - start) / 1e6}
    if last and last[-1][0] in shown:
        result["complete_present_ms"] = (shown[last[-1][0]] - offset - start) / 1e6
    if first and first[0] in shown:
        result["first_update_present_ms"] = (shown[first[0]] - offset - start) / 1e6
    for key in ("parsed_ns", "adopted_ns"):
        if key in motion:
            result[key.replace("_ns", "_ms")] = (motion[key] - motion["begin_ns"]) / 1e6
    if "adopted_ns" in motion:
        after = next((row for row in rows if row[1] >= motion["adopted_ns"]), None)
        result["first_frame_ms"] = (after[2] - motion["begin_ns"]) / 1e6 if after else None
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("directory", type=pathlib.Path)
    p.add_argument("--processor", required=True, help="Perfetto trace_processor_shell")
    p.add_argument("--package", required=True)
    args = p.parse_args()
    summary = {}
    for info in sorted(args.directory.glob("*-info.json")):
        name = info.name.removesuffix("-info.json")
        match = re.match(r"(.+-\d{14})-(.+)$", name)
        if not match:
            continue
        label, unit = match.groups()
        trace = args.directory / f"{label}-{unit}.perfetto-trace"
        shown = presents(args.processor, trace, args.package) if trace.exists() else {}
        runs = {}
        thumbnails = {}
        for path in sorted(args.directory.glob(f"{label}-{unit}-*.json")):
            run = path.name.removeprefix(f"{label}-{unit}-").removesuffix(".json")
            if run == "info" or run.startswith("prime") or run.endswith("state"):
                continue
            report = json.loads(path.read_text())
            if run.endswith("-thumbnail"):
                thumbnails[run.removesuffix("-thumbnail")] = report.get("latency_ms")
                continue
            if "motion" not in report:
                continue
            runs[run] = motion_run(report, shown) if unit.startswith(MOTION) else response_run(report, shown)
        for run, latency in thumbnails.items():
            if run in runs and latency is not None:
                runs[run]["thumbnail_ms"] = latency
        environment = " ".join((args.directory / f"{label}-{unit}-environment-{when}.txt").read_text()
                               for when in ("before", "after") if (args.directory / f"{label}-{unit}-environment-{when}.txt").exists())
        summary[f"{label}/{unit}"] = {"thermal_status": sorted(set(re.findall(r"Thermal Status: (\d)", environment))), "runs": runs}
    (args.directory / "vector-summary.json").write_text(json.dumps(summary, indent=2))

    def spread(values):
        values = sorted(v for v in values if v is not None)
        return f"{values[len(values) // 2]:.1f} [{values[0]:.1f}–{values[-1]:.1f}]" if values else "—"
    for key, value in summary.items():
        parts = sorted({name.partition("-")[2] for name in value["runs"]})
        for part in parts:
            runs = [r for name, r in value["runs"].items() if name.partition("-")[2] == part]
            fields = sorted({k for r in runs for k, v in r.items() if not isinstance(v, dict)} - {"seconds"})
            gaps = sorted({k for r in runs for k, v in r.items() if isinstance(v, dict)})
            print(f"{key}{'/' + part if part else ''} thermal {value['thermal_status']} runs {len(runs)}")
            for field in fields:
                print(f"  {field}: {spread(r.get(field) for r in runs)}")
            for field in gaps:
                print(f"  {field} p99: {spread(r[field]['p99'] for r in runs if field in r)}")


if __name__ == "__main__":
    main()
