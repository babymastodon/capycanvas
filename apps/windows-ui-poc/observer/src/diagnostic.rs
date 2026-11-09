pub const MAX_SECONDS:f64=60.0;
pub const DEFAULT_SAMPLES:u32=2400;
pub fn sample_target(command:&str,requested:Option<u32>)->Result<Option<u32>,String> {
    match command {
        "run"=>if requested.is_some(){Err("--samples is only valid for pen-check".into())}else{Ok(None)},
        "pen-check"=>{
            let samples=requested.unwrap_or(DEFAULT_SAMPLES);
            let ceiling=(MAX_SECONDS*crate::clock::nominal_hz("paint-still","pen")) as u32;
            if !(2..=ceiling).contains(&samples){return Err(format!("pen-check --samples must be between 2 and {ceiling}"));}
            Ok(Some(samples))
        },
        _=>Err("expected run or pen-check".into()),
    }
}
pub fn data_complete(target:u32,accepted_data:usize)->bool {accepted_data>=target.saturating_sub(1) as usize}
pub fn axis_progress(target:u32,accepted_index:usize)->f64 {(accepted_index as f64/target.saturating_sub(2).max(1) as f64).clamp(0.0,1.0)}
pub fn terminal_pen_pending(observed:u32,expected:u32,now:i64,deadline:i64)->Result<bool,String> {
    if observed>expected {return Err(format!("pen display marker exceeded accepted count: expected {expected}, observed {observed}"));}
    if observed==expected {return Ok(false);}
    if now>=deadline {return Err(format!("terminal pen display deadline elapsed: expected {expected}, observed {observed}"));}
    Ok(true)
}
pub fn history(trace:&[serde_json::Value])->serde_json::Value {
    let samples:Vec<_>=trace.iter().filter(|row|row["kind"]=="pen_history").filter_map(|row|row["samples"].as_u64()).collect();
    let messages_with_history=samples.iter().filter(|n|**n>1).count();
    serde_json::json!({"messages":samples.len(),"max_samples":samples.iter().copied().max().unwrap_or(0),"messages_with_history":messages_with_history,"used":messages_with_history>0})
}
#[cfg(test)]
mod tests {
    use super::*;use crate::clock::{Clock,Tick};
    #[test]
    fn functional_count_options_do_not_change_motion_benchmarks() {
        assert_eq!(sample_target("run",None),Ok(None));assert!(sample_target("run",Some(2400)).is_err());
        assert_eq!(sample_target("pen-check",None),Ok(Some(2400)));
        for n in [0,1,14401]{assert!(sample_target("pen-check",Some(n)).is_err());}
        for n in [2,2400,14400]{assert_eq!(sample_target("pen-check",Some(n)),Ok(Some(n)));}
    }
    #[test]
    fn overdue_slots_do_not_reduce_the_accepted_count_target() {
        let mut clock=Clock::new((0..14400).map(|i|i as f64/240.0).collect(),60.0,1.0/240.0);
        let mut accepted=0;let mut skipped=0;
        for t in [0.0,0.020,0.040]{
            assert!(!data_complete(4,accepted));
            let Tick::Due {skipped:missed,..}=clock.take_due(t)else{panic!("missing fresh sample");};
            skipped+=missed.len();accepted+=1;
        }
        assert!(skipped>0);assert!(data_complete(4,accepted));assert_eq!(accepted+1,4);
    }
    #[test]
    fn count_diagnostic_axis_ramp_ends_at_the_last_accepted_data_sample() {
        assert_eq!(axis_progress(2400,0),0.0);assert_eq!(axis_progress(2400,2398),1.0);
        assert_eq!(axis_progress(4,0),0.0);assert_eq!(axis_progress(4,1),0.5);assert_eq!(axis_progress(4,2),1.0);
    }
    #[test]
    fn observed_history_does_not_invent_coalescing() {
        use serde_json::json;
        assert_eq!(history(&[])["used"],false);
        assert_eq!(history(&[json!({"kind":"pen_history","samples":1})])["used"],false);
        let evidence=history(&[json!({"kind":"pen_history","samples":1}),json!({"kind":"pen_history","samples":3}),json!({"kind":"canvas_frame","samples":500})]);
        assert_eq!(evidence["messages"],2);assert_eq!(evidence["max_samples"],3);assert_eq!(evidence["messages_with_history"],1);assert_eq!(evidence["used"],true);
    }
    #[test]
    fn terminal_capture_keeps_observing_the_actual_warp_count_after_350_ms() {
        assert!(terminal_pen_pending(1445,1529,350,10_000).unwrap());
        assert!(terminal_pen_pending(1445,1529,600,10_000).unwrap());
        assert!(!terminal_pen_pending(1529,1529,700,10_000).unwrap());
    }
    #[test]
    fn terminal_capture_fails_on_a_missing_or_impossible_final_marker() {
        assert!(terminal_pen_pending(1445,1529,10_000,10_000).unwrap_err().contains("deadline"));
        assert!(terminal_pen_pending(1530,1529,700,10_000).unwrap_err().contains("exceeded"));
    }

}
