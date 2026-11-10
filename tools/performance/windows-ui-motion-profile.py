import argparse
import collections
import csv
from html.parser import HTMLParser
import importlib.util
import json
import math
import re
import subprocess
from pathlib import Path


def contact_range(events_path, markers, meta):
    provider = "75de640b-8863-4535-b326-443c2307f3e0"
    found = subprocess.run(["rg", "-i", "-F", provider, str(events_path)], check=True, capture_output=True, text=True)
    events = []
    for row in csv.reader(found.stdout.splitlines(), skipinitialspace=True):
        owner = re.fullmatch(r"windows-ui-motion\.exe\s+\(\s*(\d+)\)", row[2].strip(), re.I) if len(row) > 5 else None
        if row[0].strip() == "UnknownEvent/Crimson" and owner and row[5].strip("{} ").lower() == provider:
            events.append((float(row[1]), int(owner[1]), int(row[3])))
    events.sort()
    if len(events) != 2 or events[0][1:] != events[1][1:]:
        raise ValueError("Expected exactly two external marker events from one observer thread")
    frequency = meta["qpc_frequency"]
    matches = []
    for unit, scale in (("s", 1), ("ms", 1000), ("us", 1000000), ("ns", 1000000000)):
        bounds = [(event[0] / scale - markers[name][1] / frequency, (event[0]+1) / scale - markers[name][0] / frequency)
                  for event, name in zip(events, ("begin", "end"))]
        low, high = max(bound[0] for bound in bounds), min(bound[1] for bound in bounds)
        if low <= high:
            matches.append((unit, low, high))
    if len(matches) != 1:
        raise ValueError("ETW marker timestamps need one unambiguous unit and intersecting QPC write brackets")
    unit, low, high = matches[0]
    offset = (low + high) / 2
    return {"start": meta["motion_start_qpc"] / frequency + offset,
            "end": meta["motion_end_qpc"] / frequency + offset,
            "qpc_offset_seconds": offset, "event_timestamp_units": unit, "uncertainty_us": (high-low)*5e5,
            "marker_process_id": events[0][1], "marker_thread_id": events[0][2]}


def module_group(module):
    name = module.replace("\\", "/").rsplit("/", 1)[-1].lower()
    return {"microsoft.ui.xaml.dll": "Microsoft.UI.Xaml.dll", "capycanvas.exe": "CapyCanvas.exe",
            "layer_windows.dll": "layer_windows.dll", "windows.data.json.dll": "WinRT JSON", "windows.web.dll": "WinRT JSON",
            "dwmcorei.dll": "dwmcorei.dll", "dwmcore.dll": "dwmcorei.dll"}.get(name, "other")


class StackTables(HTMLParser):
    def __init__(self):
        super().__init__()
        self.tables, self.pending, self.active, self.row, self.cell = {}, None, None, None, None

    def handle_starttag(self, tag, attributes):
        if tag == "a" and "id" in dict(attributes):
            value = dict(attributes)["id"]
            self.pending = value if value in ("TblP", "TblME", "TblSI") else None
        elif tag == "table":
            self.active, self.pending = self.pending, None
            if self.active:
                self.tables[self.active] = []
        elif self.active and tag == "tr":
            self.row = []
        elif self.active and tag in ("td", "th"):
            self.cell = []

    def handle_data(self, value):
        if self.cell is not None:
            self.cell.append(value)

    def handle_endtag(self, tag):
        if tag in ("td", "th") and self.cell is not None:
            self.row.append("".join(self.cell).replace("\xa0", " ").strip())
            self.cell = None
        elif tag == "tr" and self.active and self.row is not None:
            self.tables[self.active].append(self.row)
            self.row = None
        elif tag == "table":
            self.active = None


def stack_summary(tables, process_id, cpu_ms, inputs, expected_samples):
    if not math.isfinite(cpu_ms) or cpu_ms <= 0 or inputs <= 0 or expected_samples <= 0:
        raise ValueError("A profile needs finite positive CPU, input and sample counts")
    process = [row for row in tables["TblP"][1:] if row[1]]
    if len(process) != 1 or process[0][1] != str(process_id):
        raise ValueError("Stack report must contain only the owned process")
    hits = int(process[0][2])
    modules = {row[0]: int(row[1]) for row in tables["TblME"][1:]}
    missing = expected_samples-hits
    if not hits or missing < -1 or missing/expected_samples > .02 or sum(modules.values()) != hits:
        raise ValueError("Stack hits do not match the raw SampledProfile events for this UI thread/window")
    functions = [{"function": row[0], "hits": int(row[1]), "exclusive_hits": int(row[3]), "base": row[4]}
                 for row in tables["TblSI"][1:]]
    if sum(row["exclusive_hits"] for row in functions) != hits:
        raise ValueError("Function leaf weights do not cover the sampled UI CPU")
    groups = collections.Counter()
    for name, count in modules.items():
        groups[module_group(name)] += count
    if missing > 0:
        groups["unattributed stack"] += missing
    for row in functions:
        row.update(share=row["hits"]/expected_samples, cpu_ms=cpu_ms*row["hits"]/expected_samples, ms_per_input=cpu_ms*row["hits"]/expected_samples/inputs)
    return {"basis": "Uni-inclusive SampledProfile shares scaled to external raw UI CPU; inclusive functions overlap; module leaf shares are disjoint",
            "cpu_ms": cpu_ms, "inputs": inputs, "sample_hits": hits, "raw_cpu_samples": expected_samples,
            "missing_stack_samples": max(0,missing), "stack_coverage": hits/expected_samples,
            "resolved_leaf_share": sum(row["exclusive_hits"] for row in functions if "unknown" not in row["function"].lower())/expected_samples,
            "module_groups": {name: {"share": count/expected_samples, "cpu_ms": cpu_ms*count/expected_samples} for name, count in groups.most_common()},
            "modules": {name: {"hits": count, "share": count/expected_samples, "cpu_ms": cpu_ms*count/expected_samples} for name, count in modules.items()},
            "inclusive_functions": sorted(functions, key=lambda row: (-row["hits"], row["function"], row["base"]))}


def cpu_samples(events_path, meta, window):
    pattern = rf"^ *SampledProfile,.*\( *{meta['process_id']}\), *{meta['ui_thread_id']},"
    found = subprocess.run(["rg", pattern, str(events_path)], check=True, capture_output=True, text=True)
    scale = {"s": 1, "ms": 1000, "us": 1000000, "ns": 1000000000}[window["event_timestamp_units"]]
    count = 0
    for row in csv.reader(found.stdout.splitlines(), skipinitialspace=True):
        if window["start"] <= float(row[1])/scale <= window["end"]:
            if int(row[8]) != 1:
                raise ValueError("Batched CPU samples require per-sample weight export")
            count += 1
    return count


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("run", type=Path)
    parser.add_argument("--stack", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    contacts = list(args.run.glob("0-control-*/contact-0/meta.json"))
    if len(contacts) != 1:
        raise ValueError("Expected one profiled contact")
    contact = contacts[0].parent
    meta = json.loads((contact/"meta.json").read_text())
    events = args.run/("cpu-events.csv" if (args.run/"cpu-events.csv").exists() else "events.csv")
    window = contact_range(events, json.loads((contact/"profile-markers.json").read_text()), meta)
    spec = importlib.util.spec_from_file_location("motion_report", Path(__file__).with_name("windows-ui-motion-report.py"))
    report = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(report)
    cpu = report.cpu_usage(json.loads((contact/"cpu.json").read_text()), meta)
    inputs = len(report.read_lines(contact/"inputs.jsonl"))
    tables = StackTables()
    tables.feed(args.stack.read_text(encoding="utf-8-sig"))
    result = stack_summary(tables.tables, meta["process_id"], cpu["ui_ms"], inputs, cpu_samples(events, meta, window))
    result.update(window=window, process_id=meta["process_id"], ui_thread_id=meta["ui_thread_id"], workload=meta["workload"], stack_report=str(args.stack))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2)+"\n")
    lines = [f'# {meta["workload"]}: control UI CPU', '', result['basis']+'.', '',
             f'UI CPU {result["cpu_ms"]:.2f} ms; {inputs} injected steps; stack coverage {result["stack_coverage"]:.2%}; resolved leaf symbols {result["resolved_leaf_share"]:.2%}. Missing stack samples remain unattributed.', '',
             '| Leaf module group | UI share | Estimated ms/contact |', '| --- | ---: | ---: |']
    lines += [f'| {name} | {value["share"]:.2%} | {value["cpu_ms"]:.2f} |' for name,value in result['module_groups'].items()]
    lines += ['', '| Top 25 inclusive functions (RVA) | UI share | Estimated ms/injected step |', '| --- | ---: | ---: |']
    for row in result['inclusive_functions'][:25]:
        name = row['function'].replace('|', r'\|')
        lines.append(f'| `` {name} ({row["base"]}) `` | {row["share"]:.2%} | {row["ms_per_input"]:.4f} |')
    lines += ['', f'Stack report: `{args.stack}`. Exact contact range: {window["start"]:.7f}–{window["end"]:.7f} s; marker correlation uncertainty {window["uncertainty_us"]:.2f} us. Full function/module values remain in the adjacent JSON.']
    args.out.with_suffix('.md').write_text('\n'.join(lines)+'\n')
    print(json.dumps({key: result[key] for key in ("workload", "cpu_ms", "sample_hits", "resolved_leaf_share")}))


if __name__ == "__main__":
    main()
