use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicBool,Ordering},
    },
};
pub fn spec() -> &'static serde_json::Value {
    static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    VALUE.get_or_init(poc_common::specification)
}
pub fn metric(name: &str) -> f32 {
    spec()["metrics"][name].as_f64().expect("shared metric") as f32
}
pub fn string(name: &str) -> &'static str {
    spec()["strings"][name].as_str().expect("shared string")
}
pub fn rgb(name: &str) -> [u8; 3] {
    let v = &spec()["retained_colors"][name];
    [
        v[0].as_u64().unwrap() as u8,
        v[1].as_u64().unwrap() as u8,
        v[2].as_u64().unwrap() as u8,
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
        }
    }
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn array(self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
    pub fn translated(self, x: f32, y: f32) -> Self {
        Self {
            x: self.x + x,
            y: self.y + y,
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Canvas,
    Title,
    Minimize,
    Maximize,
    Close,
    Tab(u32),
    GroupTab(u32),
    Drawer,
    Zen,
    List,
    Divider,
    Slider(u32),
    Number,
    Text,
    WrapText,
    Properties,
    DrawerContent,
    Menu,
    MenuItem(u32),
    Visible(u32),
}
#[derive(Default)]
pub struct CaptionPress {
    button: Option<Target>,
}
impl CaptionPress {
    pub fn press(&mut self, button: Option<Target>) -> bool {
        self.button = button;
        self.is_pressed()
    }
    pub fn is_pressed(&self) -> bool {
        self.button.is_some()
    }
    pub fn release(&mut self, button: Option<Target>) -> Option<Target> {
        self.button.take().filter(|pressed| Some(*pressed) == button)
    }
    pub fn cancel(&mut self) {
        self.button = None;
    }
}
pub struct Motion {
    pub scroll: AtomicU32,
    pub drawer: AtomicU32,
}
impl Default for Motion {
    fn default() -> Self {
        Self {
            scroll: AtomicU32::new(0.0f32.to_bits()),
            drawer: AtomicU32::new((-metric("drawer_width")).to_bits()),
        }
    }
}
#[derive(Clone, Default)]
pub struct HitMap {
    pub entries: Vec<(Rect, Target)>,
    pub caret: Rect,
    pub list: Rect,
    pub drawer: Rect,
    pub rows: Arc<[u32]>,
    pub motion: Arc<Motion>,
    pub popup:Arc<AtomicBool>,
}
impl HitMap {
    pub fn touch_target(&self, x: f32, y: f32) -> Target {
        if self.list.contains(x, y) { Target::List } else { self.at(x, y) }
    }
    pub fn at(&self, x: f32, y: f32) -> Target {
        if self.list.contains(x, y) {
            let offset = f32::from_bits(self.motion.scroll.load(Ordering::Acquire));
            let row = ((y - self.list.y + offset) / poc_common::ROW_HEIGHT)
                .floor()
                .max(0.0) as usize;
            if x >= self.list.x + self.list.w - 75.0
                && x < self.list.x + self.list.w - 39.0
                && let Some(row) = self.rows.get(row)
            {
                return Target::Visible(*row);
            }
        }
        let target = self
            .entries
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map_or(Target::Canvas, |(_, t)| *t);
        if target == Target::Canvas {
            let shift = f32::from_bits(self.motion.drawer.load(Ordering::Acquire));
            if self.drawer.translated(shift, 0.0).contains(x, y) {
                return Target::DrawerContent;
            }
        }
        target
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum LayerId {
    Title,
    Drawer,
    Layers,
    List,
    Properties,
    Popup,
    Marker,
}
pub const LAYER_COUNT: usize = 7;
#[derive(Clone, Debug)]
pub enum Prim {
    Rect {
        rect: Rect,
        color: [f32; 4],
        radius: f32,
        border: f32,
        border_color: [f32; 4],
    },
    Glyph {
        rect: Rect,
        uv: [f32; 4],
        color: [f32; 4],
    },
    Image {
        rect: Rect,
        uv: [f32; 4],
        atlas: u32,
    },
    Glass {
        rect: Rect,
        radius: f32,
    },
    Probe {
        rect: Rect,
        color: [f32; 4],
    },
}
#[derive(Clone)]
pub struct Layer {
    pub id: LayerId,
    pub rect: Rect,
    pub clip: Rect,
    pub display: Arc<Vec<Prim>>,
    pub origin: [f32; 2],
}
impl Layer {
    pub fn requires_record(&self, previous: &Self) -> bool {
        !Arc::ptr_eq(&self.display, &previous.display)
            || self.rect != previous.rect
            || self.origin != previous.origin
    }
}
pub struct RetainedTree {
    nodes: [Option<Layer>; LAYER_COUNT],
}
impl Default for RetainedTree {
    fn default() -> Self {
        Self {
            nodes: std::array::from_fn(|_| None),
        }
    }
}
impl RetainedTree {
    pub fn with_clip(&mut self, id: LayerId, clip: Rect) -> Option<Layer> {
        let node = self.nodes[id as usize].as_mut()?;
        node.clip = clip;
        Some(node.clone())
    }
    pub fn replace(&mut self, layer: Layer) -> Layer {
        let id = layer.id as usize;
        self.nodes[id] = Some(layer);
        self.nodes[id].as_ref().unwrap().clone()
    }
}
#[cfg(test)]
mod retained_tests {
    use super::*;
    #[test]
    fn divider_clip_changes_preserve_list_display_and_skip_recording() {
        let mut tree = RetainedTree::default();
        let before = tree.replace(Layer {
            id: LayerId::List,
            rect: Rect::new(1280.0, 92.0, 304.0, 1056.0),
            clip: Rect::new(1280.0, 92.0, 304.0, 520.0),
            display: Arc::new(vec![Prim::Probe {
                rect: Rect::new(1282.0, 92.0, 2.0, 44.0),
                color: [0.0, 0.0, 85.0 / 255.0, 1.0],
            }]),
            origin: [0.0, 0.0],
        });
        let after = tree
            .with_clip(LayerId::List, Rect::new(1280.0, 92.0, 304.0, 580.0))
            .unwrap();
        assert!(Arc::ptr_eq(&before.display, &after.display));
        assert_eq!(before.rect, after.rect);
        assert_ne!(before.clip, after.clip);
        assert!(!after.requires_record(&before));
    }
    #[test]
    fn changing_one_layer_preserves_other_retained_display_lists() {
        let mut tree = RetainedTree::default();
        let layer = |id| Layer {
            id,
            rect: Rect::new(0.0, 0.0, 40.0, 40.0),
            clip: Rect::new(0.0, 0.0, 40.0, 40.0),
            display: Arc::new(Vec::new()),
            origin: [0.0, 0.0],
        };
        let first = tree.replace(layer(LayerId::Title));
        tree.replace(layer(LayerId::Properties));
        tree.replace(layer(LayerId::Properties));
        assert!(Arc::ptr_eq(
            &first.display,
            &tree.nodes[LayerId::Title as usize]
                .as_ref()
                .unwrap()
                .display
        ));
    }
}
#[derive(Clone, Copy)]
pub struct Geometry {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub column: Rect,
    pub layers: Rect,
    pub list: Rect,
    pub divider: Rect,
    pub properties: Rect,
}
impl Geometry {
    pub fn new(width: u32, height: u32, scale: f32, fraction: f32) -> Self {
        let w = width as f32 / scale;
        let h = height as f32 / scale;
        let left = metric("column_left");
        let right = metric("column_right");
        let y = metric("column_top");
        let bottom = metric("column_bottom");
        let divider_size = metric("divider");
        let header = metric("header_height");
        let column = Rect::new(w - left, y, left - right, h - y - bottom);
        let top = (column.h * fraction).clamp(0.0, (column.h - divider_size).max(0.0));
        let layers = Rect::new(column.x, column.y, column.w, top);
        let divider = Rect::new(column.x, column.y + top, column.w, divider_size);
        let properties = Rect::new(
            column.x,
            divider.y + divider_size,
            column.w,
            column.h - top - divider_size,
        );
        Self {
            width: w,
            height: h,
            scale,
            column,
            layers,
            list: Rect::new(
                layers.x,
                layers.y + header,
                layers.w,
                (layers.h - header).max(0.0),
            ),
            divider,
            properties,
        }
    }
    pub fn width_probe(self) -> Rect {
        let pixel = 1.0 / self.scale;
        Rect::new(self.width - 4.0 * pixel, 0.0, 2.0 * pixel, metric("title_height"))
    }
    pub fn physical(self, r: Rect) -> Rect {
        Rect::new(
            r.x * self.scale,
            r.y * self.scale,
            r.w * self.scale,
            r.h * self.scale,
        )
    }
    pub fn base_hits(self) -> Vec<(Rect, Target)> {
        let mut hits = vec![
            (
                Rect::new(0.0, 0.0, self.width, metric("title_height")),
                Target::Title,
            ),
            (self.layers, Target::GroupTab(0)),
            (self.properties, Target::Properties),
            (self.list, Target::List),
            (self.divider, Target::Divider),
        ];
        for (x, w, t) in [
            (256.0, 42.0, Target::Tab(0)),
            (306.0, 42.0, Target::Tab(1)),
            (356.0, 78.0, Target::Drawer),
            (442.0, 64.0, Target::Zen),
            (self.width - 138.0, 46.0, Target::Minimize),
            (self.width - 92.0, 46.0, Target::Maximize),
            (self.width - 46.0, 46.0, Target::Close),
        ] {
            hits.push((Rect::new(x, 0.0, w, metric("title_height")), t));
        }
        hits
    }
}
pub fn scroll_into_view(offset: f32, viewport: f32, top: f32, height: f32) -> f32 {
    if top < offset {
        top.max(0.0)
    } else if top + height > offset + viewport {
        (top + height - viewport).max(0.0)
    } else {
        offset
    }
}
pub fn recorded_rows(offset: f32, viewport: f32, count: u32) -> Range<u32> {
    let first = ((offset - viewport).max(0.0) / poc_common::ROW_HEIGHT).floor() as u32;
    let end = ((offset + viewport * 2.0) / poc_common::ROW_HEIGHT).ceil() as u32;
    first.min(count)..end.min(count)
}
pub fn needs_rows(range: &Range<u32>, offset: f32, viewport: f32, count: u32) -> bool {
    (range.start > 0 && offset - range.start as f32 * poc_common::ROW_HEIGHT < viewport * 0.5)
        || (range.end < count
            && range.end as f32 * poc_common::ROW_HEIGHT - offset - viewport < viewport * 0.5)
}
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: f64,
    duration: f64,
    cubic: bool,
}
impl Tween {
    pub fn new(from: f32, to: f32, start: f64, duration: f64, cubic: bool) -> Self {
        Self {
            from,
            to,
            start,
            duration,
            cubic,
        }
    }
    pub fn value(self, now: f64) -> f32 {
        let t = ((now - self.start) / self.duration).clamp(0.0, 1.0) as f32;
        let p = if self.cubic {
            1.0 - (1.0 - t).powi(3)
        } else {
            t
        };
        self.from + (self.to - self.from) * p
    }
    pub fn active(self, now: f64) -> bool {
        now < self.start + self.duration
    }
}
#[derive(Default)]
pub struct Scroll {
    pub offset: f32,
    pub velocity: f32,
    pub touch: Option<(f32, f64)>,
    pub tween: Option<Tween>,
    pub maximum: f32,
    last_clock: Option<f64>,
}
impl Scroll {
    pub fn set_maximum(&mut self, maximum: f32) {
        self.maximum = maximum.max(0.0);
        self.offset = self.offset.clamp(0.0, self.maximum);
    }
    pub fn wheel(&mut self, delta: f32, now: f64) {
        self.advance(now, 0.0);
        let target = (self.tween.map_or(self.offset, |t| t.to) + delta).clamp(0.0, self.maximum);
        self.tween = Some(Tween::new(self.offset, target, now, 0.1, true));
        self.velocity = 0.0;
    }
    pub fn down(&mut self, y: f32, now: f64) {
        self.tween = None;
        self.velocity = 0.0;
        self.touch = Some((y, now));
        self.last_clock = Some(now);
    }
    pub fn moved(&mut self, y: f32, now: f64) {
        if let Some((prior, t)) = self.touch {
            let delta = prior - y;
            self.offset = (self.offset + delta).clamp(0.0, self.maximum);
            if now > t {
                self.velocity = delta / ((now - t) as f32 * 1000.0);
            }
            self.touch = Some((y, now));
            self.last_clock = Some(now);
        }
    }
    pub fn cancel(&mut self) {
        self.touch = None;
        self.velocity = 0.0;
        self.tween = None;
    }
    pub fn up(&mut self, now: f64) {
        self.touch = None;
        self.last_clock = Some(now);
    }
    pub fn advance(&mut self, now: f64, dt: f64) -> bool {
        let before = self.offset;
        let dt = self.last_clock.map_or(dt, |prior| (now - prior).max(0.0));
        self.last_clock = Some(now);
        if let Some(tween) = self.tween {
            self.offset = tween.value(now);
            if !tween.active(now) {
                self.tween = None;
            }
        } else if self.touch.is_none() && self.velocity.abs() >= 0.05 {
            let decay = 0.95f32.powf((dt * 1000.0 / 8.33) as f32);
            let factor = 8.33 / 0.95f32.ln();
            self.offset =
                (self.offset + self.velocity * factor * (decay - 1.0)).clamp(0.0, self.maximum);
            self.velocity *= decay;
            if self.offset == 0.0 || self.offset == self.maximum {
                self.velocity = 0.0;
            }
        }
        before != self.offset
    }
    pub fn active(&self) -> bool {
        self.tween.is_some() || (self.touch.is_none() && self.velocity.abs() >= 0.05)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hit_targets_prioritize_controls_over_caption_and_canvas() {
        let g = Geometry::new(2400, 1500, 1.5, 0.6);
        let h = HitMap {
            entries: g.base_hits(),
            ..Default::default()
        };
        assert_eq!(h.at(370.0, 20.0), Target::Drawer);
        assert_eq!(h.at(900.0, 20.0), Target::Title);
        assert_eq!(h.at(700.0, 700.0), Target::Canvas);
    }
    #[test]
    fn scrolling_into_view_preserves_visible_rows_and_moves_only_to_nearest_edge() {
        let row = poc_common::ROW_HEIGHT;
        assert_eq!(scroll_into_view(0.0, 600.0, 10.0 * row, row), 0.0);
        assert_eq!(scroll_into_view(200.0, 600.0, 200.0, row), 200.0);
        assert_eq!(scroll_into_view(200.0, 600.0, 800.0 - row, row), 200.0);
        assert_eq!(scroll_into_view(200.0, 600.0, 190.0, row), 190.0);
        assert_eq!(scroll_into_view(200.0, 600.0, 790.0, row), 790.0 + row - 600.0);
    }
    #[test]
    fn overscan_is_viewport_above_and_below_and_bounded() {
        assert_eq!(recorded_rows(880.0, 440.0, 1000), 10..40);
        assert_eq!(recorded_rows(0.0, 440.0, 7), 0..7);
        assert!(!needs_rows(&(10..40), 880.0, 440.0, 1000));
        assert!(needs_rows(&(10..40), 500.0, 440.0, 1000));
    }
    #[test]
    fn compositor_scroll_advances_without_ui_and_preserves_fling_after_release() {
        let mut s = Scroll {
            maximum: 44000.0,
            ..Default::default()
        };
        s.down(500.0, 0.0);
        s.moved(100.0, 0.04);
        s.up(0.04);
        let start = s.offset;
        for i in 1..=30 {
            s.advance(0.04 + i as f64 / 120.0, 1.0 / 120.0);
        }
        assert!(s.offset > start);
        assert!(s.active());
    }
    #[test]
    fn captured_touch_times_survive_batching_and_release_integrates_only_remaining_frame() {
        let mut scroll = Scroll {
            maximum: 10000.0,
            ..Default::default()
        };
        scroll.down(500.0, 0.0);
        scroll.moved(460.0, 0.004);
        scroll.moved(420.0, 0.008);
        scroll.up(0.012);
        assert!((scroll.velocity - 10.0).abs() < 0.001);
        scroll.advance(0.016, 0.016);
        let decay = 0.95f32.powf(4.0 / 8.33);
        let expected = 80.0 + 10.0 * (8.33 / 0.95f32.ln()) * (decay - 1.0);
        assert!((scroll.offset - expected).abs() < 0.001);
        assert!(scroll.offset < 120.0);
    }
    #[test]
    fn canceled_touch_releases_ownership_without_starting_inertia_or_moving_content() {
        let mut scroll = Scroll::default();
        scroll.set_maximum(10000.0);
        scroll.down(500.0, 0.0);
        scroll.moved(100.0, 0.04);
        let position = scroll.offset;
        scroll.cancel();
        assert!(!scroll.active());
        assert!(scroll.touch.is_none());
        scroll.advance(1.0, 0.96);
        assert_eq!(scroll.offset, position);
    }
    #[test]
    fn animation_endpoints_and_interruption_start_from_actual_pose() {
        let t = Tween::new(-280.0, 0.0, 1.0, 0.2, true);
        assert_eq!(t.value(0.0), -280.0);
        assert_eq!(t.value(2.0), 0.0);
        let reversed = Tween::new(t.value(1.1), -280.0, 1.1, 0.2, true);
        assert_eq!(reversed.value(1.1), t.value(1.1));
    }
}

pub fn replace_selection(text: &str, selection: Range<usize>, insert: &str) -> (String, usize) {
    let mut start = selection.start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = selection.end.min(text.len()).max(start);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut value = text.to_owned();
    value.replace_range(start..end, insert);
    (value, start + insert.len())
}
#[cfg(test)]
mod editing_tests {
    use super::*;
    #[test]
    fn replacing_a_selection_preserves_unicode_boundaries_and_caret() {
        let (value, caret) = replace_selection("a日本語z", 1..10, "にほんご");
        assert_eq!(value, "aにほんごz");
        assert_eq!(caret, 13);
        let (value, caret) = replace_selection("日本語", 1..4, "x");
        assert_eq!(value, "x本語");
        assert_eq!(caret, 1);
    }
    #[test]
    fn touch_pan_owns_the_whole_list_including_its_visibility_buttons() {
        let map = HitMap {
            list: Rect::new(100.0, 100.0, 304.0, 440.0),
            rows: Arc::from([10]),
            ..Default::default()
        };
        assert_eq!(map.at(340.0, 110.0), Target::Visible(10));
        assert_eq!(map.touch_target(340.0, 110.0), Target::List);
        assert_eq!(map.touch_target(110.0, 110.0), Target::List);
        assert_eq!(map.touch_target(10.0, 110.0), Target::Canvas);
    }
    #[test]
    fn hit_ownership_uses_current_compositor_pose_without_rebuilding_ui() {
        let mut map = HitMap {
            list: Rect::new(100.0, 100.0, 304.0, 440.0),
            rows: Arc::from([10, 80, 150]),
            drawer: Rect::new(0.0, 56.0, 280.0, 900.0),
            ..Default::default()
        };
        map.entries.push((map.list, Target::List));
        assert_eq!(map.at(340.0, 110.0), Target::Visible(10));
        map.motion
            .scroll
            .store(44.0f32.to_bits(), Ordering::Release);
        assert_eq!(map.at(340.0, 110.0), Target::Visible(80));
        assert_eq!(map.at(20.0, 300.0), Target::Canvas);
        map.motion.drawer.store(0.0f32.to_bits(), Ordering::Release);
        assert_eq!(map.at(20.0, 300.0), Target::DrawerContent);
    }
}

pub fn line_scroll(current: f32, caret: Rect, width: f32, content_width: f32) -> f32 {
    let width = width.max(1.0);
    let mut scroll = current.max(0.0);
    if caret.x < scroll {
        scroll = caret.x;
    }
    let right = caret.x + caret.w.max(1.0);
    if right > scroll + width {
        scroll = right - width;
    }
    scroll.clamp(0.0, (content_width.max(right) - width).max(0.0))
}
pub fn clip_glyph(rect: Rect, uv: [f32; 4], clip: Rect) -> Option<(Rect, [f32; 4])> {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    let left = rect.x.max(clip.x);
    let top = rect.y.max(clip.y);
    let right = (rect.x + rect.w).min(clip.x + clip.w);
    let bottom = (rect.y + rect.h).min(clip.y + clip.h);
    if right <= left || bottom <= top {
        return None;
    }
    Some((
        Rect::new(left, top, right - left, bottom - top),
        [
            uv[0] + (left - rect.x) / rect.w * uv[2],
            uv[1] + (top - rect.y) / rect.h * uv[3],
            (right - left) / rect.w * uv[2],
            (bottom - top) / rect.h * uv[3],
        ],
    ))
}
#[cfg(test)]
mod line_tests {
    use super::*;
    #[test]
    fn horizontal_scroll_keeps_the_actual_caret_visible_and_returns_to_the_start() {
        let viewport = 100.0;
        let end = Rect::new(360.0, 0.0, 1.0, 21.0);
        let scroll = line_scroll(0.0, end, viewport, 360.0);
        assert_eq!(scroll, 261.0);
        assert_eq!(end.x - scroll, 99.0);
        assert_eq!(
            line_scroll(scroll, Rect::new(0.0, 0.0, 1.0, 21.0), viewport, 360.0),
            0.0
        );
        let middle = Rect::new(140.0, 0.0, 1.0, 21.0);
        let scroll = line_scroll(scroll, middle, viewport, 360.0);
        assert!(middle.x - scroll >= 0.0 && middle.x + middle.w - scroll <= viewport);
    }
    #[test]
    fn partial_glyph_clipping_crops_uvs_without_resampling_or_changing_the_visible_ink() {
        let uv = [0.2, 0.4, 0.1, 0.2];
        let (r, crop) = clip_glyph(
            Rect::new(-10.0, 2.0, 20.0, 12.0),
            uv,
            Rect::new(0.0, 0.0, 100.0, 20.0),
        )
        .unwrap();
        assert_eq!(r, Rect::new(0.0, 2.0, 10.0, 12.0));
        assert!((crop[0] - 0.25).abs() < 0.0001);
        assert!((crop[2] - 0.05).abs() < 0.0001);
        assert_eq!(crop[1], uv[1]);
        assert_eq!(crop[3], uv[3]);
        assert!(
            clip_glyph(
                Rect::new(110.0, 0.0, 20.0, 12.0),
                uv,
                Rect::new(0.0, 0.0, 100.0, 20.0)
            )
            .is_none()
        );
        let (r, crop) = clip_glyph(
            Rect::new(-5.0, -2.0, 20.0, 10.0),
            uv,
            Rect::new(0.0, 0.0, 10.0, 6.0),
        )
        .unwrap();
        assert_eq!(r, Rect::new(0.0, 0.0, 10.0, 6.0));
        assert!((crop[1] - 0.44).abs() < 0.0001);
        assert!((crop[3] - 0.12).abs() < 0.0001);
    }
}

#[derive(Default)]
pub struct FieldCursor {
    pub anchor: usize,
    pub caret: usize,
}
impl FieldCursor {
    pub fn at_end(text: &str) -> Self {
        Self {
            anchor: text.len(),
            caret: text.len(),
        }
    }
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn replace(&mut self, text: &str, insert: &str) -> String {
        let (value, next) = replace_selection(text, self.selection(), insert);
        self.anchor = next;
        self.caret = next;
        value
    }
    pub fn backspace(&mut self, text: &str) -> String {
        if self.anchor == self.caret && self.caret > 0 {
            self.anchor = text[..self.caret]
                .char_indices()
                .last()
                .map_or(0, |(i, _)| i);
        }
        self.replace(text, "")
    }
    pub fn delete(&mut self, text: &str) -> String {
        if self.anchor == self.caret {
            self.anchor = text[self.caret..]
                .char_indices()
                .nth(1)
                .map_or(text.len(), |(i, _)| self.caret + i);
        }
        self.replace(text, "")
    }
    pub fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.caret = text.len();
    }
    pub fn left(&mut self, text: &str, extend: bool) {
        let next = if !extend && self.anchor != self.caret {
            self.selection().start
        } else {
            text[..self.caret]
                .char_indices()
                .last()
                .map_or(0, |(i, _)| i)
        };
        self.move_to(next, extend);
    }
    pub fn right(&mut self, text: &str, extend: bool) {
        let next = if !extend && self.anchor != self.caret {
            self.selection().end
        } else {
            text[self.caret..]
                .char_indices()
                .nth(1)
                .map_or(text.len(), |(i, _)| self.caret + i)
        };
        self.move_to(next, extend);
    }
    pub fn move_to(&mut self, index: usize, extend: bool) {
        self.caret = index;
        if !extend {
            self.anchor = index;
        }
    }
}
pub fn apply_number_draft(text: &str, model: &mut poc_common::model::Model) {
    if let Ok(value) = text.trim().parse::<f64>() {
        model.set_number(value);
    }
}
pub fn commit_number_draft(text: &str, model: &mut poc_common::model::Model) -> String {
    apply_number_draft(text, model);
    model.state().number.to_string()
}
#[cfg(test)]
mod field_tests {
    use super::*;
    #[test]
    fn numeric_keyboard_editing_replaces_selection_and_deletes_at_the_actual_caret() {
        let mut model = poc_common::model::Model::new(1).unwrap();
        let mut value = model.state().number.to_string();
        let mut cursor = FieldCursor::at_end(&value);
        cursor.select_all(&value);
        value = cursor.replace(&value, "42");
        apply_number_draft(&value, &mut model);
        assert_eq!(model.state().number, 42.0);
        cursor.left(&value, false);
        value = cursor.backspace(&value);
        apply_number_draft(&value, &mut model);
        assert_eq!(value, "2");
        assert_eq!(cursor.caret, 0);
        assert_eq!(model.state().number, 2.0);
        value = cursor.delete(&value);
        apply_number_draft(&value, &mut model);
        assert_eq!(value, "");
        assert_eq!(model.state().number, 2.0);
        value = cursor.replace(&value, "999999");
        apply_number_draft(&value, &mut model);
        assert_eq!(model.state().number, 10000.0);
        assert_eq!(commit_number_draft(&value, &mut model), "10000");
    }
    #[test]
    fn incomplete_numeric_drafts_do_not_replace_valid_shared_state() {
        let mut model = poc_common::model::Model::new(1).unwrap();
        for draft in ["", "-", ".", "1e", "NaN", "inf", "text"] {
            apply_number_draft(draft, &mut model);
            assert_eq!(model.state().number, 10.0);
            assert_eq!(commit_number_draft(draft, &mut model), "10");
        }
        apply_number_draft("-42", &mut model);
        assert_eq!(model.state().number, 0.0);
        assert_eq!(commit_number_draft("0.25", &mut model), "0.25");
    }
    #[test]
    fn selection_movement_preserves_unicode_and_collapses_without_extra_deletion() {
        let mut cursor = FieldCursor::at_end("a日本語z");
        cursor.left("a日本語z", true);
        cursor.left("a日本語z", true);
        assert_eq!(cursor.selection(), 7..11);
        let value = cursor.replace("a日本語z", "42");
        assert_eq!(value, "a日本42");
        assert_eq!(cursor.caret, 9);
        cursor.select_all(&value);
        cursor.left(&value, false);
        assert_eq!(cursor.selection(), 0..0);
        cursor.select_all(&value);
        cursor.right(&value, false);
        assert_eq!(cursor.selection(), value.len()..value.len());
    }
}

pub struct Invalidation{pub layer:LayerId,pub rows:bool}
pub fn activation_invalidation(target:Target)->Option<Invalidation>{let layer=match target{Target::Tab(_)=>LayerId::Title,Target::GroupTab(_)=>LayerId::Layers,Target::Visible(_)=>LayerId::List,_=>return None};Some(Invalidation{layer,rows:matches!(target,Target::Visible(_))})}
#[derive(Default)]pub struct Utf16Input{high:Option<u16>}
impl Utf16Input{pub fn push(&mut self,unit:u16)->[Option<char>;2]{let previous=self.high.take();if (0xd800..=0xdbff).contains(&unit){self.high=Some(unit);return [previous.map(|_|char::REPLACEMENT_CHARACTER),None];}
if let Some(high)=previous{if (0xdc00..=0xdfff).contains(&unit){return [char::from_u32(0x10000+((high as u32-0xd800)<<10)+(unit as u32-0xdc00)),None];}return [Some(char::REPLACEMENT_CHARACTER),Some(char::from_u32(unit as u32).unwrap_or(char::REPLACEMENT_CHARACTER))];}[Some(char::from_u32(unit as u32).unwrap_or(char::REPLACEMENT_CHARACTER)),None]}}
#[cfg(test)]mod input_contract_tests{use super::*;
 #[test]fn visibility_activation_invalidates_pixels_and_accessibility_together(){let mut model=poc_common::model::Model::new(1000).unwrap();assert!(model.activate(4,500));let change=activation_invalidation(Target::Visible(500)).unwrap();assert!(!model.row_visible(500));assert_eq!(change.layer,LayerId::List);assert!(change.rows);assert!(activation_invalidation(Target::Canvas).is_none());}
 #[test]fn utf16_messages_preserve_supplementary_characters_and_recover_after_invalid_units(){let mut decoder=Utf16Input::default();let text:String=[0x41,0xd83d,0xde00,0x65e5,0xd800,0x42,0xdc00].into_iter().flat_map(|unit|decoder.push(unit).into_iter().flatten()).collect();assert_eq!(text,"A😀日�B�");}
}

pub fn popup_item_hit(y_physical:f32,scale:f32)->Option<u32>{if !y_physical.is_finite()||!scale.is_finite()||scale<=0.0{return None;}let top=metric("divider")*0.5;let height=metric("header_height");let count=spec()["menu_items"].as_array().unwrap().len()as u32;let local=y_physical/scale-top;if local<0.0||local>=height*count as f32{return None;}Some((local/height).floor()as u32)}
#[cfg(test)]mod popup_tests{use super::*;#[test]fn popup_hits_use_padded_native_rows_at_both_required_scales(){for scale in [1.25,1.5]{assert_eq!(popup_item_hit(3.0*scale,scale),None);assert_eq!(popup_item_hit(4.0*scale,scale),Some(0));assert_eq!(popup_item_hit(39.9*scale,scale),Some(0));assert_eq!(popup_item_hit(40.0*scale,scale),Some(1));assert_eq!(popup_item_hit(291.9*scale,scale),Some(7));assert_eq!(popup_item_hit(292.0*scale,scale),None);assert_eq!(popup_item_hit(296.0*scale,scale),None);}}}

#[cfg(test)]
mod caption_tests {
    use super::*;
    #[test]
    fn caption_release_requires_the_same_button_and_consumes_the_press() {
        let mut press = CaptionPress::default();
        for button in [Target::Minimize, Target::Maximize, Target::Close] {
            assert!(press.press(Some(button)));
            assert_eq!(press.release(Some(button)), Some(button));
            assert_eq!(press.release(Some(button)), None);
        }
        press.press(Some(Target::Maximize));
        assert_eq!(press.release(Some(Target::Close)), None);
        assert!(!press.is_pressed());
        press.press(Some(Target::Close));
        assert_eq!(press.release(None), None);
        assert!(!press.is_pressed());
    }
    #[test]
    fn cancellation_or_another_nonclient_press_cannot_activate_a_stale_button() {
        let mut press = CaptionPress::default();
        press.press(Some(Target::Close));
        press.cancel();
        assert_eq!(press.release(Some(Target::Close)), None);
        press.press(Some(Target::Maximize));
        assert!(!press.press(None));
        assert_eq!(press.release(Some(Target::Maximize)), None);
    }
}
