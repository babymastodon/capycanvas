#[derive(Clone, Copy, Debug)]
pub struct DragSample {pub value:f64,pub segment:u32}
pub fn centred(seconds:f64,base:f64,amplitude:f64)->DragSample {
    DragSample {value:base+amplitude*(std::f64::consts::PI*seconds).sin(),segment:(seconds+0.5).floor() as u32}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centred_contacts_start_and_finish_at_the_same_position(){
        for seconds in [0.,1.,6.]{assert!((centred(seconds,2000.,250.).value-2000.).abs()<1e-8);}
        assert_eq!(centred(0.5,2050.,250.).value,2300.);assert_eq!(centred(1.5,2050.,250.).value,1800.);
        assert_eq!(centred(0.49,0.,1.).segment,0);assert_eq!(centred(0.51,0.,1.).segment,1);
    }
}
