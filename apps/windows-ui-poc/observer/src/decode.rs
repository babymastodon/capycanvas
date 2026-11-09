use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Pixels { pub width: usize, pub height: usize, pub rgba: Vec<[u8; 4]> }
impl Pixels {
    pub fn at(&self, x: usize, y: usize) -> Option<[u8; 4]> { if x < self.width && y < self.height { self.rgba.get(y * self.width + x).copied() } else { None } }
    pub fn marker(&self) -> Result<u32, &'static str> {
        if self.width!=98||self.height!=6{return Err("marker crop requires a 98 by 6 pixel guarded payload");}
        for y in 0..6 {for x in 0..98 {
            if (x==0||x==97||y==0||y==5)&&self.at(x,y)!=Some(poc_common::MARKER_GUARD){return Err("marker framing border is missing or incomplete");}
        }}
        let pixels: Vec<_> = (0..24).map(|i| self.at(1+i*4+2,3).ok_or("marker payload is incomplete")).collect::<Result<_, _>>()?;
        poc_common::decode_marker(&pixels)
    }
    pub fn color_run(&self, color: [u8; 3], vertical: bool) -> Option<std::ops::Range<i32>> {
        let length = if vertical { self.height } else { self.width };
        let mut first = None;
        for i in 0..length {
            let pixel = if vertical { self.at(0, i) } else { self.at(i, 0) }?;
            if pixel[..3] == color {first.get_or_insert(i as i32);} else if let Some(first)=first {return Some(first..i as i32);}
        }
        first.map(|first|first..length as i32)
    }
    pub fn ruler(&self, row_height: f64, list_top: i32) -> Option<f64> {
        if row_height <= 0.0 { return None; }
        for y in 1..self.height {
            let pixel = self.at(0, y)?;
            let prior = self.at(0, y - 1)?;
            if pixel[2] != 0x55 || pixel == prior { continue; }
            let row = pixel[0] as u32 + 256 * pixel[1] as u32;
            let required = (row_height * 0.75).max(2.0) as usize;
            if y + required > self.height { continue; }
            if (y..y + required).all(|i| self.at(0, i) == Some(pixel)) {
                return Some(row as f64 * row_height - (list_top + y as i32) as f64 + list_top as f64);
            }
        }
        None
    }
}

#[derive(Default, Debug, Serialize)]
pub struct CaptureStats {
    pub capture_start_qpc: i64,
    pub capture_end_qpc: i64,
    pub desktop_frames: u64,
    pub cursor_only_frames: u64,
    pub coalesced_frames: u64,
    pub missing_desktop_frames: u64,
    pub marker_errors: u64,
    pub valid_ui_markers: u64,
    pub valid_canvas_markers: u64,
    pub max_pen_samples: u32,
    pub capture_failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn marker(value:u32)->Pixels {
        let mut image=Pixels {width:98,height:6,rgba:vec![poc_common::MARKER_GUARD;98*6]};
        for y in 1..5 {for x in 1..97 {image.rgba[y*98+x]=if value&(1<<((x-1)/4))!=0{[255;4]}else{[0,0,0,255]};}}
        image
    }
    #[test]
    fn guarded_marker_preserves_physical_payload_cells_and_all_values() {
        for value in [0,1,0xaaaa55,0xffffff]{assert_eq!(marker(value).marker(),Ok(value));}
        let mut image=marker(0xaaaa55);image.rgba[3*98+3]=[128;4];assert!(image.marker().is_err());
        assert_ne!(poc_common::MARKER_GUARD[2],0x55);
    }
    #[test]
    fn blank_white_and_black_surfaces_are_not_marker_producers() {
        for color in [[255;4],[0,0,0,255]] {
            let image=Pixels {width:98,height:6,rgba:vec![color;98*6]};assert!(image.marker().is_err());
        }
    }
    #[test]
    fn every_framing_pixel_must_match_exactly() {
        let image=marker(0xaaaa55);
        for y in 0..6 {for x in 0..98 {
            if x==0||x==97||y==0||y==5 {let mut incomplete=image.clone();incomplete.rgba[y*98+x][0]^=1;assert!(incomplete.marker().is_err(),"accepted damaged guard at {x},{y}");}
        }}
        let mut short=image;short.width=97;assert!(short.marker().is_err());
    }
    #[test]
    fn absent_edge_is_not_zero_and_bottom_edges_align() {
        let p = Pixels { width: 1, height: 6, rgba: vec![[0;4],[0;4],[255,0,255,255],[255,0,255,255],[0;4],[0;4]] };
        assert_eq!(p.color_run([255,0,255],true), Some(2..4));
        assert_eq!(p.color_run([0,255,255],true), None);
        let glass=Pixels {width:1,height:6,rgba:vec![[0;4],[0;4],[0;4],[0;4],[0,255,255,255],[0,255,255,255]]};
        assert_eq!(p.color_run([255,0,255],true).unwrap().end,glass.color_run([0,255,255],true).unwrap().start);
        let drawer=Pixels {width:3,height:1,rgba:vec![[255,255,0,255],[255,255,0,255],[0;4]]};
        assert_eq!(drawer.color_run([255,255,0],false),Some(0..2));
    }
    #[test]
    fn ruler_decodes_absolute_rows_beyond_256() {
        let mut p = Pixels {width:1,height:100,rgba:vec![[0;4];100]};
        for y in 10..76 { p.rgba[y] = [1,2,0x55,255]; }
        assert_eq!(p.ruler(66.0,138),Some(513.0*66.0-10.0));
        for pixel in &mut p.rgba { if pixel[2]==0x55 { pixel[2]=0x54; } }
        assert_eq!(p.ruler(66.0,138),None);
    }
}
