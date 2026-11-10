use serde::{Deserialize,Serialize};

#[derive(Clone)]
pub struct Pixels {pub width:usize,pub height:usize,pub rgba:Vec<[u8;4]>}
#[derive(Clone,Deserialize,Serialize)]
pub struct Spec {pub name:String,pub rect:[i32;4],pub kind:String,#[serde(default)]pub axis:usize,#[serde(default)]pub anchor:usize,#[serde(default)]pub endpoints:Vec<String>}
#[derive(Default,Serialize)]
pub struct Observation {pub position:Option<i32>,pub error:Option<f64>,pub changed_pixels:usize,pub hash:String,pub black:usize,pub valid:bool}
pub struct Probe {spec:Spec,template:Option<Vec<[u8;3]>>,previous:Option<Pixels>,offset:i32,shift_band:Option<[usize;2]>}
fn distance(a:&[u8;3],b:&[u8;3])->u32 {(0..3).map(|i|a[i].abs_diff(b[i]) as u32).sum()}
fn strip(p:&Pixels,axis:usize)->Vec<[u8;3]> {
    let (along,across)=if axis==0 {(p.width,p.height)}else{(p.height,p.width)};
    band(p,axis,[0,across],along)
}
fn band(p:&Pixels,axis:usize,cross:[usize;2],along:usize)->Vec<[u8;3]> {
    (0..along).map(|at|{let mut sum=[0u32;3];for cross in cross[0]..cross[1]{let i=if axis==0{cross*p.width+at}else{at*p.width+cross};for (c,s) in sum.iter_mut().enumerate(){*s+=p.rgba[i][c] as u32;}}sum.map(|s|(s/(cross[1]-cross[0]).max(1) as u32) as u8)}).collect()
}
fn match_template(line:&[[u8;3]],template:&[[u8;3]])->Option<(i32,f64)> {
    if template.len()<8||line.len()<template.len(){return None;}
    if line.iter().all(|p|*p==[0,0,0]){return None;}
    let contrast=template.windows(2).map(|p|distance(&p[0],&p[1])).sum::<u32>();
    if contrast<60{return None;}
    let scores:Vec<_>=line.windows(template.len()).enumerate().map(|(i,part)|{
        (i as i32,part.iter().zip(template).map(|(a,b)|distance(a,b) as u64).sum::<u64>() as f64/(3*template.len()) as f64)
    }).collect();
    let best=scores.iter().min_by(|a,b|a.1.total_cmp(&b.1)).copied()?;
    if best.1>24.||scores.iter().any(|(at,error)|(*at-best.0).abs()>3&&*error<=best.1+0.5){return None;}Some(best)
}
impl Probe {
    pub fn new(spec:Spec)->Self {Self {spec,template:None,previous:None,offset:0,shift_band:None}}
    pub fn observe(&mut self,p:Pixels)->Observation {
        let mut out=Observation {valid:true,..Default::default()};let mut hash=14695981039346656037u64;
        for (i,pixel) in p.rgba.iter().enumerate(){for channel in &pixel[..3]{hash=(hash^*channel as u64).wrapping_mul(1099511628211);}
            out.black+=usize::from(pixel[..3]==[0,0,0]);
            if let Some(previous)=&self.previous && i<previous.rgba.len()&&pixel[..3].iter().zip(&previous.rgba[i][..3]).any(|(a,b)|a.abs_diff(*b)>12){out.changed_pixels+=1;}
        }
        out.hash=format!("{hash:016x}");
        if self.spec.kind=="edge" {
            let line=strip(&p,self.spec.axis);
            if self.template.is_none(){self.template=line.get(self.spec.anchor..self.spec.anchor+32).map(<[_]>::to_vec);}
            if let Some((position,error))=self.template.as_ref().and_then(|t|match_template(&line,t)){out.position=Some(position);out.error=Some(error);}else{out.valid=false;}
        }else if self.spec.kind=="shift" {
            let (along,across)=if self.spec.axis==0{(p.width,p.height)}else{(p.height,p.width)};
            if self.previous.is_none(){
                let width=across.min(3);
                self.shift_band=(0..=across-width).filter_map(|cross|{
                    let range=[cross,cross+width];let line=band(&p,self.spec.axis,range,along);let pad=64.min(along/4);
                    match_template(&line,&line[pad..along-pad]).map(|_|(range,line.windows(2).map(|v|distance(&v[0],&v[1])).sum::<u32>()))
                }).max_by_key(|(_,contrast)|*contrast).map(|(range,_)|range);
            }
            if let (Some(previous),Some(range))=(&self.previous,self.shift_band) {
                let line=band(&p,self.spec.axis,range,along);let before=band(previous,self.spec.axis,range,along);
                let pad=64.min(before.len()/4);let middle=&before[pad..before.len()-pad];
                if let Some((at,error))=match_template(&line,middle){self.offset+=at-pad as i32;out.error=Some(error);out.position=Some(self.offset);}else{out.valid=false;}
            }else if self.shift_band.is_some(){out.position=Some(0);}else{out.valid=false;}
        }else if self.spec.kind!="change"&&self.spec.kind!="black-grid" {out.valid=false;}
        if out.valid{self.previous=Some(p);}out
    }
}

#[cfg(test)] mod tests {
    use super::*;
    fn pattern(n:usize)->Pixels {Pixels {width:1,height:n,rgba:(0..n).map(|i|[(i*71%251) as u8,(i*43%253) as u8,(i*97%255) as u8,255]).collect()}}
    fn spec(kind:&str)->Spec {Spec {name:kind.into(),rect:[0,0,1,200],kind:kind.into(),axis:1,anchor:60,endpoints:vec![]}}
    #[test] fn edge_tracks_rigid_pixels_and_rejects_black(){
        let base=pattern(200);let mut probe=Probe::new(spec("edge"));assert_eq!(probe.observe(base.clone()).position,Some(60));
        let mut moved=base.clone();moved.rgba.rotate_right(72);assert_eq!(probe.observe(moved).position,Some(132));
        let black=Pixels {width:1,height:200,rgba:vec![[0,0,0,255];200]};assert!(!probe.observe(black).valid);
    }
    #[test] fn shift_accumulates_both_directions(){
        let base=pattern(200);let mut probe=Probe::new(spec("shift"));assert_eq!(probe.observe(base.clone()).position,Some(0));
        let mut moved=base.clone();moved.rgba.rotate_left(8);assert_eq!(probe.observe(moved).position,Some(-8));assert_eq!(probe.observe(base).position,Some(0));
    }
    #[test] fn shift_keeps_displacement_across_a_rejected_frame(){
        let base=pattern(200);let mut probe=Probe::new(spec("shift"));assert_eq!(probe.observe(base.clone()).position,Some(0));
        let mut moved=base.clone();moved.rgba.rotate_left(8);assert_eq!(probe.observe(moved).position,Some(-8));
        let black=Pixels{width:1,height:200,rgba:vec![[0,0,0,255];200]};assert!(!probe.observe(black).valid);
        let mut moved=base;moved.rgba.rotate_left(16);assert_eq!(probe.observe(moved).position,Some(-16));
    }
    #[test] fn shift_chooses_unique_text_band_over_repeated_icons(){
        let mut base=Pixels{width:9,height:200,rgba:Vec::new()};
        for y in 0..200{for x in 0..9{let v=if x<6{((y%20)*11) as u8}else{(y*71%251) as u8};base.rgba.push([v,v,v,255]);}}
        let mut probe=Probe::new(spec("shift"));assert_eq!(probe.observe(base.clone()).position,Some(0));
        base.rgba.rotate_left(9*8);assert_eq!(probe.observe(base).position,Some(-8));
        let repeated=Pixels{width:3,height:200,rgba:(0..600).map(|i|{let v=((i/3%20)*11) as u8;[v,v,v,255]}).collect()};
        assert!(!Probe::new(spec("shift")).observe(repeated).valid);
    }
    #[test] fn changes_ignore_small_pixel_noise_and_alpha(){
        let base=pattern(200);let mut probe=Probe::new(spec("change"));let first=probe.observe(base.clone());let again=probe.observe(base.clone());assert_eq!(first.hash,again.hash);assert_eq!(again.changed_pixels,0);
        let mut changed=base;changed.rgba[4]=[0,0,0,0];assert_eq!(probe.observe(changed).changed_pixels,1);
    }
    #[test] fn dark_and_repeated_strips_are_not_confident_edges(){
        let template:Vec<_>=(0..32).map(|i|if i%2==0{[0,0,0]}else{[4,4,4]}).collect();
        assert!(match_template(&vec![[0,0,0];100],&template).is_none());
        let repeated:Vec<_>=(0..128).map(|i|template[i%32]).collect();assert!(match_template(&repeated,&template).is_none());
    }
}
