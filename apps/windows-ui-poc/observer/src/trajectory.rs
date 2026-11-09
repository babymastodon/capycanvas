#[derive(Clone, Copy, Debug)]
pub struct DragSample {pub value:f64,pub segment:u32,pub resting:bool}
pub fn drag(seconds:f64,warmup:f64,base:f64,amplitude:f64)->DragSample {
    let measured=(seconds-warmup).max(0.0);
    DragSample {value:base-amplitude*(std::f64::consts::PI*measured).cos(),segment:measured.floor() as u32,resting:seconds>=0.25*warmup&&seconds<warmup}
}
pub fn is_drag(workload:&str)->bool {matches!(workload,"divider"|"slowcanvas-divider"|"scrub-while-painting"|"resize-programmatic"|"resize-native")}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warmup_holds_rest_then_measured_clock_owns_the_half_periods() {
        for t in [0.0,0.1,0.25,0.9,1.0]{assert_eq!(drag(t,1.0,500.0,60.0).value,440.0);}
        assert!(!drag(0.1,1.0,500.0,60.0).resting);
        assert!(drag(0.25,1.0,500.0,60.0).resting);
        assert!(!drag(1.0,1.0,500.0,60.0).resting);
        let maximum=drag(2.0,1.0,500.0,60.0);assert_eq!(maximum.value,560.0);assert_eq!(maximum.segment,1);
        let minimum=drag(3.0,1.0,500.0,60.0);assert_eq!(minimum.value,440.0);assert_eq!(minimum.segment,2);
    }
    #[test]
    fn short_warmup_keeps_its_own_resting_interval() {
        assert!(!drag(0.024,0.1,1.0,1.0).resting);
        assert!(drag(0.025,0.1,1.0,1.0).resting);
        assert!(drag(0.099,0.1,1.0,1.0).resting);
        assert!(!drag(0.1,0.1,1.0,1.0).resting);
    }
}
