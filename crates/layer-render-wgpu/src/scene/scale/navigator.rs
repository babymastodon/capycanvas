use super::*;

#[derive(Default)]
pub(crate) struct Navigator {
    cache: Option<Cache>,
    refreshed: f32,
    pub revision: u64,
}

impl Navigator {
    pub fn pending(&self) -> bool {
        self.cache.as_ref().is_some_and(|cache| cache.presented_matches_expected().is_some_and(|matches|
            !matches || cache.submission_valid.as_ref().is_some_and(|valid|!valid.load(std::sync::atomic::Ordering::Acquire))))
    }
    pub fn waiting_for_name(&self)->bool {
        self.cache.as_ref().is_some_and(|cache|cache.presented_matches_expected().is_none())
    }
    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.cache.as_ref().filter(|c| c.submission_valid.as_ref()
            .is_none_or(|v| v.load(std::sync::atomic::Ordering::Acquire)))
            .and_then(|c| c.pixels.committed_root()).map(pages::CommittedImage::view)
    }
    pub fn scale(&self) -> f32 { self.cache.as_ref().map_or(1., |c| (1 << c.plan.level) as f32) }
    pub fn storage_bytes(&self) -> u64 { self.cache.as_ref().map_or(0, Cache::window_bytes) }

    pub(in crate::scene) fn prepare(&mut self,r:&WgpuRasterizer,packet:FramePacket<'_>,scene:&Scene,pages:Pages,reset:bool)->Result<(),GpuRasterError> {
        if reset {self.cache=None;}
        let plan=display_mips::Plan::new(packet.document_extent)?;
        if self.cache.as_ref().is_none_or(|cache|cache.plan!=plan) {
            self.cache=Some(Cache::new(r,Request {plan,evaluation:Evaluation::Display},packet.scene.order().len()));
        }
        let cache=self.cache.as_mut().unwrap();
        cache.pages=pages;
        cache.prepare_graph(r,packet,scene)
    }
    pub(super) fn append_wanted_work(&self,r:&WgpuRasterizer,packet:FramePacket<'_>,scene:&Scene,windows:&mut Vec<pages::Interest>) {
        if let Some(cache)=&self.cache {cache.append_wanted_work(r,packet,scene,windows);}
    }

    pub fn refresh(&mut self, r: &mut WgpuRasterizer, packet: FramePacket<'_>, changed: bool,
        held: bool, encoder: &mut crate::submission::CommandEncoder,
    ) -> Result<(), GpuRasterError> {
        crate::performance_trace::counter(c"Capy Navigator age us", ((packet.time_seconds - self.refreshed).max(0.) * 1e6) as u64);
        crate::performance_trace::counter(c"Capy Navigator pending", u64::from(self.pending()));
        crate::performance_trace::counter(c"Capy Navigator bytes", self.storage_bytes());
        let Some(display) = r.scale_display.as_ref() else { return Ok(()); };
        let pages=display.pages.clone();
        self.prepare(r,packet,r.scene.as_ref().unwrap(),pages,false)?;
        let cache=self.cache.as_mut().unwrap();
        let Some(matches)=cache.presented_matches_expected() else {return Ok(());};
        let valid=cache.submission_valid.as_ref().is_none_or(|valid|valid.load(std::sync::atomic::Ordering::Acquire));
        if matches && valid {return Ok(());}
        if self.view().is_some() && (held || (changed && packet.time_seconds>=self.refreshed
            && packet.time_seconds-self.refreshed<0.05)) {return Ok(());}
        let cache = self.cache.as_mut().unwrap();
        let write = crate::submission::CacheWrite::new();
        let mut scene = r.scene.take().unwrap();
        let scene_write = scene.begin_write(r,encoder)?;
        let mut commands = scene.scale_commands.take().unwrap_or_else(|| RegionRecords::new(r));
        commands.begin();
        let _trace = crate::performance_trace::Span::new(c"capy.navigator_refresh");
        r.telemetry.phase_begin(13, &r.device, &r.queue, encoder);
        let result=(|| {
            cache.render_graph(&mut scene,r,packet,cache.plan.bounds,
                &mut Encoding {encoder,commands:&mut commands, retained:[0;2]},Destination::Navigator,None,0)?;
            Ok(())
        })();
        r.telemetry.phase_end(13, encoder);
        scene.scale_commands = Some(commands);
        r.scene = Some(scene);
        match result {
            Err(GpuRasterError::NotReady|GpuRasterError::SourceWorkingSetExceeded)=> {
                scene_write.track(encoder);
                return Ok(());
            },
            result=>result?,
        }
        if cache.presented_matches_expected()!=Some(true) {
            scene_write.track(encoder);
            return Ok(());
        }
        cache.submission_valid = Some(write.validity());
        write.track(encoder);
        scene_write.track(encoder);
        self.refreshed = packet.time_seconds;
        self.revision = self.revision.wrapping_add(1);
        crate::performance_trace::counter(c"Capy Navigator refreshes", self.revision);
        crate::performance_trace::counter(c"Capy Navigator pixels", cache.plan.size.map(u64::from).into_iter().product());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{float_pixels, packet, submit_settled};
    use layer_core::{Affine, Document, ImageTransform, Point};

    fn document() -> Document {
        let mut doc = Document::new(layer_core::authored::PortableId::random(), 1025, 513, layer_core::DocumentNames { paint: "Current ink".into(), paper: "Paper".into() });
        let SourceTarget::Paint(paint) = doc.working.target.unwrap() else { unreachable!() };
        doc.artwork.paint.get_mut(paint).unwrap().base = Some(layer_core::authored::PaintBase::new((layer_core::color::source::rgba8_source([1025, 513], |x, y|
            [(x / 5) as u8, (y / 3) as u8, 40, 255])).into()));
        doc
    }
    fn pixels(r: &WgpuRasterizer) -> Vec<[f32; 4]> {
        float_pixels(r, r.navigator.cache.as_ref().unwrap().texture())
    }
    fn pose(r: &mut WgpuRasterizer, doc: &Document, x: f32) {
        r.set_transform_preview(Some(&layer_render::TransformPreview {
            transaction: 1, target: doc.working.target.unwrap(), moving: true, selection: None,
            transform: ImageTransform::affine(Affine::translation(Point { x, y: 0. })),
        })).unwrap();
    }

    #[test]
    fn navigator_coalesces_poses_and_flushes_without_further_input() {
        let doc = document();
        let mut r = WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let mut frame = packet(doc.scene(), doc.composition().size);
        frame.composite_all = false;
        frame.view.document_to_surface = [0.25, 0., 0., 0.25, 0., 0.];
        submit_settled(&mut r, frame);
        let original = pixels(&r);
        let revision = r.navigator.revision;
        for step in 1..=4 {
            pose(&mut r, &doc, step as f32 * 20.);
            frame.time_seconds = step as f32 * 0.01;
            crate::test_support::submit_ready(&mut r, frame).unwrap();
            assert_eq!(r.navigator.revision, revision);
            assert!(r.navigator.pending() && r.has_pending_work());
        }
        assert_eq!(pixels(&r), original);
        frame.time_seconds = 0.06;
        pose(&mut r, &doc, 100.);
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        assert_eq!(r.navigator.revision, revision + 1);
        assert_ne!(pixels(&r), original);
        assert!(!r.navigator.pending());
        frame.time_seconds = 0.07;
        pose(&mut r, &doc, 120.);
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        assert!(r.has_pending_work());
        frame.time_seconds = 0.071;
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        assert_eq!(r.navigator.revision, revision + 2);
        assert!(!r.navigator.pending());
        let settled = pixels(&r);
        for step in 1..=3 {
            frame.time_seconds += 0.1;
            frame.view.document_to_surface[4] = step as f32 * -80.;
            crate::test_support::submit_ready(&mut r, frame).unwrap();
            assert_eq!(r.navigator.revision, revision + 2);
            assert_eq!(pixels(&r), settled);
        }
        for step in 1..=20 {
            frame.time_seconds += 0.06;
            pose(&mut r, &doc, 120. + step as f32);
            crate::test_support::submit_ready(&mut r, frame).unwrap();
            assert_eq!(r.navigator.revision, revision + 2 + step);
        }
        r.prepare_moving_pixels(None);
        r.set_transform_preview(None).unwrap();
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        submit_settled(&mut r, frame);
        assert_eq!(pixels(&r), original);
    }

    #[test]
    fn navigator_history_and_blending_refresh_before_native_refinement() {
        let doc = document();
        let mut r = WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let mut frame = packet(doc.scene(), doc.composition().size);
        frame.composite_all = false;
        frame.view.document_to_surface = [0.25, 0., 0., 0.25, 0., 0.];
        submit_settled(&mut r, frame);
        let original = pixels(&r);
        let original_format=r.navigator.cache.as_ref().unwrap().texture().format();
        let mut edited = doc.clone();
        edited.artwork.occurrences.get_mut(doc.working.occurrence.unwrap()).unwrap().opacity = 0.3;
        let mut alternate = None;
        for (step, document) in [&edited, &doc, &edited, &doc].into_iter().enumerate() {
            frame.time_seconds += 0.01;
            let edit = FramePacket { scene: document.scene(), ..frame };
            let previous = pixels(&r);
            r.submit(edit).unwrap();
            assert!(r.navigator.pending());
            r.background_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
            r.submit(edit).unwrap();
            assert!(pixels(&r) != previous, "logical edits must reach Navigator while native refinement is held");
            r.background_ready.store(true, std::sync::atomic::Ordering::Release);
            submit_settled(&mut r, edit);
            assert!(!r.navigator.pending());
            let actual = pixels(&r);
            if step == 0 { assert_ne!(actual, original); alternate = Some((actual,r.navigator.cache.as_ref().unwrap().texture().format())); }
            else {
                let (expected,expected_format)=if step%2==0 {alternate.clone().unwrap()}else {(original.clone(),original_format)};
                let format=r.navigator.cache.as_ref().unwrap().texture().format();
                assert_eq!((format,expected_format),(wgpu::TextureFormat::Rgba32Float,wgpu::TextureFormat::Rgba32Float));
                assert_eq!(actual.len(),expected.len());
                let width=r.navigator.cache.as_ref().unwrap().plan.size[0];
                let mut worst=(0u32,[0u32;2],0usize,0f32,0f32);
                for (index,(actual,expected)) in actual.iter().zip(&expected).enumerate() {
                    for (channel,(&actual,&expected)) in actual.iter().zip(expected).enumerate() {
                        assert!(actual.is_finite() && expected.is_finite(),"nonfinite Display comparison: formats={format:?}/{expected_format:?}, pixel={index}, channel={channel}, actual={actual}, expected={expected}");
                        let ordered=|value:f32| {let bits=value.to_bits();if bits&0x8000_0000!=0 {!bits}else {bits|0x8000_0000}};
                        let distance=if actual==expected {0}else {ordered(actual).abs_diff(ordered(expected))};
                        if distance>worst.0 {worst=(distance,[index as u32%width,index as u32/width],channel,actual,expected);}
                    }
                }
                assert!(worst.0<=1,"repeated Display comparison exceeds one stored-format ULP: formats={format:?}/{expected_format:?}, worst(ulp,xy,channel,actual,expected)={worst:?}");
            }
        }
        frame.blend_space = layer_core::BlendSpace::Perceptual;
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        submit_settled(&mut r, frame);
        assert!(!r.navigator.pending());
        assert_ne!(pixels(&r), original);
    }

    #[test]
    fn navigator_tracks_temporary_brush_tails_and_their_removal() {
        let doc = document();
        let mut r = WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let mut frame = packet(doc.scene(), doc.composition().size);
        frame.composite_all = false;
        frame.view.document_to_surface = [0.25, 0., 0., 0.25, 0., 0.];
        submit_settled(&mut r, frame);
        let original = pixels(&r);
        let mut dab = crate::tests::test_dab([100., 100.], [0.9, 0.02, 0.1, 1.], 1.);
        dab.radii = [45.; 2];
        for (time, x) in [(0.01, 100.), (0.03, 150.), (0.06, 200.)] {
            dab.center.x = x;
            let batch = DabBatch { kind: DabBatchKind::Preview, ..crate::test_support::dab_batch(
                doc.working.target.unwrap(), crate::layer_tests::preset_style(layer_core::DefaultBrushPreset::GPen), dab.bounds()) };
            frame.time_seconds = time;
            crate::test_support::submit_ready(&mut r, FramePacket { dabs: &[dab], dab_batches: &[batch], ..frame }).unwrap();
        }
        assert_ne!(pixels(&r), original);
        frame.time_seconds = 0.07;
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        assert!(r.navigator.pending());
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        submit_settled(&mut r, frame);
        assert!(!r.navigator.pending());
        assert_eq!(pixels(&r), original);
    }

    #[test]
    fn navigator_replacement_and_discarded_submission_never_publish_old_pixels() {
        let doc = document();
        let mut r = WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let validation=r.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut frame = packet(doc.scene(), doc.composition().size);
        frame.composite_all = false;
        frame.view.document_to_surface = [0.25, 0., 0., 0.25, 0., 0.];
        submit_settled(&mut r, frame);
        let original = pixels(&r);
        let mut discarded_doc=doc.clone();
        discarded_doc.artwork.occurrences.get_mut(doc.working.occurrence.unwrap()).unwrap().opacity=0.3;
        let discarded_frame=FramePacket {time_seconds:0.1,scene:discarded_doc.scene(),..frame};
        crate::scene::scale::tests::prepare_fixture_frame(&r,discarded_frame,r.scene.as_ref().unwrap());
        let mut navigator = std::mem::take(&mut r.navigator);
        navigator.prepare(&r,discarded_frame,r.scene.as_ref().unwrap(),r.scale_display.as_ref().unwrap().pages.clone(),false).unwrap();
        let revision=navigator.revision;
        let receipt;
        {
            let mut encoder = crate::submission::CommandEncoder::new(&r.device, &Default::default());
            let before=crate::test_support::PassCounts::snapshot();
            crate::test_support::query_ready(&mut r,&mut encoder,|r,encoder| {
                navigator.refresh(r,discarded_frame,true,false,encoder)?;
                if navigator.revision==revision {Err(GpuRasterError::NotReady)}else {Ok(())}
            });
            receipt=encoder.submission();
            assert!(crate::test_support::PassCounts::snapshot().delta(before).total>0,"the discarded publication must encode real GPU work");
            assert!(navigator.revision>revision,"the authored change must create a new publication");
            let cache=navigator.cache.as_ref().unwrap();
            assert_eq!(crate::test_support::published_coverage(crate::test_support::coverage_hash(&cache.geometry)).unwrap().receipt,receipt.observation_id());
            assert!(!receipt.discarded());
        }
        assert!(receipt.discarded(),"the actual publication receipt must remain discarded");
        assert!(navigator.pending());
        assert!(navigator.view().is_none());
        r.navigator = navigator;
        crate::test_support::submit_ready(&mut r, frame).unwrap();
        submit_settled(&mut r, frame);
        assert!(!r.navigator.pending());
        assert_eq!(pixels(&r), original);
        let mut replacement_doc = doc.clone();
        let SourceTarget::Paint(paint) = replacement_doc.working.target.unwrap() else { unreachable!() };
        replacement_doc.artwork.paint.get_mut(paint).unwrap().base = Some(layer_core::authored::PaintBase::new((layer_core::color::source::rgba8_source([1025, 513], |_, _| [0, 0, 255, 255])).into()));
        let replacement = FramePacket { reset_layers: true, scene: replacement_doc.scene(), ..frame };
        submit_settled(&mut r, replacement);
        let changed = pixels(&r);
        assert!(changed.iter().all(|c| c[0] == 0. && c[1] == 0. && c[2] > 0.99 && c[3] == 1.));
        let mut recovered = WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        submit_settled(&mut recovered, replacement);
        assert_eq!(pixels(&recovered), changed);
        assert!(pollster::block_on(validation.pop()).is_none());
    }

    #[test]
    fn navigator_checks_selected_signatures_without_an_invalidation_hint() {
        let mut doc=freshness_document();let extent=doc.composition().size;
        let owner=doc.working.occurrence.unwrap();let SourceTarget::Paint(paint)=doc.working.target.unwrap() else {unreachable!()};
        doc.artwork.paint.get_mut(paint).unwrap().base=Some(layer_core::authored::PaintBase::new(
            layer_core::color::source::rgba8_source(extent,|_,_|[180,70,30,255]).into()));
        doc.artwork.occurrences.get_mut(owner).unwrap().opacity=0.5;
        let mut r=WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let frame=FramePacket {blend_space:doc.composition().blend,composite_all:false,..packet(doc.scene(),extent)};
        submit_settled(&mut r,frame);let previous=pixels(&r);let revision=r.navigator.revision;
        let mut changed=doc.clone();changed.artwork.occurrences.get_mut(owner).unwrap().opacity=0.25;
        let edit=FramePacket {time_seconds:0.1,scene:changed.scene(),..frame};
        let scene=r.scene.take().unwrap();scene.prepare_signatures(&r,edit).unwrap();r.scene=Some(scene);
        let mut navigator=std::mem::take(&mut r.navigator);
        let mut encoder=crate::submission::CommandEncoder::new(&r.device,&Default::default());
        crate::test_support::query_ready(&mut r,&mut encoder,|r,encoder| {
            navigator.refresh(r,edit,false,false,encoder)?;
            if navigator.pending() {Err(GpuRasterError::NotReady)}else {Ok(())}
        });
        r.uploads.finish(&encoder);encoder.submit(&r.queue);crate::test_support::complete(&r);
        r.navigator=navigator;
        assert!(r.navigator.revision>revision,"a changed selected signature must publish without an invalidation hint");
        assert_ne!(pixels(&r),previous,"the changed opacity must reach actual Navigator pixels");
        let (error,first)=freshness_errors(&r,&changed);
        assert!(error[2]<2e-5,"Navigator must match the selected contents without an invalidation hint: {error:?}, first={first:?}");
    }

    fn freshness_document()->Document {
        let extent=[1536,768];
        let mut doc=Document::new(layer_core::authored::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Current ink".into(),paper:"Paper".into()});
        let SourceTarget::Paint(paint)=doc.working.target.unwrap() else {unreachable!()};
        doc.artwork.paint.get_mut(paint).unwrap().base=Some(layer_core::authored::PaintBase::new(
            layer_core::color::source::rgba8_source(extent,|x,y|[(x/5) as u8,(y/3) as u8,40,255]).into()));
        let root=doc.artwork.root;
        let composition=doc.artwork.compositions.get_mut(root).unwrap();
        composition.color.depth=layer_core::color::SampleDepth::F32;
        composition.blend=layer_core::BlendSpace::Linear;doc
    }

    fn freshness_reference(doc:&Document)->Vec<[f32;4]> {
        let reference=freshness_reference_renderer(doc);
        float_pixels(&reference,crate::test_support::document_texture(&reference))
    }

    fn freshness_reference_renderer(doc:&Document)->WgpuRasterizer {
        let mut reference=WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        reference.test.reference=true;
        submit_settled(&mut reference,FramePacket {blend_space:doc.composition().blend,
            ..packet(doc.scene(),doc.composition().size)});
        crate::test_support::complete(&reference);reference
    }

    fn freshness_difference(actual:&[[f32;4]],expected:&[[f32;4]],plan:display_mips::Plan,format:wgpu::TextureFormat)
        ->Option<([u32;2],[f32;4],[f32;4])> {
        freshness_mismatches(actual,expected,plan,format).0
    }

    fn freshness_mismatches(actual:&[[f32;4]],expected:&[[f32;4]],plan:display_mips::Plan,format:wgpu::TextureFormat)
        ->(Option<([u32;2],[f32;4],[f32;4])>,[usize;2]) {
        let side=1<<plan.level;
        let mut first=None;let mut halves=[0;2];
        for y in 0..plan.size[1] {for x in 0..plan.size[0] {
            let mut sum=[0.;4];let mut count=0.;
            for yy in plan.bounds.min_y()+y*side..(plan.bounds.min_y()+(y+1)*side).min(plan.bounds.max_y()) {
                for xx in plan.bounds.min_x()+x*side..(plan.bounds.min_x()+(x+1)*side).min(plan.bounds.max_x()) {
                    for c in 0..4 {sum[c]+=expected[(yy*plan.extent[0]+xx) as usize][c];}count+=1.;
                }
            }
            let expected=sum.map(|value|value/count);
            let expected=match format {wgpu::TextureFormat::Rgba16Float=>expected.map(|value|layer_core::color::f16::from_f32(value).to_f32()),
                wgpu::TextureFormat::Rgba32Float=>expected,format=>panic!("unexpected actual format: {format:?}")};
            let actual=actual[(y*plan.size[0]+x) as usize];
            if actual.iter().zip(expected).any(|(a,b)|(*a-b).abs()>=2e-5) {
                first.get_or_insert(([x,y],actual,expected));
                halves[usize::from(plan.bounds.min_x()+x*side>=4096)]+=1;
            }
        }}
        (first,halves)
    }

    fn freshness_errors(r:&WgpuRasterizer,doc:&Document)->([f32;3],Option<([u32;2],[f32;4],[f32;4])>) {
        let expected=freshness_reference(doc);
        let cache=r.navigator.cache.as_ref().expect("Navigator must publish an image");
        let actual=pixels(r);let format=cache.texture().format();
        (crate::scene::scale::tests::quality(&actual,&expected,cache.plan,format),freshness_difference(&actual,&expected,cache.plan,format))
    }

    fn settle_freshness_engine(engine:&mut layer_engine::CanvasEngine<WgpuRasterizer>) {
        let deadline=std::time::Instant::now()+crate::READBACK_TIMEOUT;
        loop {
            engine.render_frame().unwrap();crate::test_support::complete(engine.backend());
            if !engine.has_pending_input() && !engine.has_pending_document_edits() && !engine.backend().has_pending_work() {break;}
            assert!(std::time::Instant::now()<deadline,"Navigator/history work must settle under its existing deadline");
            std::thread::yield_now();
        }
    }

    #[test]
    fn navigator_freshness_matches_selected_stroke_history_and_layer_moves() {
        use layer_engine::{CanvasEngine,PenPhase,SampleFlags,ViewTransform,input_queue};
        let doc=freshness_document();let extent=doc.composition().size;
        let r=WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let (mut input,consumer)=input_queue(64);
        let view=layer_render::ViewState {width_px:512,height_px:256,document_to_surface:[0.5,0.,0.,0.5,0.,0.]};
        let mut engine=CanvasEngine::new(r,doc,consumer,view,ViewTransform {revision:1,surface_to_document:[2.,0.,0.,2.,0.,0.]}).unwrap();
        engine.set_brush(layer_core::BrushSnapshot {diameter:96.,mappings:Arc::from([]),
            ..layer_core::default_brush(layer_core::DefaultBrushPreset::GPen)}).unwrap();
        let mut failures=Vec::new();
        let check=|label:&str,engine:&CanvasEngine<WgpuRasterizer>,failures:&mut Vec<String>| {
            let (error,first)=freshness_errors(engine.backend(),engine.document());
            if error[2]>=2e-5 {failures.push(format!("{label}: Navigator/fresh-selected-native error={error:?}, first(xy,actual,expected)={first:?}"));}
        };
        settle_freshness_engine(&mut engine);check("open",&engine,&mut failures);
        for (index,phase) in [PenPhase::Down,PenPhase::Move,PenPhase::Up].into_iter().enumerate() {
            let mut event=crate::test_support::pen((index+1) as u64,phase,[72.+index as f32*28.,64.],SampleFlags::PRIMARY);
            event.view_revision=1;input.push(event).unwrap();
            engine.render_frame().unwrap();crate::test_support::complete(engine.backend());
        }
        settle_freshness_engine(&mut engine);check("stroke",&engine,&mut failures);
        assert!(engine.undo().unwrap());settle_freshness_engine(&mut engine);check("undo stroke",&engine,&mut failures);
        assert!(engine.redo().unwrap());settle_freshness_engine(&mut engine);check("redo stroke",&engine,&mut failures);
        let owner=engine.document().working.occurrence.unwrap();
        let edit=engine.document().move_layers_edit(&[owner],[128,64]).unwrap();
        engine.apply_edit(edit).unwrap();settle_freshness_engine(&mut engine);check("move layer",&engine,&mut failures);
        assert!(engine.undo().unwrap());settle_freshness_engine(&mut engine);check("undo layer move",&engine,&mut failures);
        assert_eq!(engine.document().composition().size,extent);
        assert!(failures.is_empty(),"{}",failures.join("\n"));
    }

    #[test]
    fn navigator_freshness_keeps_an_edit_owed_across_the_refresh_throttle() {
        let doc=freshness_document();let extent=doc.composition().size;
        let mut r=WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let frame=FramePacket {time_seconds:0.1,composite_all:false,blend_space:doc.composition().blend,
            ..packet(doc.scene(),extent)};
        submit_settled(&mut r,frame);let before=pixels(&r);let revision=r.navigator.revision;
        let mut changed=doc.clone();changed.artwork.occurrences.get_mut(doc.working.occurrence.unwrap()).unwrap().opacity=0.3;
        let edit=FramePacket {time_seconds:0.11,scene:changed.scene(),..frame};
        crate::test_support::submit_ready(&mut r,edit).unwrap();crate::test_support::complete(&r);
        assert_eq!(r.navigator.revision,revision,"the real50ms throttle must be exercised");
        assert_eq!(pixels(&r),before);
        submit_settled(&mut r,FramePacket {time_seconds:0.16,..edit});
        let (error,first)=freshness_errors(&r,&changed);
        assert!(error[2]<2e-5,"a throttled edit must remain owed until matching current pixels publish: {error:?}, first={first:?}");
        assert_ne!(pixels(&r),before,"the edit must change actual Navigator pixels");
    }

    #[test]
    fn navigator_freshness_covers_the_far_half_of_an_8192_pixel_drawing() {
        let extent=[8192,768];let mut doc=freshness_document();
        let mut artwork=layer_core::authored::Artwork::new(extent).unwrap();
        let (owner,target)=crate::test_support::add_paint(&mut artwork,"Far-half ink",extent);
        let SourceTarget::Paint(paint)=target else {unreachable!()};
        artwork.paint.get_mut(paint).unwrap().base=Some(layer_core::authored::PaintBase::new(
            layer_core::color::source::rgba8_source(extent,|x,y|if x>=4096 {[231,(y%127) as u8,19,255]}else {[11,31,97,255]}).into()));
        let root=artwork.root;artwork.compositions.get_mut(root).unwrap().color=doc.composition().color;
        artwork.compositions.get_mut(root).unwrap().blend=layer_core::BlendSpace::Linear;
        let stack=artwork.compositions.get(root).unwrap().result;artwork.stacks.get_mut(stack).unwrap().entries=vec![owner];
        doc=Document::from_artwork(artwork).unwrap();
        let mut r=WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        submit_settled(&mut r,FramePacket {blend_space:doc.composition().blend,..packet(doc.scene(),extent)});
        let expected=freshness_reference(&doc);
        let display=r.scale_display.as_ref().unwrap();
        let overview=display.overview.as_ref().expect("the wide drawing must exercise its separate global overview");
        for (label,cache) in [("main",display),("overview",overview.as_ref())] {
            let actual=float_pixels(&r,cache.texture());
            let error=crate::scene::scale::tests::quality(&actual,&expected,cache.plan,cache.texture().format());
            assert!(error[2]<2e-5,"8192-wide {label} must cover matching pixels at document x>=4096: {error:?}, first={:?}",freshness_difference(&actual,&expected,cache.plan,cache.texture().format()));
        }
        let mut presenter=crate::test_support::float_presenter(&r);
        let (texture,target)=create_color_target(&r.device,extent,"wide root publication oracle");
        presenter.present(&r,&target,crate::test_support::view(extent),[0.;4]).unwrap();
        let shown=float_pixels(&r,&texture);
        let reference=freshness_reference_renderer(&doc);
        let mut reference_presenter=crate::test_support::float_presenter(&reference);
        let (reference_texture,reference_target)=create_color_target(&reference.device,extent,"wide fresh presented reference");
        reference_presenter.present(&reference,&reference_target,crate::test_support::view(extent),[0.;4]).unwrap();
        let presented_expected=float_pixels(&reference,&reference_texture);
        assert!(crate::test_support::max_error(&shown,&presented_expected)<2e-5,"presented root must cover the far half of the8192-wide drawing, first={:?}",freshness_difference(&shown,&presented_expected,display_mips::Plan::at(extent,0),texture.format()));
        let cache=r.navigator.cache.as_ref().unwrap();assert_eq!(cache.plan.extent,extent);
        let actual=pixels(&r);let right=cache.plan.size[0]/2;
        assert!((0..cache.plan.size[1]).all(|y|(right..cache.plan.size[0]).all(|x|actual[(y*cache.plan.size[0]+x) as usize][0]>0.5)),
            "Navigator must cover document x>=4096, not leave the far half empty or stale");
        let error=crate::scene::scale::tests::quality(&actual,&expected,cache.plan,cache.texture().format());
        let (first,halves)=freshness_mismatches(&actual,&expected,cache.plan,cache.texture().format());
        let first_document=first.map(|(at,_,_)|[cache.plan.bounds.min_x()+(at[0]<<cache.plan.level),cache.plan.bounds.min_y()+(at[1]<<cache.plan.level)]);
        assert!(error[2]<2e-5,"8192-wide Navigator must match selected contents over its whole coverage: {error:?}, first(navigator_xy,actual,expected)={first:?}, first_document_xy={first_document:?}, mismatching_pixels(x<4096,x>=4096)={halves:?}");
    }

    #[test]
    fn a_root_window_keeps_its_previous_revision_until_every_visible_page_matches() {
        let extent=[512,256];
        let mut doc=Document::new(layer_core::authored::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Revision window".into(),paper:"Paper".into()});
        let target=doc.working.target.unwrap();let owner=doc.working.occurrence.unwrap();
        let SourceTarget::Paint(paint)=target else {unreachable!()};
        let root=doc.artwork.root;
        let composition=doc.artwork.compositions.get_mut(root).unwrap();
        composition.color.depth=layer_core::color::SampleDepth::F32;
        composition.blend=layer_core::BlendSpace::Linear;
        let stack=composition.result;doc.artwork.stacks.get_mut(stack).unwrap().entries=vec![owner];
        doc.artwork.paint.get_mut(paint).unwrap().base=Some(layer_core::authored::PaintBase::new(
            layer_core::color::source::rgba8_source(extent,|_,_|[11,31,197,255]).into()));
        let mut r=WgpuRasterizer::new_native_headless(doc.composition().color).unwrap();
        let frame=FramePacket {blend_space:doc.composition().blend,composite_all:false,..packet(doc.scene(),extent)};
        submit_settled(&mut r,frame);crate::test_support::complete(&r);
        let mut presenter=crate::test_support::float_presenter(&r);
        let (output,view)=create_color_target(&r.device,extent,"whole-revision root publication");
        presenter.present(&r,&view,frame.view,[0.;4]).unwrap();let previous=float_pixels(&r,&output);
        let cache=r.scale_display.as_ref().unwrap();
        let receipt=crate::test_support::published_coverage(crate::test_support::coverage_hash(&cache.geometry)).unwrap().receipt;
        let replacement=layer_core::color::source::rgba8_source(extent,|_,_|[231,19,47,255]);
        let mut encoder=crate::submission::CommandEncoder::new(&r.device,&Default::default());
        let pixels=crate::prepare_image_tile(&replacement,[0;2],r.device.working_space(),&r.device.source_samples,None).unwrap();
        let tile=Scene::decode_prepared_for(&mut r,&mut encoder,&replacement,[0;2],pixels,crate::scene::sources::SourceUse::Exact).unwrap();
        Scene::promote_source(&mut r,&mut encoder,tile).unwrap();
        r.uploads.finish(&encoder);encoder.submit(&r.queue);crate::test_support::complete(&r);
        let store=crate::test_support::GatedStore::new(&layer_core::raster_storage::RetainedTiles::from_sources([replacement.as_ref()]),false);
        let mut changed=doc.clone();changed.artwork.paint.get_mut(paint).unwrap().base=Some(layer_core::authored::PaintBase::new(replacement.into()));
        let next=FramePacket {scene:changed.scene(),..frame};
        crate::test_support::submit_ready(&mut r,next).unwrap();crate::test_support::complete(&r);store.gate.entered();
        for index in 0..=2 {
            if index!=0 {crate::test_support::submit_ready(&mut r,next).unwrap();crate::test_support::complete(&r);}
            let cache=r.scale_display.as_ref().unwrap();let coverage=cache.coverage_snapshot();
            assert!(coverage.ready(PixelRect::new(0,0,256,256),pages::Accuracy::Display),"the fixture must commit one real page of the new revision");
            assert!(!coverage.ready(PixelRect::new(256,0,512,256),pages::Accuracy::Display),"the gated page must remain owed");
            presenter.present(&r,&view,frame.view,[0.;4]).unwrap();
            assert!(crate::test_support::max_error(&float_pixels(&r,&output),&previous)<2e-5,
                "an authored root window must stay wholly previous while any visible page of the requested revision is missing, first={:?}",freshness_difference(&float_pixels(&r,&output),&previous,display_mips::Plan::at(extent,0),output.format()));
            let published=crate::test_support::published_coverage(crate::test_support::coverage_hash(&cache.geometry)).unwrap();
            assert_eq!(published.receipt,receipt,"a partial new revision cannot replace the root publication witness");
        }
        store.gate.release();submit_settled(&mut r,next);crate::test_support::complete(&r);
        let cache=r.scale_display.as_ref().unwrap();assert!(cache.coverage_snapshot().ready(PixelRect::full(extent),pages::Accuracy::Display));
        let published=crate::test_support::published_coverage(crate::test_support::coverage_hash(&cache.geometry)).unwrap();assert_ne!(published.receipt,receipt);
        presenter.present(&r,&view,frame.view,[0.;4]).unwrap();let actual=float_pixels(&r,&output);
        let reference=freshness_reference_renderer(&changed);
        let raw=float_pixels(&r,crate::test_support::document_texture(&r));
        let raw_expected=float_pixels(&reference,crate::test_support::document_texture(&reference));
        let raw_error=crate::test_support::max_error(&raw,&raw_expected);
        assert!(raw_error<2e-5,"complete raw root differs from fresh raw reference: max={raw_error}, first={:?}",
            freshness_difference(&raw,&raw_expected,display_mips::Plan::at(extent,0),crate::test_support::document_texture(&r).format()));
        let mut reference_presenter=crate::test_support::float_presenter(&reference);
        let (reference_texture,reference_target)=create_color_target(&reference.device,extent,"whole-revision fresh presented reference");
        reference_presenter.present(&reference,&reference_target,frame.view,[0.;4]).unwrap();
        let expected=float_pixels(&reference,&reference_texture);
        let error=crate::test_support::max_error(&actual,&expected);
        assert!(error<2e-5,"complete presented root differs from fresh presented reference: max={error}, first={:?}",
            freshness_difference(&actual,&expected,display_mips::Plan::at(extent,0),output.format()));
        assert!(crate::test_support::max_error(&actual,&previous)>0.5,"the complete new revision must really replace the previous picture");
    }
}
