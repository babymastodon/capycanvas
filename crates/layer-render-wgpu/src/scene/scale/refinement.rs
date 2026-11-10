use super::*;

impl Cache {
    pub fn approximating(&self) -> bool { !self.output_complete() || self.preparing || self.overview.as_ref().is_some_and(|overview| overview.approximating()) }


    pub fn has_pending_work(&self, r: &WgpuRasterizer) -> bool {
        if r.moving_layer.is_some() || self.transform.as_ref().is_some_and(|preview| preview.moving) { return false; }
        self.pixels.committed_root().is_none() || self.coverage_snapshot().needs(self.plan.bounds,pages::Accuracy::Exact)
            || self.overview.as_ref().is_some_and(|overview|overview.has_pending_work(r))
    }

    pub(super) fn refine(
        &mut self, scene: &mut Scene, r: &mut WgpuRasterizer, packet: FramePacket<'_>,
        encoder: &mut crate::submission::CommandEncoder, commands: &mut RegionRecords,
    ) -> Result<PixelRect, GpuRasterError> {
        let page_limit=(scene.remaining_work(false)/object_cache::PASS_TAPS).max(1) as usize;
        let mut missing=Vec::new();
        for cache in std::iter::once(&*self).chain(self.overview.as_deref()) {
            if cache.graph.root.is_none() {continue;}
            for coordinate in pages::coordinates(cache.plan.bounds,cache.plan.level) {
                let key=cache.refinement_interest(self.evaluation,cache.plan.level,pages::bounds(coordinate,cache.plan.level,cache.plan.extent)).key;
                self.pages.lock().unwrap().refinement_frontier(&key,page_limit,&mut missing,0);
                if missing.len()>=page_limit {break;}
            }
            if missing.len()>=page_limit {break;}
        }
        let (reductions,native):(Vec<_>,Vec<_>)=missing.into_iter().partition(|key|key.level>0);
        let mut changed=PixelRect::EMPTY;
        if !reductions.is_empty() {
            for key in reductions {self.reduce_exact(scene,r,encoder,commands,key)?;}
            changed=self.project_exact(scene,r,encoder,commands)?;
        }
        let passes=packet.scene.order().iter().filter_map(|handle|packet.scene.effect(*handle).filter(|effect|
            effect.program.image_boundary() && packet.scene.visible(*handle)))
            .map(|effect|effect.program.passes.len().max(1) as u64).sum::<u64>();
        for key in native {
            let region=key.plan().bounds;let window=scene.cached_capture_window(packet.scene,region);
            let work=region.area().saturating_add(window.area().saturating_mul(passes));
            if !scene.work_fits(WorkClass::Background,!changed.is_empty(),work,object_cache::IDLE_TAPS) {break;}
            let plan=key.plan();
            let inputs=self.native_inputs(scene,r,packet,window.in_frame(packet.document_extent),plan,true)?;
            let scratch=self.graph.root.as_ref().unwrap().cost().1 as usize+1;
            let interest=Self::exact_interest(key.clone(),self.evaluation,region);
            if !self.accept_working(scene,r,interest,vec![(key.clone(),region)],inputs,[scratch,0],plan,[commands.storage_bytes(),0],true,encoder)? {continue;}
            let image=self.pages.lock().unwrap().allocated(&key).expect("accepted exact page");
            let write=self.pages.lock().unwrap().prepare_write(key.clone(),region,pages::Accuracy::Exact,pages::Freshness::Current,0)?;
            let held=self.pages.lock().unwrap().take_inputs(&key,region);
            let (held,result)=scene.with_inputs(held,|scene| {
                r.ensure_exact_preview(encoder)?;
                scene.prepare_region(r,packet,(region,window),PixelRect::EMPTY,false,encoder)?;
                scene.capture_prepared_regions(r,packet,&image,&[region],scene::Output::Display,false,None,encoder)
            });
            match result {
                Ok(())=>{
                    self.pages.lock().unwrap().commit(write,encoder.submission());
                    self.pages.lock().unwrap().finish_inputs(&key,region);
                    r.metrics.composited_pixels+=region.area();r.metrics.frame_composited_pages.push((0,key.coordinate));
                    scene.frame_work=scene.frame_work.saturating_add(work.max(object_cache::PASS_TAPS));
                    if self.plan.level.max(self.overview.as_ref().map_or(0,|cache|cache.plan.level))>0 {
                        self.reduce_exact(scene,r,encoder,commands,pages::Key {level:1,coordinate:key.coordinate.map(|value|value/2),..key})?;
                    }
                    changed=changed.union(self.project_exact(scene,r,encoder,commands)?);
                },
                Err(GpuRasterError::NotReady)=>self.pages.lock().unwrap().hold_inputs(&key,region,self.evaluation,held)?,
                Err(GpuRasterError::SourceWorkingSetExceeded)=>self.pages.lock().unwrap().finish_inputs(&key,region),
                Err(error)=>return Err(error),
            }
        }
        Ok(changed)
    }

    fn reduce_exact(&mut self,scene:&Scene,r:&mut WgpuRasterizer,encoder:&mut crate::submission::CommandEncoder,commands:&mut RegionRecords,
        mut key:pages::Key,
    )->Result<(),GpuRasterError> {
        let level=self.plan.level.max(self.overview.as_ref().map_or(0,|cache|cache.plan.level));
        loop {
            if self.pages.lock().unwrap().get(&key,key.plan().bounds,pages::Accuracy::Exact,0).is_none() {
                let inputs=self.pages.lock().unwrap().reduction(&key,0);
                let Some((inputs,pages::Accuracy::Exact,pages::Freshness::Current))=inputs else {break;};
                let Some(image)=self.claim_page(scene,r,key.clone(),[commands.storage_bytes(),0],false)? else {break;};
                let write=self.pages.lock().unwrap().prepare_write(key.clone(),image.plan.bounds,pages::Accuracy::Exact,pages::Freshness::Current,0)?;
                for (input_key,input) in inputs {
                    let covered=input.plan.bounds.intersect(image.plan.bounds);
                    let [x,y,width,height]=paint_transform::texel_rect(covered.window_local(image.plan.bounds),1<<image.plan.level);
                    let mut values=[0;20];
                    values[..8].copy_from_slice(&[x,y,width,height,input.plan.bounds.width(),input.plan.bounds.height(),2,(input.plan.level<<8)|8]);
                    values[14]=((x*2) as f32).to_bits();values[15]=((y*2) as f32).to_bits();
                    let binding=RegionRecords::binding(r,&input.view,&r.empty_view,&image.view);
                    commands.reduce(r,encoder,values,&binding,"reduce exact composite page",image.texture.format())?;
                    self.pages.lock().unwrap().read(&input_key,&input,encoder.submission());
                }
                self.pages.lock().unwrap().commit(write,encoder.submission());
            }
            if key.level>=level {break;}
            key=pages::Key {level:key.level+1,coordinate:key.coordinate.map(|value|value/2),..key};
        }
        Ok(())
    }

    fn project_exact(&mut self,scene:&Scene,r:&mut WgpuRasterizer,encoder:&mut crate::submission::CommandEncoder,commands:&mut RegionRecords)->Result<PixelRect,GpuRasterError> {
        let frame=self.pages.lock().unwrap().expected().cloned().ok_or(GpuRasterError::Invariant("signature frame is missing or inconsistent"))?;
        let served=display_mips::view_bounds(frame.view(),self.plan.extent,0).unwrap_or(PixelRect::EMPTY).intersect(self.plan.bounds);
        let mut changed=self.project_level(scene,r,encoder,commands,[0;2],served)?;
        let retained=self.window_memory();
        if let Some(cache)=&mut self.overview {
            let retained=[retained[0].saturating_sub(cache.window_bytes()),retained[1].saturating_sub(cache.window_pinned_bytes())];
            changed=changed.union(cache.project_level(scene,r,encoder,commands,retained,cache.plan.bounds)?);
        }
        Ok(changed)
    }
}

impl Scene {
    pub fn refine_display(
        &mut self, r: &mut WgpuRasterizer, packet: FramePacket<'_>,
        encoder: &mut crate::submission::CommandEncoder,
    ) -> Result<PixelRect, GpuRasterError> {
        let write = self.begin_write(r,encoder)?;
        let Some(mut cache) = r.scale_display.take() else { return Ok(PixelRect::EMPTY) };
        cache.submission_valid = Some(self.valid.clone());
        let mut commands = self.scale_commands.take().unwrap_or_else(|| RegionRecords::new(r));
        commands.begin();
        let result = cache.refine(self, r, packet, encoder, &mut commands);
        self.scale_commands = Some(commands);
        r.scale_display = Some(cache);
        if matches!(&result, Err(error) if error.is_preparation_pending()) { self.clear_unencoded_jobs(); }
        if result.is_ok() || matches!(&result,Err(error) if error.is_preparation_pending()) {write.track(encoder);}
        result
    }
}
