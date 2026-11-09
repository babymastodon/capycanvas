use serde::{Deserialize, Serialize};
pub mod model;
pub mod glass_motion;
#[cfg(windows)] pub mod ready;

pub const TITLE: &str = "Capy POC";
pub const CLIENT: [u32; 2] = [1600, 1000];
pub const ROW_HEIGHT: f32 = 44.0;
pub const COLUMN_LEFT: f32 = 320.0;
pub const THUMBNAIL_CAPACITY:u32=128;
pub const THUMBNAIL_HZ:u32=30;
pub const MARKER_BITS: usize = 24;
pub const MARKER_CELL: u32 = 4;
pub const MARKER_WIDTH: u32 = 96;
pub const MARKER_GUARD: [u8;4] = [17,238,68,255];
pub const PEN_MASK: u32 = 0x00ff_ffff;
pub const LABELS: [&str; 7] = ["日本語のレイヤー", "图层 简体", "圖層 繁體", "레이어", "ชั้นภาพวาดสีน้ำมัน", "Lớp vẽ tiếng Việt", "Слой для рисования"];
pub const PARAGRAPH: &str = "ชั้นภาพวาดสีน้ำมันสำหรับการวาดภาพที่สวยงาม 日本語の文章を複数の行に折り返して表示します。";
pub const IDS: [&str; 21] = ["tab-a", "tab-b", "drawer-toggle", "zen-toggle", "layers", "properties", "divider", "layers-tab", "brushes-tab", "layers-list", "slider-size", "slider-opacity", "slider-flow", "slider-spacing", "number-field", "text-field", "menu-button", "wrap-text", "drawer", "title-bar", "drawing-canvas"];

pub fn label(index: usize) -> String {
    if index.is_multiple_of(10) { LABELS[(index / 10) % LABELS.len()].into() } else { format!("Layer {index}") }
}
pub fn row_id(index: usize) -> String { format!("layer-row-{index:05}") }
pub fn visibility_id(index: usize) -> String { format!("layer-visible-{index:05}") }
pub fn ui_marker(state: u16, frame: u8) -> u32 { ((state as u32) << 8) | frame as u32 }
pub fn marker_bit(value: u32, bit: usize) -> bool { value & (1 << bit) != 0 }
pub fn decode_marker(pixels: &[[u8; 4]]) -> Result<u32, &'static str> {
    if pixels.len() != MARKER_BITS { return Err("marker requires 24 cells"); }
    let mut value = 0;
    for (bit, pixel) in pixels.iter().enumerate() {
        let dark = pixel[..3].iter().all(|c| *c < 32);
        let light = pixel[..3].iter().all(|c| *c > 223);
        if !dark && !light { return Err("marker cell is neither black nor white"); }
        if light { value |= 1 << bit; }
    }
    Ok(value)
}
pub fn ruler_rgb(index: u16) -> [u8; 3] { [index as u8, (index >> 8) as u8, 0x55] }
pub fn decode_ruler(rgb: [u8; 3]) -> Option<u16> {
    (rgb[2] == 0x55).then_some(rgb[0] as u16 + 256 * rgb[1] as u16)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PenSample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub twist: f32,
    pub timestamp: u64,
    pub flags: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GlassRect { pub x: f32, pub y: f32, pub width: f32, pub height: f32, pub revision: u64 }
pub fn pen_history_oldest_first(samples:&mut [PenSample]) { samples.reverse(); }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Load { #[default] Light, Slow }
impl Load {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text { "light" => Ok(Self::Light), "slow" => Ok(Self::Slow), _ => Err(format!("invalid canvas load: {text}")) }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct AppOptions {
    pub trace: std::path::PathBuf,
    pub load: Load,
    pub thumbnails: bool,
    pub rows: usize,
    pub ready_event:Option<String>,
    pub ui_stall_ms:u32,
    pub ui_stall_every_ms:u32,
}
impl AppOptions {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut result = Self { trace: "canvas.jsonl".into(), load: Load::Light, thumbnails: false, rows: 1000,ready_event:None,ui_stall_ms:0,ui_stall_every_ms:100 };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut next = || args.next().ok_or_else(|| format!("missing value for {arg}"));
            match arg.as_str() {
                "--trace" => result.trace = next()?.into(),
                "--canvas-load" => result.load = Load::parse(next()?)?,
                "--thumbnails" => result.thumbnails = match next()?.as_str() { "on" => true, "off" => false, _ => return Err("invalid thumbnails mode".into()) },
                "--rows" => result.rows = next()?.parse().map_err(|_| "invalid rows")?,
                "--ready-event"=>result.ready_event=Some(next()?.clone()),
                "--ui-stall-ms"=>result.ui_stall_ms=next()?.parse().map_err(|_|"invalid UI stall duration")?,
                "--ui-stall-every-ms"=>result.ui_stall_every_ms=next()?.parse().map_err(|_|"invalid UI stall interval")?,
                _ => return Err(format!("unknown application argument: {arg}")),
            }
        }
        if !(1..=65_536).contains(&result.rows) {return Err("rows must be between 1 and 65536".into());}
        if result.ready_event.as_ref().is_some_and(|name|name.is_empty()||name.contains('\0')) {return Err("invalid input-ready event name".into());}
        if result.ui_stall_every_ms==0 || result.ui_stall_ms>result.ui_stall_every_ms {return Err("UI stall duration must not exceed its positive interval".into());}
        Ok(result)
    }
}

pub fn specification() -> serde_json::Value {
    serde_json::json!({
        "title":TITLE,"client":CLIENT,"ids":IDS,"automation":IDS.iter().map(|id|(id.replace('-',"_"),*id)).collect::<std::collections::BTreeMap<_,_>>(),"labels":LABELS,"paragraph":PARAGRAPH,
        "strings":{"properties":"Properties","divider":"Divider","title_bar":"Title bar","canvas":"Canvas","layers":"Layers","brushes":"Brushes","size":"Size","opacity":"Opacity","flow":"Flow","spacing":"Spacing","menu":"Menu","number":"Number","text":"Filter layers","percent":"100%","tab_a":"A","tab_b":"B","drawer":"Drawer","zen":"Zen"},
        "metrics":{"title_height":40,"column_left":320,"column_right":16,"column_top":56,"column_bottom":16,"divider":8,"top_fraction":0.6,"radius":12,"header_height":36,"row_height":44,"thumbnail_size":32,"thumbnail_capacity":THUMBNAIL_CAPACITY,"thumbnail_hz":THUMBNAIL_HZ,"drawer_width":280,"drawer_ms":200,"zen_ms":180,"wrap_width":260},
        "probes":{"divider":[255,0,255],"glass":[0,255,255],"fill":[255,128,0],"drawer":[255,255,0],"zen":[255,255,255],"slider":[0,255,0],"width":[0,0,255],"ruler_blue":85},
        "marker":{"bits":MARKER_BITS,"cell":MARKER_CELL,"width":MARKER_WIDTH,"ui_origin":[4,44],"canvas_origin_x":4,"canvas_bottom":12,"guard":MARKER_GUARD},
        "probe_metrics":{"bar":2,"fill":1,"zen_patch":[200,16,6,6],"zen_canvas_black":[195,11,16,16],"ruler_x":2,"drawer_scan_y":300,"title_width_y":20},
        "sliders":model::SLIDERS,"number":{"min":0,"max":10000,"initial":10},
        "marker_guard":MARKER_GUARD,"glass_tint":[32,36,42,184],"menu_items":(0..8).map(|i|format!("Item {}",i+1)).collect::<Vec<_>>(),"menu_ids":(0..8).map(|i|format!("menu-item-{i}")).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newest_first_pen_history_preserves_order_when_os_timestamps_tie() {
        let mut samples=[PenSample{x:1790.0,timestamp:100,..Default::default()},PenSample{x:1792.0,timestamp:100,..Default::default()},PenSample{x:1794.0,timestamp:99,..Default::default()}];
        pen_history_oldest_first(&mut samples);
        assert_eq!(samples.map(|s|s.x),[1794.0,1792.0,1790.0]);
    }
    #[test]
    fn marker_codec_preserves_every_bit_and_wrap() {
        for value in [0, 1, 0xffffff, ui_marker(65535, 255), ui_marker(65535, 0)] {
            let pixels: Vec<_> = (0..24).map(|i| if marker_bit(value, i) { [255; 4] } else { [0, 0, 0, 255] }).collect();
            assert_eq!(decode_marker(&pixels), Ok(value));
        }
        assert!(decode_marker(&[[127; 4]; 24]).is_err());
        assert!(decode_marker(&[]).is_err());
    }
    #[test]
    fn ids_and_multilingual_labels_are_stable() {
        assert_eq!(row_id(500), "layer-row-00500");
        assert_eq!(label(10), LABELS[1]);
        assert_eq!(label(11), "Layer 11");
    }
    #[test]
    fn removed_load_and_invalid_flags_are_rejected() {
        assert!(Load::parse("heavy").is_err());
        assert!(AppOptions::parse(&["--thumbnails".into(), "sometimes".into()]).is_err());
    }
    #[test]
    fn ruler_round_trips_every_row_without_continuity() {
        for row in 0..=u16::MAX { assert_eq!(decode_ruler(ruler_rgb(row)), Some(row)); }
        assert_eq!(decode_ruler([0, 0, 0x54]), None);
        assert_eq!(decode_ruler([255, 255, 0x56]), None);
    }
}
