use etagere::{AtlasAllocator, size2};
use parley::{
    Affinity, Alignment, AlignmentOptions, Cursor, FontContext, FontData, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};
use std::{collections::HashMap, mem::ManuallyDrop, time::Instant};
use windows::{
    Win32::Graphics::DirectWrite::*,
    core::{Interface, OutRef, PCWSTR, implement},
};

pub const ATLAS_SIZE: u32 = 2048;
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphQuad {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
}
#[derive(Clone, Debug)]
pub struct GlyphUpload {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub pixels: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct Shaped {
    pub glyphs: Vec<GlyphQuad>,
    pub line_breaks: Vec<usize>,
    pub missing: usize,
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
    pub caret: [f32; 4],
    carets: Vec<(usize, [f32; 4])>,
}
impl Shaped {
    pub fn caret_at(&self, byte_index: usize) -> [f32; 4] {
        self.carets[self
            .carets
            .partition_point(|(index, _)| *index <= byte_index)
            .saturating_sub(1)]
        .1
    }
}
#[derive(Clone, Copy)]
struct Tile {
    offset: [i32; 2],
    size: [u32; 2],
    uv: [f32; 4],
}
#[derive(Hash, PartialEq, Eq)]
struct ShapeKey {
    text: String,
    width: u32,
    size: u32,
    scale: u32,
    wrap: bool,
}
#[derive(Hash, PartialEq, Eq)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u16,
    size: u32,
    phase: u8,
}
#[implement(IDWriteTextAnalysisSource)]
struct FontTextSource {
    text: Vec<u16>,
    locale: [u16; 1],
}
impl IDWriteTextAnalysisSource_Impl for FontTextSource_Impl {
    fn GetTextAtPosition(
        &self,
        position: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        let remaining = self.text.get(position as usize..).unwrap_or_default();
        unsafe {
            *text = if remaining.is_empty() {
                std::ptr::null_mut()
            } else {
                remaining.as_ptr().cast_mut()
            };
            *length = remaining.len() as u32;
        }
        Ok(())
    }
    fn GetTextBeforePosition(
        &self,
        position: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        let preceding = self.text.get(..position as usize).unwrap_or_default();
        unsafe {
            *text = if preceding.is_empty() {
                std::ptr::null_mut()
            } else {
                preceding.as_ptr().cast_mut()
            };
            *length = preceding.len() as u32;
        }
        Ok(())
    }
    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }
    fn GetLocaleName(
        &self,
        position: u32,
        length: *mut u32,
        name: *mut *mut u16,
    ) -> windows::core::Result<()> {
        unsafe {
            *length = (self.text.len() as u32).saturating_sub(position);
            *name = self.locale.as_ptr().cast_mut();
        }
        Ok(())
    }
    fn GetNumberSubstitution(
        &self,
        position: u32,
        length: *mut u32,
        substitution: OutRef<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        unsafe {
            *length = (self.text.len() as u32).saturating_sub(position);
        }
        substitution.write(None)
    }
}
struct FontSpan {
    range: std::ops::Range<usize>,
    family: String,
    scale: f32,
}
pub struct TextSystem {
    fonts: FontContext,
    layout: LayoutContext<()>,
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
    system_fonts: IDWriteFontCollection,
    fallback: IDWriteFontFallback,
    faces: HashMap<(u64, u32), IDWriteFontFace>,
    shaped: HashMap<ShapeKey, Shaped>,
    glyphs: HashMap<GlyphKey, Tile>,
    atlas: AtlasAllocator,
    uploads: Vec<GlyphUpload>,
    shaping_ms: f64,
    raster_ms: f64,
}
impl TextSystem {
    pub fn new() -> Result<Self, String> {
        let factory: IDWriteFactory5 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(|error| error.to_string())?;
        let loader =
            unsafe { factory.CreateInMemoryFontFileLoader() }.map_err(|error| error.to_string())?;
        unsafe { factory.RegisterFontFileLoader(&loader) }.map_err(|error| error.to_string())?;
        let mut system_fonts = None;
        unsafe {
            factory
                .cast::<IDWriteFactory>()
                .map_err(|e| e.to_string())?
                .GetSystemFontCollection(&mut system_fonts, false)
        }
        .map_err(|e| e.to_string())?;
        let system_fonts =
            system_fonts.ok_or("DirectWrite system font collection is unavailable")?;
        let fallback = unsafe { factory.GetSystemFontFallback() }.map_err(|e| e.to_string())?;
        Ok(Self {
            fonts: FontContext::new(),
            layout: LayoutContext::new(),
            factory,
            loader,
            system_fonts,
            fallback,
            faces: HashMap::new(),
            shaped: HashMap::new(),
            glyphs: HashMap::new(),
            atlas: AtlasAllocator::new(size2(ATLAS_SIZE as i32, ATLAS_SIZE as i32)),
            uploads: Vec::new(),
            shaping_ms: 0.0,
            raster_ms: 0.0,
        })
    }
    pub fn shape(
        &mut self,
        text: &str,
        width: f32,
        size: f32,
        scale: f32,
    ) -> Result<Shaped, String> {
        self.shape_mode(text, width, size, scale, true)
    }
    pub fn shape_line(
        &mut self,
        text: &str,
        width: f32,
        size: f32,
        scale: f32,
    ) -> Result<Shaped, String> {
        self.shape_mode(text, width, size, scale, false)
    }
    fn shape_mode(
        &mut self,
        text: &str,
        width: f32,
        size: f32,
        scale: f32,
        wrap: bool,
    ) -> Result<Shaped, String> {
        let started = Instant::now();
        if !width.is_finite()
            || width <= 0.0
            || !size.is_finite()
            || size <= 0.0
            || !scale.is_finite()
            || scale <= 0.0
            || !(width * scale).is_finite()
            || !(size * scale).is_finite()
        {
            return Err("invalid text layout dimensions".into());
        }
        let key = ShapeKey {
            text: text.into(),
            width: width.to_bits(),
            size: size.to_bits(),
            scale: scale.to_bits(),
            wrap,
        };
        if let Some(shaped) = self.shaped.get(&key) {
            let result = shaped.clone();
            self.shaping_ms += started.elapsed().as_secs_f64() * 1000.0;
            return Ok(result);
        }
        let mut normalized = String::new();
        let mut indices = Vec::new();
        for (index, ch) in text.char_indices() {
            indices.push((index, if wrap { index } else { normalized.len() }));
            if !wrap {
                normalized.push(
                    if matches!(
                        ch,
                        '\r' | '\n'
                            | '\u{000b}'
                            | '\u{000c}'
                            | '\u{0085}'
                            | '\u{2028}'
                            | '\u{2029}'
                    ) {
                        ' '
                    } else {
                        ch
                    },
                );
            }
        }
        let display = if wrap { text } else { normalized.as_str() };
        indices.push((text.len(), display.len()));
        let spans = self.font_spans(display)?;
        let mut builder = self
            .layout
            .ranged_builder(&mut self.fonts, display, scale, false);
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
            "system-ui".into(),
        )));
        for span in &spans {
            builder.push(
                StyleProperty::FontFamily(parley::FontFamily::named(&span.family)),
                span.range.clone(),
            );
            if span.scale != 1.0 {
                builder.push(
                    StyleProperty::FontSize(size * span.scale),
                    span.range.clone(),
                );
            }
        }
        if !wrap {
            builder.push_default(StyleProperty::TextWrapMode(parley::TextWrapMode::NoWrap));
        }
        let mut layout: Layout<()> = builder.build(display);
        layout.break_all_lines(wrap.then_some(width * scale));
        layout.align(Alignment::Start, AlignmentOptions::default());
        let carets: Vec<_> = indices
            .into_iter()
            .map(|(index, display_index)| {
                let rect = Cursor::from_byte_index(&layout, display_index, Affinity::Upstream)
                    .geometry(&layout, 1.0);
                (
                    index,
                    [
                        rect.x0 as f32,
                        rect.y0 as f32,
                        rect.width() as f32,
                        rect.height() as f32,
                    ],
                )
            })
            .collect();
        let caret = carets.last().unwrap().1;
        self.shaping_ms += started.elapsed().as_secs_f64() * 1000.0;
        let mut result = Shaped {
            glyphs: Vec::new(),
            line_breaks: Vec::new(),
            missing: 0,
            width: layout.width(),
            height: layout.height(),
            line_height: 0.0,
            caret,
            carets,
        };
        for line in layout.lines() {
            result.line_breaks.push(if wrap {
                line.text_range().end
            } else {
                text.len()
            });
            result.line_height = line.metrics().line_height;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                for glyph in run.positioned_glyphs() {
                    if glyph.id == 0 {
                        result.missing += 1;
                    }
                    let glyph_id = u16::try_from(glyph.id)
                        .map_err(|_| "glyph index exceeds DirectWrite range")?;
                    let x = (glyph.x * 4.0).round() / 4.0;
                    let origin = x.floor();
                    let phase = ((x - origin) * 4.0).round() as u8;
                    let started = Instant::now();
                    let tile =
                        self.raster(run.run().font(), glyph_id, run.run().font_size(), phase);
                    self.raster_ms += started.elapsed().as_secs_f64() * 1000.0;
                    let tile = tile?;
                    if tile.size[0] > 0 && tile.size[1] > 0 {
                        result.glyphs.push(GlyphQuad {
                            rect: [
                                origin + tile.offset[0] as f32,
                                glyph.y.round() + tile.offset[1] as f32,
                                tile.size[0] as f32,
                                tile.size[1] as f32,
                            ],
                            uv: tile.uv,
                        });
                    }
                }
            }
        }
        self.shaped.insert(key, result.clone());
        Ok(result)
    }
    pub fn take_metrics(&mut self) -> (f64, f64) {
        (
            std::mem::take(&mut self.shaping_ms),
            std::mem::take(&mut self.raster_ms),
        )
    }
    pub fn take_uploads(&mut self) -> Vec<GlyphUpload> {
        std::mem::take(&mut self.uploads)
    }
    fn font_spans(&mut self, text: &str) -> Result<Vec<FontSpan>, String> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut boundaries = vec![None; wide.len() + 1];
        let mut position = 0;
        for (index, ch) in text.char_indices() {
            boundaries[position] = Some(index);
            position += ch.len_utf16();
        }
        boundaries[position] = Some(text.len());
        let family_id = self
            .fonts
            .collection
            .generic_families(fontique::GenericFamily::SystemUi)
            .next();
        let base = family_id
            .and_then(|id| self.fonts.collection.family_name(id))
            .unwrap_or("");
        let base: Vec<u16> = base.encode_utf16().chain(Some(0)).collect();
        let source: IDWriteTextAnalysisSource = FontTextSource {
            text: wide,
            locale: [0],
        }
        .into();
        let mut offset = 0;
        let mut spans = Vec::new();
        while offset < position {
            let mut length = 0;
            let mut font = None;
            let mut scale = 1.0;
            unsafe {
                self.fallback.MapCharacters(
                    &source,
                    offset as u32,
                    (position - offset) as u32,
                    &self.system_fonts,
                    PCWSTR(base.as_ptr()),
                    DWRITE_FONT_WEIGHT_REGULAR,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    &mut length,
                    &mut font,
                    &mut scale,
                )
            }
            .map_err(|e| e.to_string())?;
            let end = offset
                .checked_add(length as usize)
                .filter(|end| *end > offset && *end <= position)
                .ok_or("DirectWrite fallback returned an invalid text range")?;
            let start_byte =
                boundaries[offset].ok_or("DirectWrite fallback split a UTF16 scalar")?;
            let end_byte = boundaries[end].ok_or("DirectWrite fallback split a UTF16 scalar")?;
            if let Some(font) = font {
                if !scale.is_finite() || scale <= 0.0 {
                    return Err("DirectWrite fallback returned an invalid font scale".into());
                }
                let family = unsafe { font.GetFontFamily() }.map_err(|e| e.to_string())?;
                let names = unsafe { family.GetFamilyNames() }.map_err(|e| e.to_string())?;
                let len = unsafe { names.GetStringLength(0) }.map_err(|e| e.to_string())?;
                let mut name = vec![0u16; len as usize + 1];
                unsafe { names.GetString(0, &mut name) }.map_err(|e| e.to_string())?;
                name.pop();
                let family = String::from_utf16(&name).map_err(|e| e.to_string())?;
                spans.push(FontSpan {
                    range: start_byte..end_byte,
                    family,
                    scale,
                });
            }
            offset = end;
        }
        Ok(spans)
    }
    fn face(&mut self, font: &FontData) -> Result<IDWriteFontFace, String> {
        let key = (font.data.id(), font.index);
        if let Some(face) = self.faces.get(&key) {
            return Ok(face.clone());
        }
        let data = font.data.data();
        let length =
            u32::try_from(data.len()).map_err(|_| "font data exceeds DirectWrite range")?;
        let file = unsafe {
            self.loader.CreateInMemoryFontFileReference(
                &self.factory,
                data.as_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|error| error.to_string())?;
        let reference = unsafe {
            self.factory
                .CreateFontFaceReference(&file, font.index, DWRITE_FONT_SIMULATIONS_NONE)
        }
        .map_err(|error| error.to_string())?;
        let face: IDWriteFontFace = unsafe { reference.CreateFontFace() }
            .map_err(|error| error.to_string())?
            .cast()
            .map_err(|error| error.to_string())?;
        self.faces.insert(key, face.clone());
        Ok(face)
    }
    fn raster(
        &mut self,
        font: &FontData,
        glyph: u16,
        size: f32,
        phase: u8,
    ) -> Result<Tile, String> {
        let key = GlyphKey {
            font: font.data.id(),
            index: font.index,
            glyph,
            size: size.to_bits(),
            phase,
        };
        if let Some(tile) = self.glyphs.get(&key) {
            return Ok(*tile);
        }
        let face = self.face(font)?;
        let mut run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(face)),
            fontEmSize: size,
            glyphCount: 1,
            glyphIndices: &glyph,
            ..Default::default()
        };
        let analysis = unsafe {
            self.factory.CreateGlyphRunAnalysis(
                &run,
                None,
                DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_GRID_FIT_MODE_ENABLED,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                phase as f32 / 4.0,
                0.0,
            )
        };
        unsafe { ManuallyDrop::drop(&mut run.fontFace) };
        let analysis = analysis.map_err(|error| error.to_string())?;
        let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) }
            .map_err(|error| error.to_string())?;
        let width = (bounds.right - bounds.left).max(0) as u32;
        let height = (bounds.bottom - bounds.top).max(0) as u32;
        let mut tile = Tile {
            offset: [bounds.left, bounds.top],
            size: [width, height],
            uv: [0.0; 4],
        };
        if width > 0 && height > 0 {
            if width + 2 > ATLAS_SIZE || height + 2 > ATLAS_SIZE {
                return Err("glyph exceeds atlas dimensions".into());
            }
            let mut pixels = vec![0; width as usize * height as usize];
            unsafe {
                analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut pixels)
            }
            .map_err(|error| error.to_string())?;
            let allocation = self
                .atlas
                .allocate(size2(width as i32 + 2, height as i32 + 2))
                .ok_or("glyph atlas is full")?;
            let origin = [
                allocation.rectangle.min.x as u32 + 1,
                allocation.rectangle.min.y as u32 + 1,
            ];
            tile.uv = [
                origin[0] as f32 / ATLAS_SIZE as f32,
                origin[1] as f32 / ATLAS_SIZE as f32,
                width as f32 / ATLAS_SIZE as f32,
                height as f32 / ATLAS_SIZE as f32,
            ];
            self.uploads.push(GlyphUpload {
                origin,
                size: [width, height],
                pixels,
            });
        }
        self.glyphs.insert(key, tile);
        Ok(tile)
    }
}
impl Drop for TextSystem {
    fn drop(&mut self) {
        self.faces.clear();
        unsafe {
            let _ = self.factory.UnregisterFontFileLoader(&self.loader);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shaped_text_and_glyphs_reuse_their_atlas_entries() {
        let mut system = TextSystem::new().unwrap();
        let first = system.shape("office Layer 123", 240.0, 14.0, 1.5).unwrap();
        assert_eq!(first.missing, 0);
        assert!(!first.glyphs.is_empty());
        let uploads = system.take_uploads();
        assert!(!uploads.is_empty());
        assert!(uploads.iter().all(|upload| upload.pixels.len()
            == upload.size[0] as usize * upload.size[1] as usize
            && upload.pixels.iter().any(|coverage| *coverage > 0)));
        let again = system.shape("office Layer 123", 240.0, 14.0, 1.5).unwrap();
        assert_eq!(first.glyphs, again.glyphs);
        assert!(system.take_uploads().is_empty());
        assert!(first.caret[3] > 0.0);
        assert!(first.line_height > 0.0);
        assert_eq!(first.caret_at(0)[0], 0.0);
        assert_eq!(first.caret_at(16), first.caret);
        let metrics = system.take_metrics();
        assert!(metrics.0 > 0.0 && metrics.1 > 0.0);
        assert_eq!(system.take_metrics(), (0.0, 0.0));
        system.shape("office Layer 123", 240.0, 18.0, 1.5).unwrap();
        assert!(!system.take_uploads().is_empty());
    }
    #[test]
    fn a_long_field_is_one_line_and_wrap_mode_is_part_of_the_layout_cache() {
        let mut system = TextSystem::new().unwrap();
        let text = "Layer abc 日本語 layers drawing art brush opacity size flow spacing";
        let line = system.shape_line(text, 80.0, 14.0, 1.5).unwrap();
        assert_eq!(line.line_breaks, vec![text.len()]);
        assert!(line.width > 120.0);
        assert_eq!(line.caret_at(0)[1], line.caret[1]);
        let wrapped = system.shape(text, 80.0, 14.0, 1.5).unwrap();
        assert!(wrapped.line_breaks.len() > 1);
        let again = system.shape_line(text, 80.0, 14.0, 1.5).unwrap();
        assert_eq!(line.glyphs, again.glyphs);
        let caret = line.caret_at(text.len());
        let scroll = crate::scene::line_scroll(
            0.0,
            crate::scene::Rect::new(caret[0], caret[1], caret[2], caret[3]),
            120.0,
            line.width,
        );
        assert!(caret[0] - scroll >= 0.0);
        assert!(caret[0] + caret[2] - scroll <= 120.0);
        let viewport = crate::scene::Rect::new(0.0, 0.0, 120.0, line.height);
        let visible: Vec<_> = line
            .glyphs
            .iter()
            .filter_map(|glyph| {
                crate::scene::clip_glyph(
                    crate::scene::Rect::new(
                        glyph.rect[0] - scroll,
                        glyph.rect[1],
                        glyph.rect[2],
                        glyph.rect[3],
                    ),
                    glyph.uv,
                    viewport,
                )
            })
            .collect();
        assert!(!visible.is_empty());
        assert!(visible.len() < line.glyphs.len());
        assert!(visible.iter().all(|(r, _)| r.x >= 0.0
            && r.x + r.w <= 120.0
            && r.y >= 0.0
            && r.y + r.h <= line.height));
    }
    #[test]
    fn single_line_segment_breaks_keep_original_utf8_cursor_positions() {
        let mut system = TextSystem::new().unwrap();
        let text = "a\n日本語\u{2028}b";
        let shaped = system.shape_line(text, 80.0, 14.0, 1.5).unwrap();
        assert_eq!(shaped.line_breaks, vec![text.len()]);
        for (index, _) in text.char_indices() {
            let caret = shaped.caret_at(index);
            assert_eq!(caret[1], shaped.caret[1]);
        }
        assert_eq!(shaped.caret_at(text.len()), shaped.caret);
    }
    #[test]
    fn directwrite_fallback_covers_every_authored_multilingual_label() {
        let mut system = TextSystem::new().unwrap();
        for label in poc_common::LABELS {
            let shaped = system
                .shape_line(label, 176.0, crate::scene::metric("ui_font_size"), 1.25)
                .unwrap();
            assert_eq!(shaped.missing, 0, "missing glyphs in {label}");
            assert!(!shaped.glyphs.is_empty());
        }
        let text = "Latin 日本語 图层 简体 圖層 繁體 레이어 ไทย";
        let spans = system.font_spans(text).unwrap();
        assert!(
            spans
                .iter()
                .all(|span| text.is_char_boundary(span.range.start)
                    && text.is_char_boundary(span.range.end))
        );
        assert_eq!(spans.first().unwrap().range.start, 0);
        assert_eq!(spans.last().unwrap().range.end, text.len());
    }
    #[test]
    fn multilingual_wraps_preserve_utf8_boundaries_and_font_fallback() {
        let text =
            "ชั้นภาพวาดสีน้ำมันสำหรับการวาดภาพที่สวยงาม 日本語の文章を複数の行に折り返して表示します。";
        let mut system = TextSystem::new().unwrap();
        let shaped = system.shape(text, 160.0, 14.0, 1.5).unwrap();
        assert_eq!(shaped.missing, 0);
        assert!(shaped.line_breaks.len() > 2);
        assert!(
            shaped
                .line_breaks
                .iter()
                .all(|offset| text.is_char_boundary(*offset))
        );
        assert_eq!(shaped.line_breaks.last(), Some(&text.len()));
        assert_eq!(shaped.caret_at(1), shaped.caret_at(0));
        assert_eq!(shaped.caret_at(usize::MAX), shaped.caret);
    }
}
