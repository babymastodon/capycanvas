//! One optional-work lifecycle per editor. Hosts supply visible IDs, physical
//! geometry and a monotonic clock; they retain only the delivered native images.
use crate::UiSession;
use layer_render::{CanvasRenderer, FilterPreviewImage};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

const IDLE_NS: u64 = 200_000_000;
const RETRY_NS: u64 = 1_000_000_000;
const CACHE_ROWS: usize = 64;
const APPLY_ROWS: usize = 8;

/// Presentation facts, not a host-side invalidation policy. Reporting retained
/// images also recovers from a lost view or a failed native bitmap conversion.
#[derive(Default, Deserialize)]
pub struct FilterPreviewCache {
    pub key: Option<String>,
    pub rows: Vec<Arc<str>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FilterPreviewStatus {
    /// Opaque presentation generation, including catalog and GPU replacement.
    pub key: String,
    pub epoch: u64,
    pub pending: bool,
    /// Zero means service on the next host frame; never spin in the caller.
    pub wait_ms: u32,
    pub retained: Vec<Arc<str>>,
    pub requests: u64,
    pub error: Option<String>,
}
pub struct FilterPreviewUpdate {
    pub status: FilterPreviewStatus,
    pub image: Option<FilterPreviewImage>,
}
struct Pending {
    id: u64,
    filters: Vec<Arc<str>>,
}
#[derive(Default)]
pub(crate) struct Previews {
    pub visible: Vec<Arc<layer_core::EffectProgram>>,
    source: Option<u64>,
    generation: u64,
    serial: u64,
    requests: u64,
    pending: Option<Pending>,
    loaded: BTreeMap<Arc<str>, u64>,
    retry_at: u64,
}
impl Previews {
    pub fn renderer_replaced(&mut self) {
        self.visible.clear();
        self.source = None;
        self.pending = None;
        self.loaded.clear();
        self.generation = self.generation.wrapping_add(1);
        self.retry_at = 0;
    }
    fn cancel<R: CanvasRenderer>(&mut self, renderer: &mut R) {
        if self.pending.take().is_some() {
            renderer.cancel_filter_previews();
        }
    }
    fn invalidate<R: CanvasRenderer>(&mut self, renderer: &mut R, now_ns: u64) {
        self.cancel(renderer);
        self.loaded.clear();
        self.generation = self.generation.wrapping_add(1);
        self.retry_at = now_ns.saturating_add(IDLE_NS);
    }
}

impl<R: CanvasRenderer> UiSession<R> {
    /// A host output-color change can replace the preview surface without
    /// changing document pixels. Retire its requests through the same lifecycle.
    pub fn reset_filter_previews(&mut self) {
        self.renderer_mut().cancel_filter_previews();
        self.filter_previews.renderer_replaced();
        self.prepare_shader_context();
    }
    /// Poll once per host wake-up. An empty visible list cancels optional work.
    /// This call may submit one bounded GPU chunk, and must run on its owner.
    pub fn poll_filter_previews(
        &mut self,
        now_ns: u64,
        filters: Vec<Arc<str>>,
        cache: FilterPreviewCache,
    ) -> Result<FilterPreviewUpdate, String> {
        if filters.len() > 256
            || cache.rows.len() > CACHE_ROWS
            || filters.iter().chain(&cache.rows).any(|id| id.len() > 256)
        {
            return Err(self.state.localization.text(crate::MessageId::RESOURCES_INVALID_VISIBLE_FILTERS).to_string());
        }
        let mut apply = Vec::new();
        let mut ordered: Vec<_> = filters.iter().collect();
        ordered.sort_by_key(|id| self.state.adjustments.iter().position(|choice| choice.id.as_ref() == id.as_ref()).unwrap_or(usize::MAX));
        for id in ordered {
            if let Some(definition) = self.effect_catalog.get(id) {
                let program = definition.program();
                if !apply.contains(&program) { apply.push(program); }
            }
            if apply.len() == APPLY_ROWS { break; }
        }
        if self.filter_previews.visible != apply {
            self.filter_previews.visible = apply;
        }
        self.prepare_shader_context();
        let mut visible = Vec::new();
        for id in filters {
            if self.effect_catalog.get(&id).is_some_and(|definition| !super::effects::bundled_preview(definition)) && !visible.contains(&id) {
                visible.push(id);
            }
            if visible.len() == CACHE_ROWS {
                break;
            }
        }
        let source = self.filter_preview_revision();
        let idle = self.filter_previews_idle();
        let mut driver = std::mem::take(&mut self.filter_previews);
        if driver.source != Some(source) {
            driver.invalidate(self.renderer_mut(), now_ns);
            driver.source = Some(source);
        }
        if !idle {
            driver.cancel(self.renderer_mut());
            driver.retry_at = now_ns.saturating_add(IDLE_NS);
        }
        let key = driver.generation.to_string();
        if cache.key.as_deref() != Some(&key) {
            driver.loaded.clear();
        } else {
            let previous = driver.loaded.len();
            driver.loaded.retain(|id, _| cache.rows.contains(id));
            if previous != driver.loaded.len() {
                driver.retry_at = now_ns.saturating_add(RETRY_NS);
            }
        }
        if visible.is_empty()
            || driver
                .pending
                .as_ref()
                .is_some_and(|p| p.filters.iter().all(|id| !visible.contains(id)))
        {
            driver.cancel(self.renderer_mut());
            driver.retry_at = driver.retry_at.max(now_ns.saturating_add(IDLE_NS));
        }
        for id in &visible {
            if let Some(used) = driver.loaded.get_mut(id) {
                *used = now_ns;
            }
        }
        let mut image = None;
        let mut error = None;
        if idle && !visible.is_empty() {
            if driver.pending.is_some()
                && let Some(result) = self.renderer_mut().take_filter_previews()
            {
                let pending = driver.pending.take().unwrap();
                match result {
                    Ok(atlas)
                        if atlas.image.request_id == pending.id
                            && atlas.filters == pending.filters
                            && [atlas.image.width, atlas.image.height / atlas.filters.len().max(1) as u32] == layer_render::FILTER_PREVIEW_SIZE
                            && atlas.image.height % atlas.filters.len().max(1) as u32 == 0
                            && atlas.image.stride == atlas.image.width * 4
                            && atlas.image.bytes.len()
                                == (atlas.image.stride * atlas.image.height) as usize =>
                    {
                        for id in &atlas.filters {
                            driver.loaded.insert(id.clone(), now_ns);
                        }
                        while driver.loaded.len() > CACHE_ROWS {
                            let oldest = driver
                                .loaded
                                .iter()
                                .filter(|(id, _)| !visible.contains(id))
                                .min_by_key(|(_, used)| *used)
                                .map(|(id, _)| id.clone())
                                .unwrap();
                            driver.loaded.remove(&oldest);
                        }
                        image = Some(atlas);
                    }
                    Ok(_) => error = Some("Invalid or stale filter preview atlas".into()),
                    Err(e) => error = Some(e.to_string()),
                }
            }
            if error.is_none() && driver.pending.is_none() && now_ns >= driver.retry_at {
                let missing: Vec<_> = visible.iter().filter(|id| !driver.loaded.contains_key(*id)).collect();
                let immediate: Vec<_> = missing.iter().copied()
                    .filter(|id| self.effect_catalog.get(id).unwrap().program.analysis().is_none()).collect();
                let missing: Vec<_> = if immediate.is_empty() { missing.into_iter().take(1).cloned().collect() }
                    else { immediate.into_iter().take(8).cloned().collect() };
                if !missing.is_empty() {
                    driver.serial = driver.serial.wrapping_add(1);
                    match self.request_filter_previews(driver.serial, missing.clone())
                    {
                        Ok(true) => {
                            driver.requests += 1;
                            driver.pending = Some(Pending {
                                id: driver.serial,
                                filters: missing,
                            });
                        }
                        Ok(false) => driver.retry_at = now_ns.saturating_add(16_000_000),
                        Err(e) => error = Some(e),
                    }
                }
            }
        }
        if error.is_some() {
            driver.cancel(self.renderer_mut());
            driver.retry_at = now_ns.saturating_add(RETRY_NS);
        }
        let wait_ms = if !idle {
            50
        } else if driver.pending.is_some() {
            0
        } else if visible.iter().any(|id| !driver.loaded.contains_key(id)) {
            driver.retry_at.saturating_sub(now_ns).div_ceil(1_000_000) as u32
        } else {
            (IDLE_NS / 1_000_000) as u32
        };
        let status = FilterPreviewStatus {
            key,
            epoch: self.state.document_file.epoch,
            pending: driver.pending.is_some(),
            wait_ms,
            retained: driver.loaded.keys().cloned().collect(),
            requests: driver.requests,
            error: error.map(|error| {
                eprintln!("Filter preview: {error}");
                self.state.localization.text(crate::MessageId::RESOURCES_PREVIEW_FAILED).to_string()
            }),
        };
        self.filter_previews = driver;
        Ok(FilterPreviewUpdate { status, image })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::test_support::{Recorder, custom_filter, package_json};
    use layer_core::Document;
    use layer_render::{BackendError, ReadbackImage};

    #[derive(Default)]
    struct View {
        key: Option<String>,
        rows: Vec<Arc<str>>,
    }
    impl View {
        fn poll(
            &mut self,
            s: &mut UiSession<Recorder>,
            ms: u64,
            ids: &[&str],
        ) -> FilterPreviewUpdate {
            let update = s
                .poll_filter_previews(
                    ms * 1_000_000,
                    ids.iter().map(|id| Arc::from(*id)).collect(),
                    FilterPreviewCache {
                        key: self.key.clone(),
                        rows: self.rows.clone(),
                    },
                )
                .unwrap();
            if self.key.as_ref() != Some(&update.status.key) {
                self.rows.clear();
            }
            self.key = Some(update.status.key.clone());
            self.rows.retain(|id| update.status.retained.contains(id));
            if let Some(image) = &update.image {
                self.rows.extend(image.filters.iter().cloned());
            }
            update
        }
    }
    fn session() -> UiSession<Recorder> {
        let mut session = UiSession::new(
            Recorder::default(),
            Document::new(layer_core::PortableId::random(), 512, 512, layer_core::DocumentNames { paint: "Current ink".into(), paper: "Paper".into() }),
            [512, 512],
            crate::Platform::Gtk,
        )
        .unwrap();
        session.frame(0, 0).unwrap();
        let filters = LIVE.iter().map(|id| custom_filter(id)).collect();
        let package = package_json(session.effect_catalog.categories().to_vec(), filters);
        session.load_effect_package(&package, |_| panic!("inline package"), layer_core::EffectInstallMode::Merge).unwrap();
        session.frame(1, 1).unwrap();
        let request = session.renderer_mut().validation.take().unwrap();
        session.renderer_mut().validation_result = Some(layer_render::EffectValidationResult { request_id: request.request_id, result: Ok(()) });
        session.frame(2, 2).unwrap();
        assert!(session.filter_previews_idle());
        session
    }
    const LIVE: [&str; 12] = ["curves", "levels", "brightness_contrast", "threshold", "exposure", "vignette", "invert", "desaturate", "hue_saturation", "photo_filter", "shadows_highlights", "clarity"];
    fn finish(s: &mut UiSession<Recorder>) {
        let r = s.renderer_mut();
        let request = r.filter_preview.as_ref().unwrap();
        let [width, height] = layer_render::FILTER_PREVIEW_SIZE;
        let height = height * request.filters.len() as u32;
        r.filter_preview_ready = Some(Ok(FilterPreviewImage {
            image: ReadbackImage {
                request_id: request.request_id,
                width,
                height,
                stride: width * 4,
                bytes: vec![255; (width * height * 4) as usize],
            },
            filters: request
                .filters
                .iter()
                .map(|f| f.program.id.clone())
                .collect(),
        }));
    }
    #[test]
    fn language_refresh_preserves_pending_previews_and_completed_images() {
        let mut s = session();
        let mut v = View::default();
        v.poll(&mut s, 0, &["test:curves", "test:levels"]);
        let pending = v.poll(&mut s, 200, &["test:curves", "test:levels"]);
        assert!(pending.status.pending);
        let source = s.filter_preview_revision();
        let key = pending.status.key;
        let requests = s.renderer_mut().filter_preview_requests;
        let request_id = s.renderer_mut().filter_preview.as_ref().unwrap().request_id;
        assert!(s.set_localization(crate::Localizer::shared(crate::UiLanguage::Japanese)));
        assert_eq!(s.filter_preview_revision(), source);
        let pending = v.poll(&mut s, 208, &["test:curves", "test:levels"]);
        assert_eq!(pending.status.key, key);
        assert!(pending.status.pending);
        assert_eq!(s.renderer_mut().filter_preview_requests, requests);
        assert_eq!(s.renderer_mut().filter_preview.as_ref().unwrap().request_id, request_id);
        assert_eq!(s.renderer_mut().filter_preview_cancels, 0);
        finish(&mut s);
        let completed = v.poll(&mut s, 216, &["test:curves", "test:levels"]);
        assert!(completed.image.is_some());
        let retained = completed.status.retained;
        assert!(s.set_localization(crate::Localizer::shared(crate::UiLanguage::Korean)));
        let warm = v.poll(&mut s, 224, &["test:curves", "test:levels"]);
        assert_eq!(warm.status.key, key);
        assert_eq!(warm.status.retained, retained);
        assert!(!warm.status.pending);
        assert!(warm.image.is_none());
        assert_eq!(s.renderer_mut().filter_preview_requests, requests);
        assert_eq!(s.renderer_mut().filter_preview_cancels, 0);
    }

    #[test]
    fn pending_work_is_prompt_idle_work_is_debounced_and_rows_are_reused() {
        let mut s = session();
        let mut v = View::default();
        assert!(
            !v.poll(&mut s, 0, &["test:curves", "test:curves", "test:levels"])
                .status
                .pending
        );
        assert_eq!(s.renderer_mut().filter_preview_requests, 0);
        assert!(v.poll(&mut s, 200, &["test:curves", "test:levels"]).status.pending);
        let pending = v.poll(&mut s, 208, &["test:curves", "test:levels"]);
        assert_eq!(pending.status.wait_ms, 0);
        assert_eq!(s.renderer_mut().filter_preview_takes, 1);
        finish(&mut s);
        assert!(v.poll(&mut s, 216, &["test:curves", "test:levels"]).image.is_some());
        v.poll(&mut s, 500, &[]);
        let reopened = v.poll(&mut s, 700, &["test:levels"]);
        assert_eq!(reopened.status.requests, 1);
        assert!(!reopened.status.pending);
        assert!(reopened.status.wait_ms >= 200);
        assert_eq!(v.poll(&mut s, 900, &["test:curves"]).status.requests, 1, "Other projections reuse rows");
    }
    #[test]
    fn bundled_rows_never_request_previews() {
        let mut s = session();
        let mut v = View::default();
        let bundled: Vec<_> = s.state.adjustments.iter().filter(|choice| choice.bundled_preview).map(|choice| choice.id.clone()).collect();
        assert_eq!(bundled.len(), layer_core::bundled_effect_catalog().filters().len());
        assert!(s.state.adjustments.iter().filter(|choice| !choice.bundled_preview).all(|choice| choice.id.starts_with("test:")));
        let ids: Vec<&str> = bundled.iter().map(AsRef::as_ref).collect();
        v.poll(&mut s, 0, &ids);
        let update = v.poll(&mut s, 200, &ids);
        assert!(!update.status.pending);
        assert!(update.status.retained.is_empty());
        assert_eq!(s.renderer_mut().filter_preview_requests, 0);
    }
    #[test]
    fn drawing_pauses_pending_work_and_keeps_delivered_rows() {
        let mut s = session();
        let mut v = View::default();
        v.poll(&mut s, 0, &["test:curves"]);
        v.poll(&mut s, 200, &["test:curves"]);
        finish(&mut s);
        s.input_pending = true;
        let paused = v.poll(&mut s, 208, &["test:curves"]);
        assert!(!paused.status.pending);
        assert_eq!(paused.status.wait_ms, 50);
        assert!(paused.image.is_none());
        assert_eq!(s.renderer_mut().filter_preview_takes, 0);
        assert_eq!(s.renderer_mut().filter_preview_cancels, 1);
        v.poll(&mut s, 500, &["test:curves"]);
        assert_eq!(s.renderer_mut().filter_preview_requests, 1);
        s.input_pending = false;
        v.poll(&mut s, 600, &["test:curves"]);
        assert_eq!(s.renderer_mut().filter_preview_requests, 1);
        v.poll(&mut s, 700, &["test:curves"]);
        assert_eq!(s.renderer_mut().filter_preview_requests, 2);
        finish(&mut s);
        assert!(v.poll(&mut s, 708, &["test:curves"]).image.is_some());
        let key = v.key.clone().unwrap();
        s.input_pending = true;
        let painting = v.poll(&mut s, 716, &["test:curves"]);
        assert_eq!(painting.status.key, key);
        assert_eq!(painting.status.retained, [Arc::from("test:curves")]);
        s.input_pending = false;
        assert_eq!(v.poll(&mut s, 1000, &["test:curves"]).status.key, key);
        assert_eq!(s.renderer_mut().filter_preview_requests, 2);
    }
    #[test]
    fn hiding_and_renderer_replacement_retire_work_but_documents_keep_rows() {
        let mut s = session();
        let mut v = View::default();
        v.poll(&mut s, 0, &["test:curves"]);
        v.poll(&mut s, 200, &["test:curves"]);
        let original = v.key.clone();
        v.poll(&mut s, 208, &[]);
        assert_eq!(s.renderer_mut().filter_preview_cancels, 1);
        v.poll(&mut s, 408, &["test:curves"]);
        finish(&mut s);
        s.state.document_file.epoch += 1;
        crate::session::test_support::layer(&mut s, crate::LayerAction::New { group: false, clipped: false });
        s.frame(1, 1).unwrap();
        let kept = v.poll(&mut s, 416, &["test:curves"]);
        assert!(kept.image.is_some(), "Other documents and edits preview the same scene");
        assert_eq!(v.key, original);
        assert_eq!(s.renderer_mut().filter_preview_cancels, 1);
        let before_gpu = v.key.clone();
        s.replace_renderer(Recorder::default()).unwrap();
        s.frame(700_000_000, 700_000_000).unwrap();
        assert!(v.poll(&mut s, 700, &["test:curves"]).image.is_none());
        assert_ne!(v.key, before_gpu);
        assert!(v.poll(&mut s, 900, &["test:curves"]).status.pending);
    }
    #[test]
    fn changing_visible_categories_retires_work_and_stale_atlases_back_off() {
        let mut s = session();
        let mut v = View::default();
        v.poll(&mut s, 0, &["test:curves"]);
        v.poll(&mut s, 200, &["test:curves"]);
        finish(&mut s);
        let changed = v.poll(&mut s, 208, &["test:levels"]);
        assert!(changed.image.is_none());
        assert!(!changed.status.pending);
        assert_eq!(s.renderer_mut().filter_preview_cancels, 1);
        assert_eq!(s.renderer_mut().filter_preview_takes, 0);
        v.poll(&mut s, 408, &["test:levels"]);
        finish(&mut s);
        s.renderer_mut()
            .filter_preview_ready
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .image
            .request_id = 1;
        let stale = v.poll(&mut s, 416, &["test:levels"]);
        assert!(stale.image.is_none());
        assert!(stale.status.error.is_some());
        assert_eq!(stale.status.wait_ms, 1000);
        assert!(stale.status.retained.is_empty());
        assert!(!v.poll(&mut s, 1000, &["test:levels"]).status.pending);
        assert!(v.poll(&mut s, 1416, &["test:levels"]).status.pending);
    }
    #[test]
    fn optional_failures_back_off_and_missing_host_images_can_be_retried() {
        let mut s = session();
        let mut v = View::default();
        s.renderer_mut().reject_filter_previews = true;
        v.poll(&mut s, 0, &["test:curves"]);
        let failure = v.poll(&mut s, 200, &["test:curves"]);
        assert!(failure.status.error.is_some());
        assert_eq!(failure.status.wait_ms, 1000);
        s.renderer_mut().reject_filter_previews = false;
        assert!(!v.poll(&mut s, 1199, &["test:curves"]).status.pending);
        assert!(v.poll(&mut s, 1200, &["test:curves"]).status.pending);
        finish(&mut s);
        v.poll(&mut s, 1208, &["test:curves"]);
        v.rows.clear(); // Native conversion failed or its retained image was lost.
        assert!(!v.poll(&mut s, 1400, &["test:curves"]).status.pending);
        assert!(v.poll(&mut s, 2400, &["test:curves"]).status.pending);
        s.renderer_mut().filter_preview_ready = Some(Err(BackendError("readback failed")));
        assert!(v.poll(&mut s, 2408, &["test:curves"]).status.error.is_some());
        assert!(!v.poll(&mut s, 2500, &["test:curves"]).status.pending);
        assert!(v.poll(&mut s, 3408, &["test:curves"]).status.pending);
    }
    #[test]
    fn ordinary_rows_publish_before_filters_that_need_full_source_analysis() {
        let mut s = session();
        let mut v = View::default();
        let ids = ["test:shadows_highlights", "test:curves", "test:levels", "test:clarity"];
        v.poll(&mut s, 0, &ids);
        v.poll(&mut s, 200, &ids);
        let requested = |s: &mut UiSession<Recorder>| s.renderer_mut().filter_preview.as_ref().unwrap().filters
            .iter().map(|f| f.program.id.to_string()).collect::<Vec<_>>();
        assert_eq!(requested(&mut s), ["test:curves", "test:levels"]);
        finish(&mut s);
        let update = v.poll(&mut s, 208, &ids);
        assert_eq!(update.image.unwrap().filters.iter().map(AsRef::as_ref).collect::<Vec<&str>>(), ["test:curves", "test:levels"]);
        assert_eq!(requested(&mut s), ["test:shadows_highlights"]);
        finish(&mut s);
        v.poll(&mut s, 216, &ids);
        assert_eq!(requested(&mut s), ["test:clarity"]);
    }

    #[test]
    fn preparing_shaders_retries_on_the_next_frame_without_repeating_the_idle_delay() {
        let mut s = session();
        let mut v = View::default();
        s.renderer_mut().preparing_filter_previews = true;
        v.poll(&mut s, 0, &["test:curves"]);
        let preparing = v.poll(&mut s, 200, &["test:curves"]);
        assert!(!preparing.status.pending);
        assert_eq!(preparing.status.wait_ms, 16);
        assert_eq!(v.poll(&mut s, 210, &["test:curves"]).status.wait_ms, 6);
        s.renderer_mut().preparing_filter_previews = false;
        assert!(v.poll(&mut s, 216, &["test:curves"]).status.pending);
    }

    #[test]
    fn visible_rows_are_batched_and_host_cache_is_bounded() {
        let mut s = session();
        let mut v = View::default();
        let ids: Vec<String> = s
            .effect_catalog
            .filters()
            .iter()
            .filter(|f| f.program.analysis().is_none() && !super::super::effects::bundled_preview(f))
            .take(10)
            .map(|f| f.program.id.to_string())
            .collect();
        let ids: Vec<_> = ids.iter().map(String::as_str).collect();
        assert_eq!(ids.len(), 10);
        v.poll(&mut s, 0, &ids);
        v.poll(&mut s, 200, &ids);
        assert_eq!(s.renderer_mut().filter_preview.as_ref().unwrap().filters.len(), 8);
        finish(&mut s);
        let first = v.poll(&mut s, 208, &ids);
        assert_eq!(first.image.unwrap().filters.len(), 8);
        assert_eq!(s.renderer_mut().filter_preview.as_ref().unwrap().filters.len(), 2);
        // Fill the cache with previously visited offscreen rows before delivery.
        for i in 0..56 {
            let id: Arc<str> = format!("offscreen-{i}").into();
            s.filter_previews.loaded.insert(id.clone(), 0);
            v.rows.push(id);
        }
        finish(&mut s);
        let second = v.poll(&mut s, 216, &ids);
        assert_eq!(second.status.retained.len(), CACHE_ROWS);
        assert!(ids.iter().all(|id| {
            second
                .status
                .retained
                .iter()
                .any(|kept| kept.as_ref() == *id)
        }));
    }
}

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use crate::session::test_support::session;

    #[test]
    fn painting_with_docked_visible_filters_keeps_the_prepared_context() {
        use crate::session::test_support::{pen_at, FILTER_DOCUMENT_PREPARATIONS};
        use layer_engine::PenPhase;
        let mut session = session(crate::Platform::Gtk);
        session.frame(0, 0).unwrap();
        let initial_revision = session.engine().document().revision;
        for (sequence, phase, point) in [(1, PenPhase::Down, [450., 450.]),
            (2, PenPhase::Move, [500., 500.]), (3, PenPhase::Up, [550., 550.])] {
            pen_at(&mut session, sequence, phase, point);
            session.frame(sequence * 20_000_000, sequence * 20_000_000).unwrap();
        }
        session.frame(200_000_000, 200_000_000).unwrap();
        assert!(session.engine().document().revision > initial_revision);
        assert!(!session.engine().has_pending_document_edits());
        let target = session.engine().document().drawing_target().unwrap();
        let data = session.engine().document().target_raster(target).unwrap().try_data().unwrap().unwrap();
        assert!(data.tiles.values().any(|tile| tile.descriptor().channels > 1
            && tile.descriptor().depth() == layer_core::color::SampleDepth::U8));
        session.state.workspace.layout.set_panel_visible(crate::Panel::Adjustments, true).unwrap();
        let group = session.state.workspace.layout.panel_group(crate::Panel::Adjustments).unwrap();
        session.state.workspace.layout.select_tab(group, crate::Panel::Adjustments).unwrap();
        session.changed(crate::regions::LAYOUT, false);
        session.frame(210_000_000, 210_000_000).unwrap();
        let visible = vec![Arc::from("curves"), Arc::from("gaussian_blur")];
        session.poll_filter_previews(210_000_000, visible.clone(), FilterPreviewCache::default()).unwrap();
        assert_eq!(session.engine().backend().contexts.last().unwrap().filters.len(), visible.len());
        let before = (session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get()));
        let revision = session.engine().document().revision;
        for (sequence, phase, point) in [(4, PenPhase::Down, [450., 450.]),
            (5, PenPhase::Move, [500., 500.]), (6, PenPhase::Up, [550., 550.])] {
            pen_at(&mut session, sequence, phase, point);
            let now = 200_000_000 + sequence * 20_000_000;
            session.frame(now, now).unwrap();
            session.poll_filter_previews(now, visible.clone(), FilterPreviewCache::default()).unwrap();
        }
        session.frame(400_000_000, 400_000_000).unwrap();
        session.poll_filter_previews(400_000_000, visible, FilterPreviewCache::default()).unwrap();
        assert!(!session.engine().has_pending_input());
        assert!(!session.engine().has_active_stroke());
        assert!(!session.engine().has_pending_document_edits());
        assert!(session.engine().document().revision > revision, "the stroke must publish");
        let after = (session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get()));
        assert_eq!(after, before, "painting must neither redeclare contexts nor rebuild candidate documents");
    }

    #[test]
    fn coarse_filter_preparation_survives_layer_and_selection_changes() {
        use crate::session::test_support::{rectangle, select, FILTER_DOCUMENT_PREPARATIONS};
        let mut session = session(crate::Platform::Gtk);
        let first = session.engine().document().working.occurrence.unwrap();
        session.dispatch(crate::UiAction::Layer { action: crate::LayerAction::New { group: false, clipped: false } }).unwrap();
        select(&mut session, rectangle([100., 100., 300., 300.]));
        session.poll_filter_previews(0, vec![Arc::from("curves")], FilterPreviewCache::default()).unwrap();
        let key = layer_render_wgpu::ShaderDocument::new(session.engine().document());
        let brush = layer_render_wgpu::ShaderBrushKey::new(session.engine().document(), session.engine().brush());
        let before = (session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get()));
        for action in [crate::LayerAction::Select { id: crate::occurrence_token(first), mask: false },
            crate::LayerAction::Visibility { id: crate::occurrence_token(first), value: false }] {
            session.dispatch(crate::UiAction::Layer { action }).unwrap();
            assert!(key.matches(session.engine().document()), "ordinary layer changes keep startup preparation");
            assert!(brush == layer_render_wgpu::ShaderBrushKey::new(session.engine().document(), session.engine().brush()));
            assert_eq!((session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get())), before);
        }
        let pixels = layer_core::SelectionPixels::new([8, 1], [0, 0, 8, 1], vec![0x44444444]).unwrap();
        select(&mut session, layer_core::Selection { shape: layer_core::SelectionShape::Pixels(Arc::new(pixels)),
            affine: layer_core::Affine::IDENTITY, inverted: false });
        assert!(key.matches(session.engine().document()), "selection shape keeps startup preparation");
        assert!(brush == layer_render_wgpu::ShaderBrushKey::new(session.engine().document(), session.engine().brush()));
        assert_eq!((session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get())), before);
    }

    #[test]
    fn visible_filters_reprepare_after_coarse_dependencies_change() {
        use crate::session::test_support::{rectangle, select, FILTER_DOCUMENT_PREPARATIONS};
        use layer_core::authored::RecordChange;
        let mut session = session(crate::Platform::Gtk);
        session.frame(0, 0).unwrap();
        let visible = vec![Arc::from("curves"), Arc::from("gaussian_blur")];
        session.poll_filter_previews(0, visible.clone(), FilterPreviewCache::default()).unwrap();
        let mut before = (session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get()));
        select(&mut session, rectangle([100., 100., 300., 300.]));
        session.poll_filter_previews(2, visible.clone(), FilterPreviewCache::default()).unwrap();
        let after = (session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get()));
        assert!(after.0 > before.0 && after.1 > before.1);
        before = after;
        select(&mut session, rectangle([150., 150., 350., 350.]));
        session.poll_filter_previews(2, visible.clone(), FilterPreviewCache::default()).unwrap();
        assert_eq!((session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get())), before,
            "selection geometry reuses the same recipe kind");
        let document = session.engine().document();
        let mut composition = document.composition().clone();
        composition.color.space = layer_core::color::RgbSpace::DisplayP3;
        let color = composition.color;
        let change = RecordChange::replace(&document.artwork.compositions, document.artwork.root, Some(composition)).unwrap();
        let (prepared, _) = session.prepare_document_color_transition(layer_core::ColorTransition::Apply {
            edit: Box::new(layer_core::Edit::Composition(change)),
        }).unwrap();
        session.renderer_mut().prepared_color = Some(color);
        session.commit_document_color_transition(prepared).unwrap();
        session.poll_filter_previews(4, visible, FilterPreviewCache::default()).unwrap();
        let after = (session.engine().backend().contexts.len(), FILTER_DOCUMENT_PREPARATIONS.with(|count| count.get()));
        assert!(after.0 > before.0 && after.1 > before.1);
    }

    #[test]
    fn bundled_visible_rows_prepare_apply_in_picker_order_without_live_previews() {
        let mut session = session(crate::Platform::Gtk);
        session.frame(0, 0).unwrap();
        assert!(session.filter_previews_idle());
        session.state.filter_picker.category = None;
        session.state.adjustments = super::super::effects::catalog(&session.effect_catalog,
            &session.state.filter_picker, &session.state.localization);
        let ordered: Vec<_> = session.state.adjustments.iter().filter(|choice| choice.bundled_preview)
            .take(APPLY_ROWS + 2).map(|choice| choice.id.clone()).collect();
        assert_eq!(ordered.len(), APPLY_ROWS + 2);
        let shown: Vec<_> = ordered.iter().rev().cloned().collect();
        let before = session.engine().backend().contexts.len();
        for now in [0, 200_000_000, 400_000_000] {
            let update = session.poll_filter_previews(now, shown.clone(), FilterPreviewCache::default()).unwrap();
            assert_eq!(update.status.requests, 0);
            assert!(update.image.is_none());
            assert_eq!(session.engine().backend().contexts.len(), before + 1);
        }
        let prepared: Vec<_> = session.engine().backend().contexts.last().unwrap().filters.iter()
            .map(|filter| filter.program.id.clone()).collect();
        assert_eq!(prepared, ordered[..APPLY_ROWS]);
        session.changed(crate::regions::CAMERA, false);
        assert_eq!(session.engine().backend().contexts.len(), before + 1);
        session.input_pending = true;
        session.poll_filter_previews(450_000_000, shown.clone(), FilterPreviewCache::default()).unwrap();
        assert_eq!(session.engine().backend().contexts.last().unwrap().filters.len(), APPLY_ROWS);
        let held = session.engine().backend().contexts.len();
        session.changed(crate::regions::DOCUMENT, true);
        assert_eq!(session.engine().backend().contexts.len(), held);
        session.input_pending = false;
        session.poll_filter_previews(500_000_000, shown, FilterPreviewCache::default()).unwrap();
        assert_eq!(session.engine().backend().contexts.last().unwrap().filters.len(), APPLY_ROWS);
        session.poll_filter_previews(600_000_000, Vec::new(), FilterPreviewCache::default()).unwrap();
        assert!(session.engine().backend().contexts.last().unwrap().filters.is_empty());
        assert_eq!(session.engine().backend().contexts.len(), before + 2);
    }
}
