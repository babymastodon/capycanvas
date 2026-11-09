use serde::Serialize;
use serde_json::Value;

pub fn process_exit(code:u32)->Result<(),String> {
    if code!=0 {return Err(format!("application exited with code 0x{code:08x}"));}
    Ok(())
}
pub fn canvas_trace(trace:&[Value])->Result<(),String> {
    if let Some(record)=trace.iter().find(|record|record.get("kind").and_then(Value::as_str)==Some("error")) {
        return Err(format!("canvas trace reports a failure: {}",record.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(||record.to_string())));
    }
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct StartupEvidence {pub first_ui_qpc:i64,pub first_canvas_qpc:i64,pub canvas_present_lower_bound_qpc:i64}
pub fn startup(frames:&[Value],trace:&[Value],launch:i64)->Result<StartupEvidence,String> {
    let first_ui=frames.iter().filter(|f|f.get("ui_marker").and_then(Value::as_u64).is_some_and(|v|v!=0)).filter_map(|f|f.get("last_present_qpc").and_then(Value::as_i64)).find(|qpc|*qpc>=launch).ok_or("no displayed nonzero UI marker after launch")?;
    let lower_bound=trace.iter().find(|r|r.get("milestone").and_then(Value::as_str)==Some("first_canvas_present")).and_then(|r|r.get("qpc")).and_then(Value::as_i64).ok_or("canvas trace has no first_canvas_present milestone")?;
    let first_canvas=frames.iter().filter(|f|f.get("pen_samples").and_then(Value::as_u64).is_some()).filter_map(|f|f.get("last_present_qpc").and_then(Value::as_i64)).find(|qpc|*qpc>=lower_bound&&*qpc>=launch).ok_or("no decoded canvas marker displayed after first canvas present")?;
    Ok(StartupEvidence{first_ui_qpc:first_ui,first_canvas_qpc:first_canvas,canvas_present_lower_bound_qpc:lower_bound})
}
#[derive(Debug, Serialize)]
pub struct PenTolerances {pub position_px:f64,pub pressure:f64,pub tilt_degrees:f64,pub twist_degrees:f64}
#[derive(Debug, Serialize)]
pub struct PenEvidence {
    pub validated:bool,
    pub injected:u32,
    pub received:u32,
    pub consumed:u32,
    pub matching_basis:&'static str,
    pub timestamp_identity:&'static str,
    pub rewritten_receipt_timestamps:Option<u32>,
    pub rewritten_consumed_timestamps:Option<u32>,
    pub timestamp_comparable_samples:u32,
    pub timestamp_noncomparable_samples:u32,
    pub host_timestamp_bases:Vec<String>,
    pub native_latest_identity:&'static str,
    pub native_latest_checked_samples:u32,
    pub native_latest_rewritten_samples:Option<u32>,
    pub tolerances:PenTolerances,
}
fn ordered<'a>(records:&'a [Value],kind:&str,expected:u32)->Result<Vec<&'a Value>,String> {
    let rows:Vec<_>=records.iter().filter(|r|r.get("kind").and_then(Value::as_str)==Some(kind)).collect();
    if rows.len()!=expected as usize{return Err(format!("expected {expected} {kind} records, found {}",rows.len()));}
    for (i,row) in rows.iter().enumerate(){
        if row.get("index").and_then(Value::as_u64)!=Some(i as u64){return Err(format!("missing, duplicate or reordered {kind} index {i}"));}
    }
    Ok(rows)
}
fn number(row:&Value,key:&str,index:usize)->Result<f64,String> {
    row.get(key).and_then(Value::as_f64).filter(|v|v.is_finite()).ok_or_else(||format!("pen sample {index} has no finite {key}"))
}
fn timestamp(row:&Value,index:usize)->Result<u64,String> {
    row.get("timestamp").and_then(Value::as_u64).ok_or_else(||format!("pen sample {index} has no timestamp"))
}
fn sample_tuple(input:&Value,sample:&Value,prefix:&str,index:usize,tolerances:&PenTolerances)->Result<(),String> {
    for (key,tolerance) in [("x",tolerances.position_px),("y",tolerances.position_px),("pressure",tolerances.pressure),("tilt_x",tolerances.tilt_degrees),("tilt_y",tolerances.tilt_degrees),("twist",tolerances.twist_degrees)] {
        let expected=number(input,key,index)?;let actual=number(sample,&format!("{prefix}{key}"),index)?;
        if (expected-actual).abs()>tolerance{return Err(format!("ordered pen sample {index} differs in {key}: injected {expected}, {prefix}sample {actual}, quantization tolerance {tolerance}"));}
    }
    Ok(())
}
struct HostTimestampEvidence {bases:Vec<TimestampBasis>,names:Vec<String>,latest_checked:u32,latest_rewritten:u32}
#[derive(Clone, Copy)]
enum TimestampBasis {NativeQpc,WinrtMicroseconds,Undeclared}
fn host_timestamp_basis(trace:&[Value],receipts:&[&Value],injections:&[&Value],tolerances:&PenTolerances,expected:u32)->Result<HostTimestampEvidence,String> {
    let messages:Vec<_>=trace.iter().filter(|row|row["kind"]=="host_pen_message").collect();
    if messages.is_empty(){return Ok(HostTimestampEvidence{bases:vec![TimestampBasis::Undeclared;expected as usize],names:Vec::new(),latest_checked:0,latest_rewritten:0});}
    let mut samples=Vec::new();let mut names=std::collections::BTreeSet::new();let mut latest_checked=0;let mut latest_rewritten=0;
    for message in messages {
        let count=message["samples"].as_f64().filter(|n|n.is_finite()&&n.fract()==0.0&&*n>0.0&&*n<=expected as f64).ok_or("host pen message has no valid sample count")? as usize;
        if samples.len()+count>receipts.len(){return Err("host pen messages exceed the accepted delivery count".into());}
        let name=message["timestamp_basis"].as_str().unwrap_or("undeclared");names.insert(name.to_owned());
        let basis=match name {
            "native_PerformanceCount_matched_ordered_history"=>{
                if message["native_history_matched"]!=true||message["native_history_count"].as_f64()!=Some(count as f64){return Err("native PerformanceCount basis lacks matched ordered history".into());}
                TimestampBasis::NativeQpc
            },
            "PointerPoint.Timestamp_microseconds"=>TimestampBasis::WinrtMicroseconds,
            _=>TimestampBasis::Undeclared,
        };
        let receipt=message["receipt_qpc"].as_f64().filter(|v|v.is_finite()&&v.fract()==0.0).ok_or("host pen message has no receipt QPC")?;
        for record in &receipts[samples.len()..samples.len()+count]{if record["qpc"].as_f64()!=Some(receipt){return Err("host pen message sample count or receipt QPC does not match ordered delivery".into());}}
        if !matches!(basis,TimestampBasis::NativeQpc)&&message["native_latest_tuple_matched"]==true {
            let index=samples.len()+count-1;
            sample_tuple(injections[index],message,"native_latest_",index,tolerances)?;
            let native_stamp=message["native_performance_count"].as_f64().filter(|n|n.is_finite()&&n.fract()==0.0&&*n>0.0).ok_or("matched native latest sample has no PerformanceCount")?;
            latest_checked+=1;latest_rewritten+=u32::from(native_stamp!=timestamp(injections[index],index)? as f64);
        }
        samples.extend(std::iter::repeat_n(basis,count));
    }
    if samples.len()!=expected as usize{return Err("host timestamp basis does not cover the accepted delivery count".into());}
    Ok(HostTimestampEvidence{bases:samples,names:names.into_iter().collect(),latest_checked,latest_rewritten})
}
pub fn pen_delivery(inputs:&[Value],trace:&[Value],expected:u32)->Result<PenEvidence,String> {
    if expected<2{return Err("pen workload has no complete accepted contact".into());}
    let injections=ordered(inputs,"pen",expected)?;
    let receipts=ordered(trace,"input_receipt",expected)?;
    let consumed=ordered(trace,"pen_consumed",expected)?;
    let tolerances=PenTolerances {position_px:0.5,pressure:0.5/1024.0+1e-7,tilt_degrees:0.5+1e-7,twist_degrees:0.5+1e-7};
    let host=host_timestamp_basis(trace,&receipts,&injections,&tolerances,expected)?;
    let timestamp_bases=host.bases;
    let mut rewritten_receipt_timestamps=0;let mut rewritten_consumed_timestamps=0;let mut timestamp_comparable_samples=0;
    for (i,((input,receipt),consumed)) in injections.iter().zip(&receipts).zip(&consumed).enumerate(){
        let injected_stamp=timestamp(input,i)?;
        let receipt_stamp=timestamp(receipt,i)?;
        let sample=consumed.get("sample").ok_or_else(||format!("pen consumption {i} has no sample tuple"))?;
        let consumed_stamp=timestamp(sample,i)?;
        if receipt_stamp!=consumed_stamp{return Err(format!("pen receipt and consumption timestamps disagree at index {i}"));}
        if matches!(timestamp_bases[i],TimestampBasis::NativeQpc){
            timestamp_comparable_samples+=1;
            rewritten_receipt_timestamps+=u32::from(receipt_stamp!=injected_stamp);
            rewritten_consumed_timestamps+=u32::from(consumed_stamp!=injected_stamp);
        }
        if consumed.get("pen_samples").and_then(Value::as_u64)!=Some(i as u64+1){return Err(format!("invalid consumed pen count at index {i}"));}
        sample_tuple(input,sample,"",i,&tolerances)?;
    }
    let timestamp_identity=if timestamp_comparable_samples==expected {if rewritten_receipt_timestamps==0&&rewritten_consumed_timestamps==0{"unchanged"}else{"rewritten"}}else if timestamp_comparable_samples>0{"partially_comparable"}else if timestamp_bases.iter().all(|basis|matches!(basis,TimestampBasis::WinrtMicroseconds)){"not_comparable_different_units"}else{"unverified_source_basis"};
    let native_latest_identity=if host.latest_checked>0{if host.latest_rewritten>0{"rewritten"}else{"unchanged"}}else if timestamp_comparable_samples==expected{"covered_by_ordered_history"}else{"unverified"};
    Ok(PenEvidence {validated:true,injected:expected,received:expected,consumed:expected,matching_basis:"ordered_indices_and_quantized_sample_tuples",timestamp_identity,rewritten_receipt_timestamps:(timestamp_comparable_samples>0).then_some(rewritten_receipt_timestamps),rewritten_consumed_timestamps:(timestamp_comparable_samples>0).then_some(rewritten_consumed_timestamps),timestamp_comparable_samples,timestamp_noncomparable_samples:expected-timestamp_comparable_samples,host_timestamp_bases:host.names,native_latest_identity,native_latest_checked_samples:host.latest_checked,native_latest_rewritten_samples:(host.latest_checked>0).then_some(host.latest_rewritten),tolerances})
}

#[cfg(test)]
mod tests {
    use super::*;use serde_json::json;
    #[test]
    fn complete_rendering_does_not_hide_a_failed_application_exit() {
        assert!(process_exit(0).is_ok());
        for code in [1,7,259,0xc0000005] {assert!(process_exit(code).is_err());}
    }
    #[test]
    fn explicit_canvas_failure_rejects_an_otherwise_complete_trace() {
        let mut trace=vec![json!({"kind":"startup","milestone":"first_canvas_present","qpc":120}),json!({"kind":"canvas_frame","pen_samples":100})];
        assert!(canvas_trace(&trace).is_ok());
        trace.push(json!({"kind":"error","qpc":130,"error":"GPU wait failed during shutdown"}));
        assert!(canvas_trace(&trace).unwrap_err().contains("GPU wait failed during shutdown"));
        trace[2]=json!({"kind":"error"});assert!(canvas_trace(&trace).is_err());
    }
    #[test]
    fn empty_paint_workload_is_not_successful_delivery() { assert!(pen_delivery(&[],&[],0).is_err()); }
    #[test]
    fn startup_rejects_blank_decode_and_uses_display_time() {
        let frames=vec![json!({"ui_marker":0,"pen_samples":0,"last_present_qpc":100}),json!({"ui_marker":1,"pen_samples":0,"last_present_qpc":125})];
        let trace=vec![json!({"milestone":"first_canvas_present","qpc":120})];
        let startup=startup(&frames,&trace,90).unwrap();
        assert_eq!(startup.first_ui_qpc,125);assert_eq!(startup.first_canvas_qpc,125);
        assert!(super::startup(&frames[..1],&trace,90).is_err());
    }
    fn samples(rewritten:bool)->(Vec<Value>,Vec<Value>) {
        let mut inputs=Vec::new();let mut trace=Vec::new();
        for i in 0..3 {
            let pressure=0.2+i as f64*0.1;let tilt_x=0.4+i as f64*1.2;let tilt_y=29.9-i as f64*1.4;let twist=15.4+i as f64*3.0;
            let injected=100+i*10;let stamp=if rewritten{injected+5}else{injected};
            inputs.push(json!({"kind":"pen","index":i,"qpc":injected,"timestamp":injected,"x":100+i*4,"y":200+i*3,"pressure":pressure,"tilt_x":tilt_x,"tilt_y":tilt_y,"twist":twist}));
            trace.push(json!({"kind":"input_receipt","index":i,"qpc":injected+8,"timestamp":stamp}));
            trace.push(json!({"kind":"pen_consumed","index":i,"pen_samples":i+1,"sample":{"timestamp":stamp,"x":100+i*4,"y":200+i*3,"pressure":(pressure*1024.0).round()/1024.0,"tilt_x":tilt_x.round(),"tilt_y":tilt_y.round(),"twist":twist.round()}}));
        }
        for i in 0..3 {trace.push(json!({"kind":"host_pen_message","samples":1,"receipt_qpc":108+i*10,"timestamp_basis":"native_PerformanceCount_matched_ordered_history","native_history_count":1,"native_history_matched":true}));}
        (inputs,trace)
    }
    #[test]
    fn timestamp_rewriting_preserves_ordered_quantized_sample_identity() {
        for rewritten in [false,true]{
            let (inputs,trace)=samples(rewritten);let evidence=pen_delivery(&inputs,&trace,3).unwrap();
            assert!(evidence.validated);assert_eq!(evidence.timestamp_identity,if rewritten{"rewritten"}else{"unchanged"});
            assert_eq!(evidence.rewritten_receipt_timestamps,Some(if rewritten{3}else{0}));assert_eq!(evidence.rewritten_consumed_timestamps,Some(if rewritten{3}else{0}));
            assert_eq!(evidence.matching_basis,"ordered_indices_and_quantized_sample_tuples");
            assert_eq!(evidence.native_latest_identity,"covered_by_ordered_history");assert_eq!(evidence.native_latest_checked_samples,0);assert_eq!(evidence.native_latest_rewritten_samples,None);
        }
    }
    #[test]
    fn balanced_missing_and_duplicate_samples_fail_despite_matching_counts_and_indices() {
        let (inputs,mut trace)=samples(true);let stamp=trace[3]["sample"]["timestamp"].clone();trace[3]["sample"]=trace[1]["sample"].clone();trace[3]["sample"]["timestamp"]=stamp;
        assert!(pen_delivery(&inputs,&trace,3).unwrap_err().contains("differs in x"));
    }
    #[test]
    fn reordered_tuples_and_missing_or_duplicate_trace_indices_fail() {
        let (inputs,trace)=samples(true);
        let mut reordered=trace.clone();let first=reordered[1]["sample"].clone();let second=reordered[3]["sample"].clone();reordered[1]["sample"]=second;reordered[3]["sample"]=first;
        reordered[1]["sample"]["timestamp"]=trace[1]["sample"]["timestamp"].clone();reordered[3]["sample"]["timestamp"]=trace[3]["sample"]["timestamp"].clone();
        assert!(pen_delivery(&inputs,&reordered,3).is_err());
        assert!(pen_delivery(&inputs,&trace[..5],3).is_err());
        let mut duplicate=trace.clone();duplicate[4]["index"]=json!(1);assert!(pen_delivery(&inputs,&duplicate,3).is_err());
    }
    #[test]
    fn axes_outside_os_quantization_and_disagreeing_receipt_timestamps_fail() {
        let (inputs,mut trace)=samples(true);trace[1]["sample"]["pressure"]=json!(0.21);assert!(pen_delivery(&inputs,&trace,3).unwrap_err().contains("pressure"));
        let (inputs,mut trace)=samples(true);trace[1]["sample"]["tilt_x"]=json!(1.0);assert!(pen_delivery(&inputs,&trace,3).unwrap_err().contains("tilt_x"));
        let (inputs,mut trace)=samples(true);trace[1]["sample"]["timestamp"]=json!(999);assert!(pen_delivery(&inputs,&trace,3).unwrap_err().contains("timestamps disagree"));
    }
    #[test]
    fn skipped_nominal_slots_preserve_contiguous_accepted_sample_identity() {
        let (mut inputs,trace)=samples(true);
        for (row,slot) in inputs.iter_mut().zip([0,3,5]){row["source_index"]=json!(slot);}
        let evidence=pen_delivery(&inputs,&trace,3).unwrap();assert_eq!(evidence.injected,3);assert_eq!(evidence.received,3);assert_eq!(evidence.consumed,3);
    }
    #[test]
    fn equal_os_timestamps_do_not_hide_the_observed_history_order_swap() {
        let inputs=vec![
            json!({"kind":"pen","index":0,"timestamp":4734057386u64,"x":1792,"y":850,"pressure":0.2145920190589637,"tilt_x":13.268660706570039,"tilt_y":26.90618224598065,"twist":7}),
            json!({"kind":"pen","index":1,"timestamp":4734059261u64,"x":1790,"y":852,"pressure":0.2150089338892198,"tilt_x":13.619714992186402,"tilt_y":26.730195725651036,"twist":8}),
        ];
        let mut trace=vec![
            json!({"kind":"input_receipt","index":0,"timestamp":4734059640u64}),
            json!({"kind":"pen_consumed","index":0,"pen_samples":1,"sample":{"timestamp":4734059640u64,"x":1790,"y":852,"pressure":0.21484375,"tilt_x":14,"tilt_y":27,"twist":8}}),
            json!({"kind":"input_receipt","index":1,"timestamp":4734059640u64}),
            json!({"kind":"pen_consumed","index":1,"pen_samples":2,"sample":{"timestamp":4734059640u64,"x":1792,"y":850,"pressure":0.21484375,"tilt_x":13,"tilt_y":27,"twist":7}}),
        ];
        assert!(pen_delivery(&inputs,&trace,2).unwrap_err().contains("differs in x"));
        let first=trace[1]["sample"].clone();trace[1]["sample"]=trace[3]["sample"].clone();trace[3]["sample"]=first;
        assert!(pen_delivery(&inputs,&trace,2).unwrap().validated);
    }

    #[test]
    fn winrt_microseconds_are_not_classified_as_rewritten_qpc() {
        let (inputs,mut trace)=samples(true);
        for row in trace.iter_mut().filter(|row|row["kind"]=="host_pen_message"){row["timestamp_basis"]=json!("PointerPoint.Timestamp_microseconds");row["native_history_matched"]=json!(false);}
        let evidence=pen_delivery(&inputs,&trace,3).unwrap();
        assert!(evidence.validated);assert_eq!(evidence.timestamp_identity,"not_comparable_different_units");assert_eq!(evidence.timestamp_comparable_samples,0);assert_eq!(evidence.timestamp_noncomparable_samples,3);assert_eq!(evidence.rewritten_receipt_timestamps,None);assert_eq!(evidence.rewritten_consumed_timestamps,None);
    }
    fn microsecond_batch(rewritten:bool)->(Vec<Value>,Vec<Value>) {
        let (inputs,mut trace)=samples(true);trace.truncate(6);trace[2]["qpc"]=json!(108);
        let mut message=json!({"kind":"host_pen_message","samples":2.0,"receipt_qpc":108.0,"timestamp_basis":"PointerPoint.Timestamp_microseconds","native_history_matched":false,"native_latest_tuple_matched":true,"native_performance_count":if rewritten{115.0}else{110.0}});
        for key in ["x","y","pressure","tilt_x","tilt_y","twist"]{message[format!("native_latest_{key}")]=trace[3]["sample"][key].clone();}
        trace.push(message);trace.push(json!({"kind":"host_pen_message","samples":1,"receipt_qpc":128,"timestamp_basis":"PointerPoint.Timestamp_microseconds","native_latest_tuple_matched":false}));
        (inputs,trace)
    }
    #[test]
    fn microsecond_history_independently_checks_only_batch_latest_native_qpc() {
        for rewritten in [false,true]{
            let (inputs,trace)=microsecond_batch(rewritten);let evidence=pen_delivery(&inputs,&trace,3).unwrap();
            assert_eq!(evidence.timestamp_identity,"not_comparable_different_units");assert_eq!(evidence.timestamp_comparable_samples,0);assert_eq!(evidence.rewritten_receipt_timestamps,None);
            assert_eq!(evidence.native_latest_identity,if rewritten{"rewritten"}else{"unchanged"});assert_eq!(evidence.native_latest_checked_samples,1);assert_eq!(evidence.native_latest_rewritten_samples,Some(u32::from(rewritten)));
        }
    }
    #[test]
    fn unmatched_or_missing_latest_native_proof_remains_unverified() {
        for matched in [json!(false),Value::Null]{
            let (inputs,mut trace)=microsecond_batch(true);trace[6]["native_latest_tuple_matched"]=matched;
            let evidence=pen_delivery(&inputs,&trace,3).unwrap();assert_eq!(evidence.native_latest_identity,"unverified");assert_eq!(evidence.native_latest_checked_samples,0);assert_eq!(evidence.native_latest_rewritten_samples,None);
        }
    }
    #[test]
    fn latest_native_proof_requires_the_last_accepted_tuple_in_its_batch() {
        let (inputs,mut trace)=microsecond_batch(true);trace[6]["native_latest_x"]=inputs[0]["x"].clone();assert!(pen_delivery(&inputs,&trace,3).unwrap_err().contains("sample 1 differs in x"));
        let (inputs,mut trace)=microsecond_batch(true);trace[6]["native_performance_count"]=Value::Null;assert!(pen_delivery(&inputs,&trace,3).unwrap_err().contains("no PerformanceCount"));
    }
    #[test]
    fn mixed_or_missing_timestamp_units_preserve_order_gates_without_identity_claims() {
        let (inputs,mut trace)=samples(true);trace[6]["timestamp_basis"]=json!("PointerPoint.Timestamp_microseconds");
        let evidence=pen_delivery(&inputs,&trace,3).unwrap();assert_eq!(evidence.timestamp_identity,"partially_comparable");assert_eq!(evidence.timestamp_comparable_samples,2);assert_eq!(evidence.rewritten_receipt_timestamps,Some(2));
        trace.retain(|row|row["kind"]!="host_pen_message");let evidence=pen_delivery(&inputs,&trace,3).unwrap();assert_eq!(evidence.timestamp_identity,"unverified_source_basis");assert_eq!(evidence.rewritten_receipt_timestamps,None);
    }
    #[test]
    fn timestamp_basis_must_cover_the_same_ordered_receipt_batches() {
        let (inputs,mut trace)=samples(true);trace[6]["samples"]=json!(1.0);trace[6]["native_history_count"]=json!(1.0);assert!(pen_delivery(&inputs,&trace,3).is_ok());
        let (inputs,mut trace)=samples(true);trace[6]["receipt_qpc"]=json!(999);assert!(pen_delivery(&inputs,&trace,3).is_err());
        let (inputs,mut trace)=samples(true);trace[6]["native_history_matched"]=json!(false);assert!(pen_delivery(&inputs,&trace,3).is_err());
    }

}
