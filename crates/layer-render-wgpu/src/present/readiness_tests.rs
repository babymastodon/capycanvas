use super::*;

fn picker() -> layer_render::ColorPickerOverlay {
    layer_render::ColorPickerOverlay {sample_color_only: false, center: [32.; 2], sample: [32.; 2], scale: 1., classic: false,
        layer: false, original: [0., 0., 0., 1.], candidate: [1.; 4]}
}

#[test]
fn first_ready_frame_requires_only_its_viewport_variant() {
    let extent = [64; 2];
    let document = crate::scene::scale::tests::document_at(extent);
    let mut r = WgpuRasterizer::new_native_headless(document.composition().color).unwrap();
    let all = layer_core::SceneScope::All;
    crate::test_support::submit_settled(&mut r, crate::test_support::packet(document.scene().with_scope(&all), extent));
    assert!(r.presentation_page().is_some(), "the first ready frame must have a committed keyed projection");
    assert!(r.selected_display_cache().unwrap().presentable_pages() > 0);
    let batch = r.compilation.batch();
    let mut presenter = ViewportPresenter::for_surface(&r, wgpu::TextureFormat::Rgba8Unorm, SdrSurfaceColor::Srgb).unwrap();
    let unused_required = presenter.pipeline[1..].iter().any(|pipeline|
        crate::compile::tests::requested(&r.compilation, pipeline, crate::startup::DOCUMENT));
    pollster::block_on(r.compilation.prepare_all([&presenter.pipeline[0], &presenter.surround_pipeline,
        &presenter.cursor_pipeline, &presenter.recipes.transition.pipeline])).unwrap();
    let ready = presenter.pipelines_ready(&r);
    let (_, target) = crate::create_target(r.device(), extent, wgpu::TextureFormat::Rgba8Unorm, "first viewport variant");
    let mut encoder = r.device().create_command_encoder(&Default::default());
    presenter.encode(&r, &mut encoder, &target, crate::test_support::view(extent), [0.; 4]).unwrap();
    let encoded = presenter.history.valid;
    let anticipated = presenter.pipeline[1..].iter().all(|pipeline|
        crate::compile::tests::requested(&r.compilation, pipeline, crate::startup::VALIDATION)
            && !crate::compile::tests::requested(&r.compilation, pipeline, crate::startup::DOCUMENT));
    drop(encoder);
    drop(presenter);
    drop(batch);
    drop(r);
    crate::finish_shader_compiler_shutdown();
    assert!(!unused_required, "unused viewport variants do not enter the first frame gate");
    assert!(ready && encoded, "the ready source draws while the other variants remain cold");
    assert!(anticipated, "after the first ready frame, remaining variants run without idle admission");
}

#[test]
fn presenter_creation_declares_each_output_before_first_presentation() {
    let r = WgpuRasterizer::new_native_headless(Default::default()).unwrap();
    let batch = r.compilation.batch();
    for format in [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Rgba16Float] {
        let before = r.compilation.pending();
        let presenter = ViewportPresenter::for_surface(&r, format, SdrSurfaceColor::Srgb).unwrap();
        assert!(r.compilation.pending() > before, "{format:?}: creation must declare its recipes");
        assert!(presenter.pipeline.iter().all(|pipeline| !pipeline.ready()));
        let queued = r.compilation.pending();
        for _ in 0..20 { assert!(!presenter.pipelines_ready(&r)); }
        assert_eq!(r.compilation.pending(), queued, "repeated declarations share one queue entry per recipe");
    }
    drop(batch);
}

#[test]
fn unused_picker_is_prepared_after_construction_when_idle() {
    let r = WgpuRasterizer::new_native_headless(Default::default()).unwrap();
    let batch = r.compilation.batch();
    let presenter = ViewportPresenter::for_surface(&r, wgpu::TextureFormat::Rgba8Unorm, SdrSurfaceColor::Srgb).unwrap();
    assert!(!presenter.picker.pipeline.ready());
    assert!(presenter.picker.bounds().is_none());
    r.shader_idle(true, true);
    drop(batch);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !presenter.picker.pipeline.ready() {
        r.compilation.check().unwrap();
        assert!(std::time::Instant::now() < deadline, "constructor-declared optional picker must run while idle");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(presenter.picker.bounds().is_none());
}

#[test]
fn hidden_overview_and_picker_stay_optional_while_visible_navigator_is_required() {
    let mut r = WgpuRasterizer::new_native_headless(Default::default()).unwrap();
    let _batch = r.compilation.batch();
    let mut presenter = ViewportPresenter::for_surface(&r, wgpu::TextureFormat::Rgba8Unorm, SdrSurfaceColor::Srgb).unwrap();
    presenter.prepare_overviews(&r);
    presenter.set_color_picker(Some(picker()));
    pollster::block_on(r.compilation.prepare_all(presenter.pipeline.iter()
        .chain([&presenter.surround_pipeline, &presenter.cursor_pipeline, &presenter.recipes.transition.pipeline]))).unwrap();
    assert!(!presenter.overview_pipeline.as_ref().unwrap().ready());
    assert!(!presenter.picker.pipeline.ready());
    assert!(presenter.pipelines_ready(&r), "hidden overview and picker must not hold the canvas gate");
    let queued = crate::compile::tests::queued_recipes(&r.compilation);
    for _ in 0..20 { assert!(presenter.pipelines_ready(&r)); }
    assert_eq!(crate::compile::tests::queued_recipes(&r.compilation), queued);
    r.presentation.navigator = true;
    assert!(presenter.pipelines_ready(&r), "an available image without a visible slot is not a visible Navigator");
    presenter.set_overviews(&r, &[crate::OverviewPlacement {bounds: [0., 0., 32., 32.], clip: None, work_area: [[0.; 2]; 4],
        outline_linear: [0.; 3], scale: 1., opacity: 1.}]);
    r.presentation.navigator = false;
    assert!(!presenter.pipelines_ready(&r), "a visible slot must prepare before its first image arrives");
    r.presentation.navigator = true;
    assert!(!presenter.pipelines_ready(&r), "a visible Navigator must be ready at the canvas gate");
    pollster::block_on(r.compilation.prepare_all(presenter.overview_pipeline.iter())).unwrap();
    assert!(presenter.pipelines_ready(&r));
    assert!(!presenter.picker.pipeline.ready());
}

#[test]
fn standalone_navigator_declares_its_output_recipe_at_creation() {
    let r = WgpuRasterizer::new_native_headless(Default::default()).unwrap();
    let _batch = r.compilation.batch();
    let presenter = ViewportPresenter::for_overview_surface(&r, wgpu::TextureFormat::Rgba8Unorm, SdrSurfaceColor::Srgb).unwrap();
    assert_eq!(presenter.overview_pipeline.as_ref().unwrap().identity(), presenter.recipes.overviews[1].get().unwrap().identity());
    assert!(!presenter.pipelines_ready(&r));
    pollster::block_on(r.compilation.prepare_all(presenter.pipeline.iter()
        .chain([&presenter.surround_pipeline, &presenter.cursor_pipeline, &presenter.recipes.transition.pipeline])
        .chain(presenter.overview_pipeline.iter()))).unwrap();
    assert!(presenter.pipelines_ready(&r));
    assert!(!presenter.recipes.overviews[0].get().unwrap().ready());
}

#[test]
fn optional_picker_completion_repaints_without_parking_the_canvas() {
    let r = WgpuRasterizer::new_native_headless(Default::default()).unwrap();
    let _batch = r.compilation.batch();
    let mut presenter = ViewportPresenter::for_surface(&r, wgpu::TextureFormat::Rgba8Unorm, SdrSurfaceColor::Srgb).unwrap();
    presenter.set_target_retention(true);
    presenter.set_color_picker(Some(picker()));
    pollster::block_on(r.compilation.prepare_all(presenter.pipeline.iter()
        .chain([&presenter.surround_pipeline, &presenter.cursor_pipeline, &presenter.recipes.transition.pipeline]))).unwrap();
    let (_, target) = crate::create_target(&r.device, [64; 2], wgpu::TextureFormat::Rgba8Unorm, "optional picker");
    let view = crate::test_support::view([64; 2]);
    let mut encoder = r.device.create_command_encoder(&Default::default());
    presenter.encode(&r, &mut encoder, &target, view, [0.; 4]).unwrap();
    assert!(presenter.history.valid);
    assert!(!presenter.picker.pipeline.ready());
    assert!(!presenter.needs_present(&r, view, [0.; 4]), "a cold optional picker must allow an idle canvas");
    pollster::block_on(r.compilation.prepare_all(presenter.overview_pipeline.iter())).unwrap();
    assert!(!presenter.needs_present(&r, view, [0.; 4]), "a hidden overview does not invalidate presentation");
    pollster::block_on(r.compilation.prepare_all([&presenter.picker.pipeline])).unwrap();
    assert!(presenter.needs_present(&r, view, [0.; 4]), "the newly ready picker must repaint");
    presenter.encode(&r, &mut encoder, &target, view, [0.; 4]).unwrap();
    assert!(!presenter.needs_present(&r, view, [0.; 4]));
    r.queue.submit([encoder.finish()]);
}
