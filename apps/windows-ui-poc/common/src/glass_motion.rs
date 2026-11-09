use crate::GlassRect;
use serde::Serialize;

pub const GLASS_OPACITY: u32 = u32::MAX;
pub const GLASS_LINEAR: u32 = 0;
pub const GLASS_CUBIC_OUT: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize)]
pub struct GlassAnimation {
    pub from_rect: GlassRect,
    pub to_rect: GlassRect,
    pub from_opacity: f32,
    pub to_opacity: f32,
    pub rect_index: u32,
    pub start_qpc: i64,
    pub duration_ms: u32,
    pub easing: u32,
}
impl GlassAnimation {
    pub fn validate(&self) -> Result<(), String> {
        if self.start_qpc <= 0 || !(1..=10_000).contains(&self.duration_ms) || self.easing > GLASS_CUBIC_OUT { return Err("invalid glass animation timing".into()); }
        if ![self.from_opacity,self.to_opacity].into_iter().all(|value|value.is_finite() && (0.0..=1.0).contains(&value)) { return Err("glass opacity must be between zero and one".into()); }
        if self.rect_index != GLASS_OPACITY && (self.rect_index >= 64 || !valid_rect(self.from_rect) || !valid_rect(self.to_rect)) { return Err("invalid animated glass rectangle".into()); }
        Ok(())
    }
    fn progress(&self, now: i64, frequency: i64) -> f32 {
        let elapsed = (now as i128 - self.start_qpc as i128).max(0) as f64;
        let linear = (elapsed * 1000.0 / frequency as f64 / self.duration_ms as f64).clamp(0.0,1.0) as f32;
        if self.easing == GLASS_CUBIC_OUT { 1.0 - (1.0-linear).powi(3) } else { linear }
    }
    fn rectangle(&self, now: i64, frequency: i64) -> GlassRect {
        let t = self.progress(now,frequency);
        let blend = |a,b|a+(b-a)*t;
        GlassRect {x:blend(self.from_rect.x,self.to_rect.x),y:blend(self.from_rect.y,self.to_rect.y),width:blend(self.from_rect.width,self.to_rect.width),height:blend(self.from_rect.height,self.to_rect.height),revision:self.to_rect.revision}
    }
    fn opacity(&self, now:i64, frequency:i64)->f32 {self.from_opacity+(self.to_opacity-self.from_opacity)*self.progress(now,frequency)}
}
pub fn valid_rect(rect:GlassRect)->bool {[rect.x,rect.y,rect.width,rect.height].into_iter().all(f32::is_finite) && rect.width>=0.0 && rect.height>=0.0}
fn same(a:GlassRect,b:GlassRect)->bool {a.x==b.x && a.y==b.y && a.width==b.width && a.height==b.height}
fn matches_target(rect:GlassRect,animation:&GlassAnimation)->bool {
    let from=animation.from_rect;let to=animation.to_rect;
    [(rect.x,from.x,to.x),(rect.y,from.y,to.y),(rect.width,from.width,to.width),(rect.height,from.height,to.height)].into_iter().all(|(value,start,end)|start==end || (value-end).abs()<=0.001)
}
fn update_layout(animation:&mut GlassAnimation,rect:GlassRect) {
    if animation.from_rect.x==animation.to_rect.x {animation.from_rect.x=rect.x;animation.to_rect.x=rect.x;}
    if animation.from_rect.y==animation.to_rect.y {animation.from_rect.y=rect.y;animation.to_rect.y=rect.y;}
    if animation.from_rect.width==animation.to_rect.width {animation.from_rect.width=rect.width;animation.to_rect.width=rect.width;}
    if animation.from_rect.height==animation.to_rect.height {animation.from_rect.height=rect.height;animation.to_rect.height=rect.height;}
}
struct Drawer { animation:GlassAnimation, completed:bool }
pub struct GlassMotion {rects:Vec<GlassRect>,drawer:Option<Drawer>,zen:Option<GlassAnimation>,opacity:f32}
impl Default for GlassMotion {fn default()->Self {Self {rects:Vec::new(),drawer:None,zen:None,opacity:1.0}}}
impl GlassMotion {
    pub fn rects(&self)->&[GlassRect] {&self.rects}
    pub fn opacity(&self)->f32 {self.opacity}
    pub fn needs_frame(&self)->bool {self.drawer.as_ref().is_some_and(|drawer|!drawer.completed) || self.zen.is_some()}
    pub fn publish(&mut self,rects:Vec<GlassRect>)->Result<bool,String> {
        if rects.len()>64 || !rects.iter().copied().all(valid_rect) {return Err("invalid published glass rectangles".into());}
        if let Some(drawer)=&mut self.drawer && let Some(rect)=rects.get(drawer.animation.rect_index as usize) {update_layout(&mut drawer.animation,*rect);}
        if let Some(drawer)=&self.drawer && drawer.completed && rects.get(drawer.animation.rect_index as usize).is_some_and(|rect|matches_target(*rect,&drawer.animation)) {self.drawer=None;}
        let mut visible=rects.clone();
        if let Some(drawer)=&self.drawer {
            let index=drawer.animation.rect_index as usize;
            visible.resize(visible.len().max(index+1),GlassRect::default());
            let mut current=self.rects.get(index).copied().unwrap_or(drawer.animation.from_rect);
            let animation=drawer.animation;
            if animation.from_rect.x==animation.to_rect.x {current.x=animation.to_rect.x;}
            if animation.from_rect.y==animation.to_rect.y {current.y=animation.to_rect.y;}
            if animation.from_rect.width==animation.to_rect.width {current.width=animation.to_rect.width;}
            if animation.from_rect.height==animation.to_rect.height {current.height=animation.to_rect.height;}
            visible[index]=current;
        }
        let changed=visible.len()!=self.rects.len() || visible.iter().zip(&self.rects).any(|(a,b)|!same(*a,*b));
        self.rects=visible;Ok(changed)
    }
    pub fn animate(&mut self,mut animation:GlassAnimation,frequency:i64)->Result<GlassAnimation,String> {
        animation.validate()?;
        if frequency<=0 {return Err("invalid animation clock frequency".into());}
        if animation.rect_index==GLASS_OPACITY {
            if let Some(previous)=&self.zen {animation.from_opacity=previous.opacity(animation.start_qpc,frequency);}
            self.zen=Some(animation);
        } else {
            if let Some(previous)=&self.drawer {
                if previous.animation.rect_index!=animation.rect_index {return Err("only the drawer rectangle animates".into());}
                animation.from_rect=previous.animation.rectangle(animation.start_qpc,frequency);
            }
            self.drawer=Some(Drawer {animation,completed:false});
        }
        Ok(animation)
    }
    pub fn advance(&mut self,now:i64,frequency:i64)->bool {
        let mut changed=false;
        if let Some(drawer)=&mut self.drawer {
            let index=drawer.animation.rect_index as usize;
            let next=drawer.animation.rectangle(now,frequency);
            self.rects.resize(self.rects.len().max(index+1),GlassRect::default());
            changed|=!same(self.rects[index],next);self.rects[index]=next;
            drawer.completed=drawer.animation.progress(now,frequency)>=1.0;
        }
        if let Some(zen)=&self.zen {
            let next=zen.opacity(now,frequency);changed|=self.opacity!=next;self.opacity=next;
            if zen.progress(now,frequency)>=1.0 {self.zen=None;}
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drawer()->GlassAnimation {GlassAnimation {from_rect:GlassRect{x:-280.0,y:56.0,width:280.0,height:928.0,revision:1},to_rect:GlassRect{x:0.0,y:56.0,width:280.0,height:928.0,revision:2},from_opacity:1.0,to_opacity:1.0,rect_index:0,start_qpc:1000,duration_ms:200,easing:GLASS_CUBIC_OUT}}
    #[test]
    fn drawer_uses_signed_dips_cubic_ease_out_and_one_terminal_pose() {
        let mut motion=GlassMotion::default();motion.publish(vec![drawer().from_rect]).unwrap();motion.animate(drawer(),1000).unwrap();
        motion.advance(900,1000);assert_eq!(motion.rects()[0].x,-280.0);assert!(motion.needs_frame());
        motion.advance(1100,1000);assert_eq!(motion.rects()[0].x,-35.0);
        assert!(motion.advance(1200,1000));assert_eq!(motion.rects()[0].x,0.0);assert!(!motion.needs_frame());
        assert!(!motion.advance(1400,1000));
    }
    #[test]
    fn stale_layout_does_not_reset_motion_or_its_completed_target() {
        let mut motion=GlassMotion::default();motion.publish(vec![drawer().from_rect]).unwrap();motion.animate(drawer(),1000).unwrap();motion.advance(1100,1000);
        assert!(!motion.publish(vec![drawer().from_rect]).unwrap());assert_eq!(motion.rects()[0].x,-35.0);
        motion.advance(1200,1000);assert!(!motion.publish(vec![drawer().from_rect]).unwrap());assert_eq!(motion.rects()[0].x,0.0);
        assert!(!motion.publish(vec![drawer().to_rect]).unwrap());
        assert!(motion.publish(vec![drawer().from_rect]).unwrap());assert_eq!(motion.rects()[0].x,-280.0);
    }
    #[test]
    fn replacement_samples_existing_pose_and_zen_runs_concurrently() {
        let mut motion=GlassMotion::default();motion.animate(drawer(),1000).unwrap();
        let reverse=GlassAnimation {from_rect:drawer().to_rect,to_rect:drawer().from_rect,start_qpc:1100,easing:GLASS_LINEAR,..drawer()};
        let accepted=motion.animate(reverse,1000).unwrap();assert_eq!(accepted.from_rect.x,-35.0);
        let zen=GlassAnimation {rect_index:GLASS_OPACITY,from_opacity:1.0,to_opacity:0.0,start_qpc:1100,duration_ms:180,easing:GLASS_LINEAR,..drawer()};
        motion.animate(zen,1000).unwrap();motion.advance(1190,1000);assert!((motion.opacity()-0.5).abs()<0.0001);assert!(motion.rects()[0].x< -35.0);
        let reopen=GlassAnimation {from_opacity:0.0,to_opacity:1.0,start_qpc:1190,..zen};assert!((motion.animate(reopen,1000).unwrap().from_opacity-0.5).abs()<0.0001);
        motion.advance(1400,1000);assert_eq!(motion.opacity(),1.0);assert_eq!(motion.rects()[0].x,-280.0);assert!(!motion.needs_frame());
    }
    #[test]
    fn layout_resizes_unanimated_dimensions_without_resetting_the_slide() {
        let mut motion=GlassMotion::default();motion.animate(drawer(),1000).unwrap();motion.advance(1100,1000);
        assert!(motion.publish(vec![GlassRect{height:600.0,..drawer().from_rect}]).unwrap());assert_eq!(motion.rects()[0].x,-35.0);assert_eq!(motion.rects()[0].height,600.0);
        motion.advance(1200,1000);assert_eq!(motion.rects()[0].x,0.0);assert_eq!(motion.rects()[0].height,600.0);
        assert!(motion.publish(vec![GlassRect{height:700.0,..drawer().from_rect}]).unwrap());assert_eq!(motion.rects()[0].x,0.0);assert_eq!(motion.rects()[0].height,700.0);
        motion.publish(vec![GlassRect{height:700.0,..drawer().to_rect}]).unwrap();
        assert!(motion.publish(vec![GlassRect{height:800.0,..drawer().to_rect}]).unwrap());assert_eq!(motion.rects()[0].height,800.0);
    }
    #[test]
    fn invalid_commands_preserve_the_current_tracks() {
        let mut motion=GlassMotion::default();motion.animate(drawer(),1000).unwrap();
        for invalid in [GlassAnimation {duration_ms:0,..drawer()},GlassAnimation {start_qpc:0,..drawer()},GlassAnimation {easing:2,..drawer()},GlassAnimation {from_opacity:f32::NAN,..drawer()},GlassAnimation {to_rect:GlassRect{width:-1.0,..drawer().to_rect},..drawer()}] {assert!(motion.animate(invalid,1000).is_err());}
        motion.advance(1100,1000);assert_eq!(motion.rects()[0].x,-35.0);
    }
}
