use super::*;
use layer_render::CanvasRenderer;

#[test]
fn first_rotate_binds_the_transform_recipe_declared_before_input() {
    for scale in [1., 0.5] {
        let extent = [128; 2];
        let reference = WgpuRasterizer::new_native_headless(Default::default()).unwrap();
        let mut renderer = crate::test_support::staged_renderer(&reference, Default::default());
        let mut document = crate::layer_tests::placement::paint_document(extent, "Rotate");
        crate::layer_tests::placement::set_source(&mut document,
            layer_core::color::source::rgba8_source(extent, |_, _| [127, 63, 191, 255]));
        let brush = layer_core::default_brush(layer_core::DefaultBrushPreset::GPen);
        renderer.prepare_startup(&document, &brush, false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        crate::test_support::wait_startup(&mut renderer, deadline, |progress| progress.brush_ready,
            format_args!("Transform startup timed out"));
        let declared: Vec<_> = renderer.transforms.as_ref().unwrap().pipelines()
            .iter().map(Deferred::identity).collect();
        let mut transform_needs = Needs::default();
        transform_needs.transform(&renderer);
        let declared_compute: Vec<_> = transform_needs.compute.iter().map(Deferred::identity).collect();
        let packet = crate::test_support::packet(document.scene(), extent);
        crate::test_support::submit_published(&mut renderer, packet);
        renderer.wait_idle().unwrap();
        renderer.prepare_context(&document, &brush, layer_render::PreparationContext::default());
        renderer.shader_idle(true, true);
        while compile::tests::queued_recipes(&renderer.context_compilation) != 0
            || compile::tests::queued_recipes(&renderer.startup.as_ref().unwrap().compiler) != 0 {
            renderer.poll_startup().unwrap();
            assert!(std::time::Instant::now() < deadline, "Transform idle warming timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        renderer.shader_idle(false, false);
        let first_use = renderer.compilation.batch();
        let preview = layer_render::TransformPreview {
            transaction: 1, moving: true,
            target: crate::layer_tests::placement::target(&document), selection: None,
            transform: layer_core::ImageTransform::affine(layer_core::Affine::around(
                layer_core::Point { x: 64., y: 64. }, [scale; 2], 0.3,
                layer_core::Point { x: 0., y: 0. })),
        };
        renderer.set_transform_preview(Some(&preview)).unwrap();
        assert!(renderer.raster_dependencies_ready(packet, Some(&preview)), "a warmed first rotation cannot defer");
        renderer.submit(packet).unwrap();
        let (key, pipeline) = renderer.transforms.as_ref().unwrap().bound_color_pipeline();
        assert_eq!(key, format!("Key {{ mapping: Affine, filter: {}, area: {} }}",
            layer_core::Interpolation::Linear as u8, scale < 1.));
        assert!(pipeline.ready());
        assert!(declared.contains(&pipeline.identity()),
            "first rotate bound {key}, recipe {}, absent from transform preparation {declared:?}", pipeline.identity());
        assert_eq!(compile::tests::queued_recipes(&renderer.compilation), 0);
        drop(first_use);
        renderer.wait_idle().unwrap();
        let plan = document.layer_transform_plan(preview.target, &preview.transform.placement,
            None, Default::default()).unwrap();
        let geometry = plan.plane_geometry(plan.target);
        assert!(!geometry.is_identity() && geometry.as_affine().is_some());
        assert_eq!(geometry.placement.interpolation, layer_core::Interpolation::Linear);
        assert_eq!(crate::pixel_transform::exact_taps(&geometry) > 1, scale < 1.);
        let placement = renderer.transforms.as_ref().unwrap().placement_pipeline(false, &geometry);
        assert!(placement.ready(), "first rotate Apply must not compile its color affine placement recipe synchronously");
        assert!(declared_compute.contains(&placement.identity()),
            "first rotate Apply recipe {} is absent from transform preparation {declared_compute:?}", placement.identity());
        let center = [64 - plan.origin[0], 64 - plan.origin[1]].map(|n| u32::try_from(n).unwrap());
        let output = pollster::block_on(renderer.snapshot_gpu().transform_pixels(plan, Default::default())).unwrap();
        document.apply(output).unwrap();
        let paint = crate::layer_tests::placement::paint(&document);
        assert!(paint.base.is_none() && paint.operations.is_empty());
        let data = paint.raster.wait_data().unwrap();
        let coordinate = center.map(|n| n / PAGE_SIZE);
        let tile = &data.tiles[&layer_core::raster::TileKey { plane: layer_core::raster::RasterPlane::Color, coordinate }];
        let pixels = tile.wait_backing().unwrap().decode_on_worker().unwrap();
        let offset = ((center[1] % PAGE_SIZE) * PAGE_SIZE + center[0] % PAGE_SIZE) as usize * 4;
        assert_eq!(&pixels[offset..offset + 4], &[127, 63, 191, 255]);
    }
}
