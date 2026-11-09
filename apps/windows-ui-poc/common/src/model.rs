use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
pub struct SliderSpec { pub id: &'static str, pub label: &'static str, pub min:f64, pub max:f64, pub initial:f64, pub step:f64 }
pub const SLIDERS:[SliderSpec;4]=[
    SliderSpec{id:"slider-size",label:"Size",min:1.0,max:100.0,initial:32.0,step:0.1},
    SliderSpec{id:"slider-opacity",label:"Opacity",min:0.0,max:100.0,initial:100.0,step:0.1},
    SliderSpec{id:"slider-flow",label:"Flow",min:0.0,max:100.0,initial:100.0,step:0.1},
    SliderSpec{id:"slider-spacing",label:"Spacing",min:1.0,max:100.0,initial:10.0,step:0.1},
];

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UiState {
    pub sliders:[f64;4],
    pub number:f64,
    pub divider_fraction:f32,
    pub rows:u32,
    pub visible_rows:u32,
    pub tab:u32,
    pub group_tab:u32,
    pub menu_item:u32,
    pub revision:u64,
    pub drawer_open:bool,
    pub zen_visible:bool,
}

pub struct Model { state:UiState, text:String, labels:Vec<String>, visible:Vec<u32>, row_visible:Vec<bool> }
impl Model {
    pub fn new(rows:u32)->Result<Self,String> {
        if !(1..=65_536).contains(&rows) {return Err("rows must be between 1 and 65536".into());}
        Ok(Self {state:UiState {sliders:SLIDERS.map(|s|s.initial),number:10.0,divider_fraction:0.6,rows,visible_rows:rows,menu_item:u32::MAX,revision:1,zen_visible:true,..Default::default()},text:String::new(),labels:(0..rows).map(|row|crate::label(row as usize)).collect(),visible:(0..rows).collect(),row_visible:vec![true;rows as usize]})
    }
    pub fn state(&self)->UiState {self.state}
    pub fn text(&self)->&str {&self.text}
    fn changed(&mut self)->bool {self.state.revision=self.state.revision.wrapping_add(1);true}
    pub fn set_slider(&mut self,index:u32,value:f64)->bool {
        let Some(spec)=SLIDERS.get(index as usize) else{return false;};
        if !value.is_finite() {return false;}
        let steps=1.0/spec.step;let value=(value.clamp(spec.min,spec.max)*steps).round()/steps;
        if self.state.sliders[index as usize]==value {return false;}
        self.state.sliders[index as usize]=value;self.changed()
    }
    pub fn set_number(&mut self,value:f64)->bool {
        if !value.is_finite() {return false;}
        let value=value.clamp(0.0,10_000.0);
        if self.state.number==value {return false;}
        self.state.number=value;self.changed()
    }
    pub fn set_text(&mut self,text:&str)->bool {
        if self.text==text {return false;}
        self.text=text.into();
        self.visible=self.labels.iter().enumerate().filter_map(|(row,label)|label.contains(text).then_some(row as u32)).collect();
        self.state.visible_rows=self.visible.len() as u32;self.changed()
    }
    pub fn set_divider(&mut self,top:f32,height:f32)->bool {
        if !top.is_finite() || !height.is_finite() || height<=8.0 {return false;}
        let min=((height-8.0)*0.5).min(80.0);
        let value=top.clamp(min,height-8.0-min)/height;
        if self.state.divider_fraction==value {return false;}
        self.state.divider_fraction=value;self.changed()
    }
    pub fn activate(&mut self,action:u32,index:u32)->bool {
        match action {
            0 if index<2 && index!=self.state.tab=>self.state.tab=index,
            1 if index<2 && index!=self.state.group_tab=>self.state.group_tab=index,
            2=>self.state.drawer_open = !self.state.drawer_open,
            3=>self.state.zen_visible = !self.state.zen_visible,
            4 if index<self.state.rows=>self.row_visible[index as usize] = !self.row_visible[index as usize],
            5 if index<8 && index!=self.state.menu_item=>self.state.menu_item=index,
            _=>return false,
        }
        self.changed()
    }
    pub fn visible_row(&self,index:u32)->Option<u32> {self.visible.get(index as usize).copied()}
    pub fn row_visible(&self,index:u32)->bool {self.row_visible.get(index as usize).copied().unwrap_or(false)}
    pub fn slider_text(&self,index:u32)->String {
        self.state.sliders.get(index as usize).map(|value|format!("{value:.1}").trim_end_matches(".0").to_owned()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtering_preserves_authored_row_identity_and_visibility() {
        let mut model=Model::new(1000).unwrap();
        assert!(model.activate(4,10));assert!(!model.row_visible(10));
        assert!(model.set_text("图层"));
        assert_eq!(model.visible_row(0),Some(10));assert_eq!(model.visible_row(1),Some(80));
        assert!(model.set_text("Layer 501"));assert_eq!(model.visible_row(0),Some(501));assert_eq!(model.state().visible_rows,1);
        model.set_text("");assert_eq!(model.state().visible_rows,1000);assert!(!model.row_visible(10));
    }
    #[test]
    fn invalid_values_preserve_state_and_redundant_updates_remain_idle() {
        let mut model=Model::new(1000).unwrap();let initial=model.state().revision;
        assert!(!model.set_slider(0,f64::NAN));assert!(!model.set_number(f64::INFINITY));assert!(!model.activate(4,1000));assert!(!model.activate(0,0));
        assert_eq!(model.state().revision,initial);
        assert!(model.set_slider(0,1000.0));assert_eq!(model.state().sliders[0],100.0);
        assert!(!model.set_slider(0,1000.0));assert_eq!(model.slider_text(0),"100");
        assert!(model.set_slider(0,42.34));assert_eq!(model.state().sliders[0],42.3);
        assert!(Model::new(0).is_err());assert!(Model::new(65_537).is_err());
    }
    #[test]
    fn divider_keeps_both_groups_nonnegative_through_short_window_resizes() {
        let mut model=Model::new(1).unwrap();
        for height in [20.0,160.0,928.0] {
            for target in [-1000.0,5000.0] {
                model.set_divider(target,height);let top=model.state().divider_fraction*height;
                assert!(top>=0.0 && height-8.0-top>=0.0);
            }
        }
    }
}
