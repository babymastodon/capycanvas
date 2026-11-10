import argparse
import bisect
import collections
import csv
import json
import math
import subprocess
from pathlib import Path


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


def drag_interior(value, low, high):
    return high > low and low + .05 * (high-low) < value < high - .05 * (high-low)


def interior_motion_cadence(frames, inputs, field, frequency, offset=0):
    requested=[sample for sample in inputs if sample.get('expected_value') is not None]
    segments=collections.defaultdict(list)
    for sample in requested:
        if sample.get('segment') is not None:
            segments[sample['segment']].append(sample)
    requested_directions=[]
    for samples in segments.values():
        delta=samples[-1]['expected_value']-samples[0]['expected_value']
        requested_directions.append(1 if delta>0 else -1 if delta<0 else 0)
    observed_directions=[]
    expected_turns=max(0,len(segments)-1)
    intervals,turns,off_peak_turns=[],0,0
    if not requested:
        return {'qualified':False,'interval_ms':intervals,'expected_turns':expected_turns,
                'observed_turns':turns,'off_peak_turns':off_peak_turns}
    low=min(sample['expected_value'] for sample in requested)
    high=max(sample['expected_value'] for sample in requested)
    previous=None
    previous_direction=0
    for frame in frames:
        value=frame.get(field)
        if value is None:
            previous=None;previous_direction=0
            continue
        value-=offset
        tick=frame_time(frame)
        if previous is not None and value!=previous[1]:
            direction=1 if value>previous[1] else -1
            if previous_direction==direction:
                if drag_interior(previous[1],low,high) and drag_interior(value,low,high):
                    intervals.append((tick-previous[0])*1000/frequency)
            elif previous_direction:
                turns+=1
                off_peak_turns+=int(previous[1]<high-.05*(high-low) if previous_direction>0 else previous[1]>low+.05*(high-low))
            if direction!=previous_direction:
                observed_directions.append(direction)
            previous_direction=direction
        elif previous is not None:
            continue
        previous=(tick,value)
    direction_match=observed_directions==requested_directions and 0 not in requested_directions
    return {'qualified':bool(intervals) and bool(segments) and turns==expected_turns and not off_peak_turns and direction_match,
            'interval_ms':intervals,'expected_turns':expected_turns,
            'observed_turns':turns,'off_peak_turns':off_peak_turns,'direction_match':direction_match}


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
            if not drag_interior(value,low,high):
                censored += 1
                continue
            match = next((sample for sample in reversed(samples) if sample["qpc"] <= tick and abs(sample["expected_value"] - value) <= 1), None)
            if match is None:
                unmatched += 1
            else:
                values.append((tick - match["qpc"]) * 1000 / frequency)
    return values, censored, unmatched


def read_json(path):
    raw=path.read_bytes()
    try:
        text=raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        text=raw.decode("cp1252")
    return json.loads(text)


def read_lines(path):
    return [json.loads(line) for line in path.read_text(encoding="utf-8-sig").splitlines() if line.strip()]


def interpolate(rows, tick, field):
    stamps = [row["qpc"] for row in rows]
    i = bisect.bisect_left(stamps, tick)
    if i < len(rows) and stamps[i] == tick:
        return rows[i][field]
    if i == 0 or i == len(rows):
        raise ValueError("CPU samples do not bracket the requested contact")
    left, right = rows[i-1], rows[i]
    return left[field] + (right[field]-left[field])*(tick-left["qpc"])/(right["qpc"]-left["qpc"])


def cpu_usage(rows, meta):
    start, end, idle = (meta[key] for key in ("motion_start_qpc", "motion_end_qpc", "idle_start_qpc"))
    frequency = meta["qpc_frequency"]
    duration, idle_seconds = (end-start)/frequency, (start-idle)/frequency
    total = interpolate(rows, end, "ui_cpu_ms")-interpolate(rows, start, "ui_cpu_ms")
    idle_ms = interpolate(rows, start, "ui_cpu_ms")-interpolate(rows, idle, "ui_cpu_ms")
    process = interpolate(rows, end, "cpu_ms")-interpolate(rows, start, "cpu_ms")
    idle_process = interpolate(rows, start, "cpu_ms")-interpolate(rows, idle, "cpu_ms")
    return {"ui_ms": total, "idle_ui_ms_per_s": idle_ms/idle_seconds,
            "active_ui_ms": max(0, total-idle_ms*duration/idle_seconds),
            "process_ms": process, "idle_process_ms_per_s": idle_process/idle_seconds,
            "active_process_ms": max(0, process-idle_process*duration/idle_seconds),
            "busy_percent": total/duration/10,
            "private_bytes": max(row["private_bytes"] for row in rows if start <= row["qpc"] <= end)}


def probe_frames(frames, name):
    return [{**frame, **frame["probes"][name]} for frame in frames if name in frame["probes"]]


def visible_changes(frames, field, start, end):
    if field != "hash":
        return changes(frames,field,start,end)
    return [frame_time(f) for f in frames if start <= frame_time(f) <= end and f["changed_pixels"] > 2]


def discrete_latencies(frames, inputs, frequency, end, targets=None):
    responses, completions, windows, missing, incomplete = [], [], [], 0, 0
    for i, sample in enumerate(inputs):
        tick = sample["qpc"]
        limit = inputs[i+1]["qpc"] if i+1 < len(inputs) else end
        observed = [f for f in frames if tick <= frame_time(f) < limit and f["valid"]]
        target = targets[i] if targets else None
        changed = [f for f in observed if f["changed_pixels"] > 2]
        if not changed:
            missing += 1
            previous=max((f for f in frames if frame_time(f)<tick and f["valid"]),key=frame_time,default=None)
            if targets and (previous is None or previous["hash"]!=target):
                reached=next((frame_time(f) for f in observed if f["hash"]==target),None)
                if reached is None:
                    incomplete+=1
                else:
                    completions.append((reached-tick)*1000/frequency)
            continue
        first = frame_time(changed[0])
        complete = next((frame_time(f) for f in observed if f["hash"] == target),None) if targets else None
        responses.append((first-tick)*1000/frequency)
        if complete is not None:
            completions.append((complete-tick)*1000/frequency)
            windows.append((tick, complete))
        elif targets:
            incomplete += 1
        else:
            windows.append((tick,frame_time(changed[-1])))
    return responses, completions, windows, missing, incomplete


def summarize_contact(path):
    meta, plan = read_json(path/"meta.json"), read_json(path/"plan.json")
    inputs, cpu = read_lines(path/"inputs.jsonl"), read_json(path/"cpu.json")
    frequency = meta["qpc_frequency"]
    start, end = meta["motion_start_qpc"], meta["motion_end_qpc"]
    seconds = (end-start)/frequency
    workload = meta["workload"]
    nominal = meta.get("nominal_hz",{"resize-native":120, "resize-programmatic":120, "wheel":60, "drawer":2.5, "expansion":1/1.2, "typing":10}.get(workload, 240))
    input_share = len(inputs)/(seconds*nominal)
    usage = cpu_usage(cpu, meta)
    result = {"path":str(path), "workload":workload, "input_count":len(inputs), "input_share":input_share,
              "nominal_hz":nominal, "cpu":usage, "probes":{}, "errors":[]}
    if input_share < .9:
        result["errors"].append("input below 90% nominal")
    if not (path/"completed.json").exists():
        result["errors"].append("observer did not complete")
    if meta["no_capture"]:
        result["capture"] = "off"
        result["valid"] = not result["errors"]
        return result
    frames = read_lines(path/"frames.jsonl")
    timed = [f for f in frames if start <= frame_time(f) <= end]
    missing = sum(max(0, f["accumulated_frames"]-1) for f in timed)
    missing_share = missing/(len(timed)+missing) if timed else 1
    cost = quantiles(f["capture_ms"] for f in timed)
    result["capture"] = {"missing_share":missing_share, "capture_ms":cost}
    if missing_share > .02 or cost["p95"] is None or cost["p95"] > 500/meta["refresh_hz"]:
        result["errors"].append("capture inadequate")
    for spec in plan["probes"]:
        name, kind = spec["name"], spec["kind"]
        observed = probe_frames(frames, name)
        if kind == "black-grid":
            result["black_samples"] = quantiles(f["black"] for f in observed if start <= frame_time(f) <= end)
            continue
        rest = [f for f in observed if meta["idle_start_qpc"] <= frame_time(f) < start]
        if not rest:
            rest = [max((f for f in observed if frame_time(f) < start), key=frame_time, default=None)]
            rest = [f for f in rest if f]
        positions = [f["position"] for f in rest if f["valid"] and f["position"] is not None]
        calibration = {"samples":len(rest), "spread_px":max(positions)-min(positions) if positions else None}
        if not rest or not all(f["valid"] for f in rest) or (positions and calibration["spread_px"] > 1):
            result["errors"].append(f"{name}: resting calibration failed")
        good = [f for f in observed if f["valid"]]
        invalid = sum(not f["valid"] for f in observed if start <= frame_time(f) <= end)
        probe_missing_share=(missing+invalid)/(len(timed)+missing) if timed else 1
        if probe_missing_share>.02:
            result["errors"].append(f"{name}: {invalid} low-confidence frames; total missing {probe_missing_share:.2%}")
        discrete = workload in ("drawer", "expansion", "typing")
        motion_cadence={'qualified':False,'interval_ms':[]}
        if discrete:
            endpoints=spec.get("endpoints",[])
            targets=[endpoints[1 if sample["slot"]%2==0 else 0] for sample in inputs] if len(endpoints)==2 else None
            if workload=="typing" and len(endpoints)==7:
                targets=[endpoints[(sample["slot"]%12)+1 if sample["slot"]%12<6 else 11-sample["slot"]%12] for sample in inputs]
            if workload=="typing" and (len(endpoints)!=7 or (name=="field" and len(set(endpoints))!=7)):
                result["errors"].append(f"{name}: missing calibrated typing states")
            response, completion, windows, no_change, incomplete = discrete_latencies(good, inputs, frequency, end+int(.4*frequency),targets)
            if workload in ("drawer","expansion") and not targets:
                result["errors"].append(f"{name}: missing calibrated endpoints")
            stamps = visible_changes(good, "hash", start, end)
            duration = seconds if workload=="typing" else sum((b-a)/frequency for a,b in windows)
            intervals = [(b-a)*1000/frequency for lo,hi in windows for a,b in zip([s for s in stamps if lo<=s<=hi], [s for s in stamps if lo<=s<=hi][1:])]
        else:
            stamps = visible_changes(good, "position" if kind in ("edge","shift") else "hash", start, end)
            duration = seconds
            intervals = [(b-a)*1000/frequency for a,b in zip(stamps,stamps[1:])]
            response, completion, no_change, incomplete = [], [], 0, 0
            if kind == "edge":
                converted = [{**sample, "expected_value":sample["expected"]} for sample in inputs if sample["expected"] is not None]
                axis = plan["axis"]
                offset = spec["anchor"]-plan["point"][axis]
                response, censored, unmatched = latency_drag(good, converted, "position", frequency, offset)
                motion_cadence=interior_motion_cadence(
                    [frame for frame in good if start<=frame_time(frame)<=end],
                    converted,"position",frequency,offset)
                terminal = read_json(path/"terminal.json")
                reached = next((frame_time(f) for f in good if frame_time(f) >= terminal["qpc"] and f["position"] is not None and abs(f["position"]-spec["anchor"]) <= 1),None)
                if reached is not None:
                    completion = [(reached-terminal["qpc"])*1000/frequency]
                actual = [f["position"] for f in good if start <= frame_time(f) <= end and f["position"] is not None]
                expected = [s["expected"] for s in inputs if s["expected"] is not None]
                if not actual or not expected or max(actual)-min(actual) < .9*(max(expected)-min(expected)):
                    result["errors"].append(f"{name}: commanded range not displayed")
        row = {"calibration":calibration, "changes":len(stamps), "motion_seconds":duration,
               "rejected_probe_frames":invalid,"total_missing_share":probe_missing_share,
               "changes_per_s":len(stamps)/duration if duration else None, "interval_ms":quantiles(intervals),
               "response_ms":quantiles(response), "completion_ms":quantiles(completion), "events_without_roi_change":no_change,
               "incomplete_events":incomplete,
               "ui_ms_per_change":usage["active_ui_ms"]/len(stamps) if stamps else None,
               "ui_ms_per_input":usage["active_ui_ms"]/len(inputs) if inputs else None}
        row['interior_motion_cadence']={key:value for key,value in motion_cadence.items() if key!='interval_ms'}
        row["samples"] = {"interval_ms":intervals,"interior_interval_ms":motion_cadence['interval_ms'],
                          "response_ms":response,"completion_ms":completion}
        if not stamps:
            result["errors"].append(f"{name}: no displayed motion")
        if workload in ("drawer","expansion") and no_change:
            result["errors"].append(f"{name}: {no_change} events without a response")
        if discrete and targets and incomplete:
            result["errors"].append(f"{name}: {incomplete} calibrated targets not displayed")
        result["probes"][name] = row
    if workload=="divider":
        result["bounded_rate"]=divider_rate_bounds(result,divider_loss_window(probe_frames(frames,"edge"),start,end),seconds,
            cost["p95"] is not None and cost["p95"]<=500/meta["refresh_hz"])
    result["valid"] = not result["errors"]
    return result


def divider_loss_window(frames,start,end):
    pairs=[(a,b) for a,b in zip(frames,frames[1:]) if frame_time(a)<end and frame_time(b)>=start]
    lost=[(a,b) for a,b in pairs if b["accumulated_frames"]>1]
    leading=sum(frame_time(a)<start and a["position"]!=b["position"] for a,b in lost)
    covered=bool(frames and frame_time(frames[0])<start and frame_time(frames[-1])>=end
                 and all(a.get("valid") and b.get("valid") and a.get("position") is not None and b.get("position") is not None for a,b in pairs))
    return {"window_bracketed":covered,"ambiguous_leading_changes":leading,
            "missed_presents":sum(b["accumulated_frames"]-1 for _,b in lost),
            "equal_endpoint_missed_gaps":sum(a["position"]==b["position"] for a,b in lost)}


def divider_rate_bounds(contact,window,seconds,capture_budget_ok):
    edge=contact["probes"].get("edge",{})
    missed=window["missed_presents"];equal=window["equal_endpoint_missed_gaps"]
    loss_error=f'edge: 0 low-confidence frames; total missing {edge.get("total_missing_share",0):.2%}'
    qualified=bool(capture_budget_ok and window["window_bracketed"] and not window["ambiguous_leading_changes"] and seconds>0 and edge.get("changes",0)>0
        and edge.get("rejected_probe_frames") == 0
        and not set(contact["errors"])-{"capture inadequate",loss_error})
    return {**window,"qualified":qualified,"counted_changes":edge.get("changes",0),
            "motion_seconds":seconds,"lower":edge.get("changes",0)/seconds if seconds>0 else None,
            "upper":(edge.get("changes",0)+missed+equal)/seconds if seconds>0 else None}


def pooled_rate_bounds(bounds):
    if not bounds or not all(row.get("qualified",False) for row in bounds):
        return {"qualified":False}
    counted=sum(row["counted_changes"] for row in bounds)
    missed=sum(row["missed_presents"] for row in bounds)
    equal=sum(row.get("equal_endpoint_missed_gaps",0) for row in bounds)
    seconds=sum(row["motion_seconds"] for row in bounds)
    return {"qualified":True,"counted_changes":counted,"missed_presents":missed,"equal_endpoint_missed_gaps":equal,
            "motion_seconds":seconds,"lower":counted/seconds,"upper":(counted+missed+equal)/seconds}


def compare_rate_bounds(slots,order,noise=None):
    result={"status":"unmeasured","slots":slots}
    if len(slots)!=4 or not all(slot.get("qualified",False) for slot in slots):
        return result
    control=pooled_rate_bounds([slots[0],slots[3]])
    candidate=pooled_rate_bounds(slots[1:3])
    result.update(control=control,candidate=candidate)
    if order=="aa":
        result.update(status="A/A",noise=max(row["upper"] for row in slots)-min(row["lower"] for row in slots))
    elif order=="abba":
        if noise is None or not math.isfinite(noise) or noise<0:
            result["status"]="no A/A noise control"
            return result
        upper=max(slots[i]["upper"] for i in (0,3))
        drift=upper-min(slots[i]["lower"] for i in (0,3))
        threshold=max(noise,drift)
        result.update(status="gain" if candidate["lower"]>upper+threshold else "no qualified gain",
                      noise=noise,control_upper=upper,bounded_control_drift=drift,threshold=threshold)
    return result


def compare_abba(slots):
    if [s["arm"] for s in slots] != ["control", "candidate", "candidate", "control"]:
        raise ValueError("Expected control,candidate,candidate,control slots")
    values = [quantiles(s["values"])["p50"] for s in slots]
    if any(v is None for v in values):
        raise ValueError("Every ABBA slot needs observations")
    low, high = sorted((values[0],values[3]))
    candidate = quantiles(slots[1]["values"]+slots[2]["values"])["p50"]
    threshold = max(high-low, slots[0].get("aa_noise",0))
    return {"control_slots":values[::3], "candidate":candidate, "drift":high-low,
            "threshold":threshold, "improves_lower":candidate < low-threshold,
            "regresses_lower":candidate > high+threshold, "improves_higher":candidate > high+threshold,
            "regresses_higher":candidate < low-threshold}


def compare_metrics(slots,control,candidate,order,key,noise):
    result={}
    for metric in slots[0]:
        values=[row[metric] for row in slots]
        if any(v is None for v in values):
            result[metric]={"status":"unmeasured"}
            continue
        aa_noise=max(values)-min(values) if order=="aa" else (noise or {}).get(key,{}).get("metrics",{}).get(metric,{}).get("noise")
        if aa_noise is None:
            result[metric]={"status":"no A/A noise control"}
            continue
        threshold=max(aa_noise,abs(values[0]-values[3]))
        low,high=sorted((values[0],values[3]));value=candidate[metric]
        higher=metric in ("changes_per_s","renderer_frames_per_s")
        gain=(value>high+threshold) if higher else (value<low-threshold)
        regression=(value<low-threshold) if higher else (value>high+threshold)
        result[metric]={"noise":aa_noise,"drift":abs(values[0]-values[3]),"control":control[metric],"candidate":value,"threshold":threshold,"status":"A/A" if order=="aa" else "gain" if gain else "regression" if regression else "within noise"}
    return result


def pooled(contacts, probe):
    rows = [c["probes"][probe] for c in contacts]
    duration = sum(row["motion_seconds"] for row in rows)
    changes_count = sum(row["changes"] for row in rows)
    active_cpu = sum(c["cpu"]["active_ui_ms"] for c in contacts)
    metrics = {"changes_per_s":changes_count/duration if duration else None,
               "ui_ms_per_change":active_cpu/changes_count if changes_count else None,
               "process_ms_per_change":sum(c["cpu"]["process_ms"] for c in contacts)/changes_count if changes_count else None,
               "active_process_ms_per_change":sum(c["cpu"]["active_process_ms"] for c in contacts)/changes_count if changes_count else None,
               "ui_ms_per_input":active_cpu/sum(c["input_count"] for c in contacts),
               "process_ms_per_input":sum(c["cpu"]["process_ms"] for c in contacts)/sum(c["input_count"] for c in contacts),
               "active_process_ms_per_input":sum(c["cpu"]["active_process_ms"] for c in contacts)/sum(c["input_count"] for c in contacts),
               "private_bytes":quantiles(c["cpu"]["private_bytes"] for c in contacts)["p50"]}
    for key in ("interval_ms","interior_interval_ms","response_ms","completion_ms"):
        qualified=key!='interior_interval_ms' or all(row.get('interior_motion_cadence',{}).get('qualified') for row in rows)
        values = quantiles(v for row in rows for v in row['samples'].get(key,[]) if qualified)
        for q in ("p50","p95","p99"):
            metrics[f"{key}_{q}"] = values[q]
    return metrics


def aggregate(root, contacts, noise=None, pen=None, bounded_noise=None):
    order_path = root/"order.json"
    if not order_path.exists():
        return {"errors":["Missing ABBA order/provenance"],"workloads":{}}
    order = read_json(order_path)
    slots = collections.defaultdict(list)
    errors = []
    for row in contacts:
        folder = Path(row["path"]).relative_to(root).parts[0]
        index = int(folder.split("-",1)[0])
        slots[(row["workload"],index)].append(row)
    output = {};bounded_output={}
    expected_slots = len(order["slots"])
    for workload in order["workloads"]:
        if workload == "pen":
            continue
        by_slot = [slots[(workload,i)] for i in range(expected_slots)]
        if workload=="divider" and expected_slots==4:
            bounds=[pooled_rate_bounds([row.get("bounded_rate",{}) for row in rows]) if len(rows)==3 else {"qualified":False} for rows in by_slot]
            closed=all((Path(row["path"]).parent/"completed.json").exists()
                       and read_json(Path(row["path"]).parent/"completed.json").get("ok") is True
                       for rows in by_slot for row in rows)
            bounded_output["divider/edge"]=compare_rate_bounds(bounds,order["order"],
                (bounded_noise or {}).get("divider/edge",{}).get("noise")) if closed else {"status":"unmeasured","error":"Missing clean slot exit"}
        if any(len(rows)!=3 or not all(row["valid"] for row in rows) for rows in by_slot):
            errors.append(f"{workload}: requires three valid contacts in every slot")
            continue
        probes = by_slot[0][0]["probes"]
        for probe in probes:
            key = f"{workload}/{probe}"
            metrics = [pooled(rows,probe) for rows in by_slot]
            entry = {"slots":metrics,"all":pooled([r for rows in by_slot for r in rows],probe),"metrics":{}}
            if expected_slots==4:
                control = pooled(by_slot[0]+by_slot[3],probe)
                candidate = pooled(by_slot[1]+by_slot[2],probe)
                entry["metrics"]=compare_metrics(metrics,control,candidate,order["order"],key,noise)
            output[key]=entry
    if "pen" in order["workloads"]:
        pen_slots={int(Path(row["path"]).relative_to(root).parts[0].split("-",1)[0]):row["contacts"] for row in pen or []}
        if any(len(pen_slots.get(i,[]))!=3 for i in range(expected_slots)):
            errors.append("pen: requires three measured contacts in every slot")
        else:
            def median_metrics(rows):
                values={k:[r[k] for r in rows] for k in ("renderer_frames_per_s","ui_ms_per_input","ui_ms_per_renderer_change","private_bytes")}
                for column in ("interval_ms","response_ms"):
                    for q in ("p50_ms","p95_ms","p99_ms"):
                        values[f"{column}_{q}"]=[r[column][q] for r in rows if r[column]]
                return {k:quantiles(v)["p50"] for k,v in values.items()}
            metrics=[median_metrics(pen_slots[i]) for i in range(expected_slots)]
            entry={"basis":"Existing pen harness; median per-contact distributions; raw UI CPU without idle subtraction","slots":metrics,"all":median_metrics([c for row in pen_slots.values() for c in row]),"metrics":{}}
            if expected_slots==4:
                entry["metrics"]=compare_metrics(metrics,median_metrics(pen_slots[0]+pen_slots[3]),median_metrics(pen_slots[1]+pen_slots[2]),order["order"],"pen/renderer",noise)
            output["pen/renderer"]=entry
    return {"order":order,"workloads":output,"bounded_rates":bounded_output,"errors":errors}


def summarize_pen(path):
    script=Path(__file__).with_name("windows-pen-report.mjs")
    subprocess.run(["node",str(script),str(path)],check=True,capture_output=True,text=True)
    original=read_json(path/"summary.json")
    capture=read_json(path/"capture.json")
    monitor=read_json(path.parent/"pen-cpu/meta.json")
    samples=read_json(path.parent/"pen-cpu/cpu.json")
    if original["process_id"]!=monitor["process_id"] or capture["qpc_frequency"]!=monitor["qpc_frequency"] or monitor.get("failure"):
        raise ValueError("Pen CPU monitor identity/frequency mismatch")
    result=[]
    contacts=original["brush_motion"]["contacts"]
    for contact in contacts:
        if not contact["measured"]:
            continue
        with (path/contact["file"]).open(encoding="utf-8-sig",newline="") as stream:
            inputs=list(csv.DictReader(stream))
        start,end=int(inputs[0]["qpc_before"]),int(inputs[-1]["qpc_after"])
        milliseconds=interpolate(samples,end,"ui_cpu_ms")-interpolate(samples,start,"ui_cpu_ms")
        result.append({"file":contact["file"],"basis":"Existing pen harness renderer submissions; external raw UI CPU, without idle subtraction",
                       "renderer_frames_per_s":contact["renderer_frames_per_second"],
                       "interval_ms":contact["frame_intervals_ms"],"response_ms":contact["input_to_frame_return_ms"],
                       "ui_cpu_ms":milliseconds,"ui_ms_per_input":milliseconds/len(inputs),
                       "ui_ms_per_renderer_change":milliseconds/contact["distinct_submitted_frames"],
                       "private_bytes":max(row["private_bytes"] for row in samples if start<=row["qpc"]<=end)})
    if len(result)!=3:
        raise ValueError("Expected three measured pen contacts")
    return {"path":str(path),"contacts":result,"true_presented_fps":None,"valid":True}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("run", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--noise", type=Path)
    args = parser.parse_args()
    rows, pen, failures = [], [], []
    for meta in sorted(args.run.rglob("meta.json")):
        if not meta.parent.name.startswith("contact-"):
            continue
        try:
            rows.append(summarize_contact(meta.parent))
        except (ValueError,KeyError,OSError) as error:
            failures.append({"path":str(meta.parent), "error":str(error)})
    for capture in sorted(args.run.rglob("pen/capture.json")):
        try:
            pen.append(summarize_pen(capture.parent))
        except (ValueError,KeyError,OSError,subprocess.CalledProcessError) as error:
            failures.append({"path":str(capture.parent),"error":str(error)})
    failures.extend({"path":str(p), "error":p.read_text(encoding="utf-8-sig")} for p in args.run.rglob("failure.txt"))
    args.out.mkdir(parents=True, exist_ok=True)
    noise=read_json(args.noise) if args.noise else {}
    arms=aggregate(args.run,rows,noise.get("workloads"),pen,noise.get("bounded_rates"))
    if "pen" in arms.get("order",{}).get("workloads",[]) and len(pen)!=len(arms["order"]["slots"]):
        arms["errors"].append("Missing pen slots")
    summary = {"basis":"Desktop Duplication composed changes; GPU VM diagnostic", "contacts":rows,"pen":pen, "failures":failures,
               "valid":bool(rows or pen) and not failures and not arms["errors"] and all(r.get("valid",False) for r in rows)}
    (args.out/"contacts.json").write_text(json.dumps(summary,indent=2)+"\n")
    lines = ["| Workload/contact | Probe | Changes/s | Interval p50/p95/p99 ms | Response p95 ms | Completion p95 ms | UI ms/change | Private MiB | Valid |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    def fmt(value):
        return "unmeasured" if value is None else f"{value:.3f}"
    for row in rows:
        for name, probe in row["probes"].items():
            intervals="/".join(fmt(probe["interval_ms"][p]) for p in ("p50","p95","p99"))
            lines.append(f'| {row["workload"]}/{Path(row["path"]).name} | {name} | {fmt(probe["changes_per_s"])} | {intervals} | {fmt(probe["response_ms"]["p95"])} | {fmt(probe["completion_ms"]["p95"])} | {fmt(probe["ui_ms_per_change"])} | {fmt(row["cpu"]["private_bytes"]/2**20)} | {row.get("valid",False)} |')
    lines.extend(['', 'Interior motion cadence is additional to the unchanged raw intervals above. It uses consecutive changes in one observed direction, excluding 5% at each end of the full requested position range. This reuses the drag-latency margin fraction; latency applies it per input segment. Observed direction order and turns must match the requests, with turns at their extrema; otherwise this diagnostic is unqualified. Mid-run stalls remain included.', '',
                  '| Workload/contact | Probe | Interior p95/p99 ms | Qualified | Observed/expected turns | Off-peak turns |',
                  '| --- | --- | --- | --- | --- | --- |'])
    for row in rows:
        for name,probe in row['probes'].items():
            cadence=probe.get('interior_motion_cadence',{})
            if 'expected_turns' not in cadence:
                continue
            values=quantiles(probe['samples']['interior_interval_ms'] if cadence['qualified'] else [])
            intervals='/'.join(fmt(values[key]) for key in ('p95','p99'))
            lines.append(f'| {row["workload"]}/{Path(row["path"]).name} | {name} | {intervals} | {cadence["qualified"]} | {cadence["observed_turns"]}/{cadence["expected_turns"]} | {cadence["off_peak_turns"]} |')
    (args.out/"contacts.md").write_text("\n".join(lines)+"\n")
    (args.out/"comparison.json").write_text(json.dumps(arms,indent=2)+"\n")
    print(json.dumps({"contacts":len(rows),"failures":len(failures),"valid":summary["valid"]}))
    if not summary["valid"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
