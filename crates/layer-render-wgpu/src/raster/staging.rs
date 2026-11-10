use super::*;

const PREFETCH:usize=18;
const UPLOADS:usize=16;

#[derive(PartialEq)]
struct DataKey {contents:[layer_core::raster::ContentStamp;3],watercolor:Option<[u32;3]>}
impl DataKey {
    fn new(data:&RasterData)->Self {Self {contents:[RasterPlane::Color,RasterPlane::Mask,RasterPlane::WatercolorWetness]
        .map(|plane|data.tiles.content(plane).root()),
        watercolor:data.watercolor.map(|w|[w.wet_edge.to_bits(),w.burnt_edge.to_bits(),w.edge_width.to_bits()])}}
}
#[derive(PartialEq)]
struct FrameRoot {target:SourceTarget,revision:u64,published:Option<usize>,restore:Option<u64>,extent:[u32;2]}
#[derive(PartialEq)]
enum Key {
    Frame {color:layer_core::color::DocumentColor,extent:[u32;2],reset:bool,clear_starts:bool,roots:Vec<FrameRoot>,
        required:Vec<(SourceTarget,Vec<[u32;2]>)>,bases:Vec<(SourceTarget,usize,[u32;2])>,outputs:Vec<(SourceTarget,Output)>},
    Direct {color:layer_core::color::DocumentColor,target:SourceTarget,extent:[u32;2],before:DataKey,after:DataKey},
}
#[derive(Default)]
pub(crate) struct PreparedPages {
    pub colors:Vec<LayerPage>,
    pub masks:Vec<([u32;2],layer_masks::MaskPage)>,
    pub wetness:Vec<WatercolorWetnessPage>,
    pub coverage:Vec<StrokeCoveragePage>,
    pub companions:Vec<([u32;2],resident::Resident<PageSurface>)>,
    pub starts:Vec<([u32;2],layer_core::StrokeId,bool,resident::Resident<PageSurface>)>,
}
impl PreparedPages {
    fn storage_bytes(&self)->u64 {
        self.colors.iter().map(|page|page.primary.storage_bytes()+page.secondary.as_ref().map_or(0,|surface|surface.storage_bytes())
            +page.stroke_start.as_ref().map_or(0,StrokeStart::storage_bytes)).sum::<u64>()
            +self.masks.iter().map(|(_,page)|page.storage_bytes()).sum::<u64>()
            +self.wetness.iter().map(|page|page.primary.storage_bytes()+page.secondary.storage_bytes()).sum::<u64>()
            +self.coverage.iter().map(|page|page.primary.storage_bytes()+page.secondary.storage_bytes()).sum::<u64>()
            +self.companions.iter().map(|(_,page)|page.storage_bytes()).sum::<u64>()
            +self.starts.iter().map(|(_,_,_,page)|page.storage_bytes()).sum::<u64>()
    }
}
struct TargetRestore {
    target:SourceTarget,
    extent:[u32;2],
    defaults:[layer_core::raster::ContentVersion;3],
    data:Arc<RasterData>,
    selected:RasterData,
    pending:Vec<SourcePage>,
    next:usize,
    pages:PreparedPages,
    replace:bool,
    scalars:Vec<object_image_mips::PreparedRasterTile>,
    inputs:scene::sources::AcceptedInputs,
}
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord)]
pub(crate) enum Output {
    Color([u32;2]),Coverage([u32;2],u32,bool),Wetness([u32;2]),Companion([u32;2]),
    StrokeStart([u32;2],layer_core::StrokeId,bool),Mask([u32;2],u32),
    PreviewColor([u32;2]),PreviewCoverage([u32;2],u32,bool),PreviewWetness([u32;2]),PreviewCompanion([u32;2]),
}
impl Output {
    pub(crate) fn native_key(&self)->Option<TileKey> {
        let (plane,coordinate)=match *self {
            Self::Color(c)=>(RasterPlane::Color,c),Self::Mask(c,_)=>(RasterPlane::Mask,c),
            Self::Wetness(c)=>(RasterPlane::WatercolorWetness,c),_=>return None,
        };Some(TileKey {plane,coordinate})
    }
    fn coordinate(&self)->[u32;2] {match self {
        Self::Color(c)|Self::Coverage(c,..)|Self::Wetness(c)|Self::Companion(c)|Self::StrokeStart(c,..)|Self::Mask(c,_)
            |Self::PreviewColor(c)|Self::PreviewCoverage(c,..)|Self::PreviewWetness(c)|Self::PreviewCompanion(c)=>*c,
    }}
    fn preview(&self)->bool {matches!(self,Self::PreviewColor(_)|Self::PreviewCoverage(..)|Self::PreviewWetness(_)|Self::PreviewCompanion(_))}
    fn bytes(&self,r:&WgpuRasterizer,preview_level:u32)->u64 {
        let (format,count)=match self {
            Self::Color(_)|Self::Companion(_)|Self::StrokeStart(..)|Self::PreviewColor(_)|Self::PreviewCompanion(_)=>(r.device.working_format(),1),
            Self::Coverage(_,slabs,rgba)|Self::PreviewCoverage(_,slabs,rgba)=>(if *rgba {wgpu::TextureFormat::Rgba32Float}else {r.device.scalar_format()},2*u64::from(*slabs)),
            Self::Wetness(_)|Self::PreviewWetness(_)=>(r.device.scalar_format(),2),Self::Mask(..)=>(r.device.scalar_format(),1),
        };
        let side=if matches!(self,Self::PreviewColor(_)) {PAGE_SIZE>>preview_level}else {PAGE_SIZE};
        u64::from(side).pow(2)*u64::from(format.block_copy_size(None).unwrap())*count
    }
    fn same_storage(&self,other:&Self)->bool {match (self,other) {
        (Self::Mask(a,_),Self::Mask(b,_))=>a==b,_=>self==other,
    }}
    fn preview_present(&self,r:&WgpuRasterizer,level:u32)->bool {match self {
        Self::PreviewColor(c)=>r.preview_pages.iter().any(|page|page.coordinate==*c && page.primary.size().width==PAGE_SIZE>>level),
        Self::PreviewCoverage(c,slabs,rgba)=>r.preview_coverage_pages.iter().any(|page|page.coordinate==*c
            && page.primary.size().height==PAGE_SIZE*slabs && (page.primary.format()==wgpu::TextureFormat::Rgba32Float)==*rgba),
        Self::PreviewWetness(c)=>r.preview_watercolor_wetness_pages.iter().any(|page|page.coordinate==*c),
        Self::PreviewCompanion(c)=>r.preview_pages.iter().any(|page|page.coordinate==*c && page.secondary.is_some()),
        _=>false,
    }}
}
enum SourcePage {Raster(TileKey,RasterTile),Base([u32;2],layer_core::authored::PaintBase),Output(Output)}
impl SourcePage {
    fn inputs(&self,space:layer_core::color::RgbSpace)->Result<Option<scene::sources::Inputs>,GpuRasterError> {
        let mut inputs=scene::sources::Inputs::default();
        match self {
            Self::Raster(key,tile) if key.plane==RasterPlane::Color=> {
                let Some(blob)=tile.try_backing().transpose().map_err(GpuRasterError::Effect)? else {return Ok(None);};
                if blob.descriptor!=tile.descriptor() {return Err(GpuRasterError::Effect("Raster plane has the wrong pixel representation".into()));}
                inputs.raster(blob,space,space);
            },
            Self::Base(coordinate,base)=>inputs.base(base,*coordinate,space),
            _=>{},
        }
        Ok(Some(inputs))
    }
    fn output(&self)->Output {match self {
        Self::Raster(key,_)=>match key.plane {RasterPlane::Color=>Output::Color(key.coordinate),RasterPlane::Mask=>Output::Mask(key.coordinate,0),RasterPlane::WatercolorWetness=>Output::Wetness(key.coordinate)},
        Self::Base(c,_)=>Output::Color(*c),Self::Output(output)=>output.clone(),
    }}
}
struct Request {target:SourceTarget,extent:[u32;2],before:RasterData,data:Arc<RasterData>,required:BTreeSet<[u32;2]>,base:Option<layer_core::authored::PaintBase>,replace:bool}
impl TargetRestore {
    fn new(r:&mut WgpuRasterizer,request:Request,commit:Option<&paint_transform::PreparedTransformCommit>)->Result<Self,GpuRasterError> {
        let Request {target,extent,before,data,required,base,replace}=request;
        data.validate_storage_index(extent,target.is_coverage(),r.document_color()).map_err(GpuRasterError::Effect)?;
        let defaults=super::contents::defaults_for(extent,base.as_ref(),None);
        let (before,selected)=r.select_restore_data(target,&before,&data,&required);
        let resident=|key:&TileKey|if commit.is_some_and(|commit|commit.contains(target,key.plane,key.coordinate)) {true}else {match key.plane {
            RasterPlane::Mask=>r.layer_masks.pages.get(&(target,key.coordinate)).is_some_and(|page| {
                let name=(!replace).then(||r.raster_intent(target,key.plane,key.coordinate).ok().map(|intent|intent.version().clone())).flatten()
                    .or_else(||data.tiles.get(key).map(RasterTile::content_version));
                name.is_some_and(|name|page.has_name(&name))
            }),
            RasterPlane::Color|RasterPlane::WatercolorWetness=> {
                let name=(!replace).then(||r.current_raster_version(target,key.plane,key.coordinate).ok()).flatten()
                    .or_else(||data.tiles.get(key).map(RasterTile::content_version))
                    .unwrap_or_else(||super::contents::fallback(&defaults[super::contents::index(key.plane)],key.coordinate));
                r.paint_layers.iter().find(|layer|layer.id==target).is_some_and(|layer|match key.plane {
                    RasterPlane::Color=>layer.pages.iter().find(|page|page.coordinate==key.coordinate).is_some_and(|page|page.active().has_name(&name)),
                    _=>layer.watercolor_wetness_pages.iter().find(|page|page.coordinate==key.coordinate).is_some_and(|page|page.active().has_name(&name)),
                })
            },
        }};
        let mut pending:Vec<_>=selected.tiles.iter().filter(|(key,tile)|!commit.is_some_and(|commit|commit.contains(target,key.plane,key.coordinate)) && if replace {
            !before.tiles.get(key).is_some_and(|old|old.same_capture(tile)) || required.contains(&key.coordinate) && !resident(key)
        }else {required.contains(&key.coordinate) && !resident(key)})
            .map(|(key,tile)|SourcePage::Raster(*key,tile.clone())).collect();
        if let Some(base)=base {
            pending.extend(required.iter().filter(|coordinate| !commit.is_some_and(|commit|commit.contains(target,RasterPlane::Color,**coordinate)) && (replace || !resident(&TileKey {plane:RasterPlane::Color,coordinate:**coordinate})) && !data.tiles.contains_key(&TileKey {plane:RasterPlane::Color,coordinate:**coordinate})
                && source_access::paint_base_contains(&base,**coordinate)).map(|coordinate|SourcePage::Base(*coordinate,base.clone())));
        }
        Ok(Self {target,extent,defaults,data,selected,pending,next:0,pages:PreparedPages::default(),replace,scalars:Vec::new(),inputs:Default::default()})
    }
    fn awaits(&self,blob:&TileBlob)->bool {
        self.pending.iter().skip(self.next).filter(|source|!matches!(source,SourcePage::Output(_))).take(PREFETCH).any(|source|matches!(source,SourcePage::Raster(key,tile) if key.plane!=RasterPlane::Color
            && tile.try_backing().and_then(Result::ok).is_some_and(|awaited|scene::sources::same_raster_samples(&awaited,blob))))
    }
    fn held(&self,blob:&TileBlob)->Option<&object_image_mips::PreparedRasterTile> {
        self.scalars.iter().find(|prepared|scene::sources::same_raster_samples(&prepared.blob,blob))
    }
    fn projected_bytes(&self,r:&WgpuRasterizer)->Result<u64,GpuRasterError> {
        if self.pages.storage_bytes()!=0 {return Ok(0);}
        self.pending.iter().try_fold(0u64,|bytes,source|bytes.checked_add(source.output().bytes(r,r.preview_level)).ok_or(GpuRasterError::SizeOverflow))
    }
    fn add_outputs(&mut self,r:&WgpuRasterizer,outputs:&[(SourceTarget,Output)],reset:bool,clear_starts:bool,commit:Option<&paint_transform::PreparedTransformCommit>) {
        for (_,output) in outputs.iter().filter(|(target,_)|*target==self.target) {
            if output.preview() {continue;}
            if self.pending.iter().any(|source|source.output().same_storage(output)) {continue;}
            let c=output.coordinate();
            let plane=match output {Output::Color(_)=>Some(RasterPlane::Color),Output::Wetness(_)=>Some(RasterPlane::WatercolorWetness),Output::Mask(..)=>Some(RasterPlane::Mask),_=>None};
            if plane.is_some_and(|plane|commit.is_some_and(|commit|commit.contains(self.target,plane,c))) {continue;}
            let rewritten=self.pending.iter().any(|source|matches!(source.output(),Output::Color(at) if at==c));
            if let Output::StrokeStart(_,id,true)=output && self.pending.iter().any(|source|
                matches!(source,SourcePage::Output(Output::StrokeStart(at,old,false)) if *at==c && old==id)) {continue;}
            let layer=(!reset).then(||r.paint_layers.iter().find(|layer|layer.id==self.target)).flatten();
            let page=layer.and_then(|layer|layer.pages.iter().find(|page|page.coordinate==c)).filter(|page| {
                let key=TileKey {plane:RasterPlane::Color,coordinate:c};
                let name=(!self.replace).then(||r.current_raster_version(self.target,key.plane,c).ok()).flatten()
                    .or_else(||self.selected.tiles.get(&key).map(RasterTile::content_version))
                    .unwrap_or_else(||super::contents::fallback(&self.defaults[0],c));
                !rewritten && page.active().has_name(&name)
            });
            if matches!(output,Output::Companion(_)|Output::StrokeStart(_,_,false)) && page.is_none() && !rewritten {continue;}
            let present=match output {
                Output::Color(_)=>page.is_some(),Output::Companion(_)=>page.is_some_and(|page|page.secondary.is_some()),
                Output::StrokeStart(_,id,_)=>!clear_starts && page.is_some_and(|page|page.stroke_start.as_ref().is_some_and(|start|start.state.stroke==*id)),
                Output::Coverage(_,slabs,rgba)=>!self.replace && layer.is_some_and(|layer|layer.coverage_pages.iter().any(|page|page.coordinate==c
                    && page.primary.size().height==PAGE_SIZE*slabs && (page.primary.format()==wgpu::TextureFormat::Rgba32Float)==*rgba)),
                Output::Wetness(_)=>layer.is_some_and(|layer|layer.watercolor_wetness_pages.iter().any(|page|page.coordinate==c))
                    && (!self.replace || self.selected.tiles.contains_key(&TileKey {plane:RasterPlane::WatercolorWetness,coordinate:c})),
                Output::Mask(_,default)=>!reset && r.layer_masks.pages.get(&(self.target,c)).is_some_and(|page| {
                    let key=TileKey {plane:RasterPlane::Mask,coordinate:c};
                    let name=(!self.replace).then(||r.raster_intent(self.target,key.plane,c).ok().map(|intent|intent.version().clone())).flatten()
                        .or_else(||self.selected.tiles.get(&key).map(RasterTile::content_version))
                        .unwrap_or_else(||super::contents::mask_default(self.extent,f32::from_bits(*default),c));
                    page.has_name(&name)
                }),
                _=>unreachable!(),
            };
            if !present {self.pending.push(SourcePage::Output(output.clone()));}
        }
    }
    fn allocate(&mut self,r:&WgpuRasterizer,encoder:&mut submission::CommandEncoder)->Result<(),GpuRasterError> {
        for source in &self.pending {match source.output() {
            Output::Color(c)=> {
                let mut page=r.create_page(c,"prepared raster color");
                if matches!(source,SourcePage::Output(_)) {
                    let receipt=encoder.submission();let write=page.primary.initialize(&receipt).ok_or(GpuRasterError::Invariant("prepared output was already initialized"))?;
                    write.value().clear(&r.zero_pages,encoder);write.publish(super::contents::fallback(&self.defaults[0],c));page.primary_needs_clear=false;
                }
                self.pages.colors.push(page);
            },
            Output::Coverage(c,slabs,rgba)=>self.pages.coverage.push(r.create_stroke_coverage_page(c,slabs,rgba)),
            Output::Wetness(c)=>self.pages.wetness.push(WatercolorWetnessPage {pending:None,derived:None,coordinate:c,
                primary:resident::Resident::unwritten(r.create_scalar_page_surface("prepared wetness primary")),secondary:resident::Resident::unwritten(r.create_scalar_page_surface("prepared wetness companion")),
                active_secondary:false,primary_needs_clear:true,secondary_needs_clear:true}),
            Output::Companion(c)=>self.pages.companions.push((c,resident::Resident::unwritten(r.create_page_surface("prepared destination companion")))),
            Output::StrokeStart(c,id,preview)=>self.pages.starts.push((c,id,preview,resident::Resident::unwritten(r.create_page_surface("prepared stroke start")))),
            Output::Mask(c,default)=> {
                let mut page=layer_masks::MaskPage::new(&r.device);let value=f64::from(f32::from_bits(default));
                if matches!(source,SourcePage::Output(_)) {
                    let receipt=encoder.submission();let write=page.initialize(&receipt)?;
                    {let _clear=encoder.color_pass("prepared default mask",&write.value().view,wgpu::LoadOp::Clear(wgpu::Color {r:value,g:value,b:value,a:value}));}
                    write.publish(super::contents::mask_default(self.extent,f32::from_bits(default),c));
                }
                self.pages.masks.push((c,page));
            },
            _=>unreachable!(),
        }}
        Ok(())
    }
    fn complete(&self)->bool {self.next==self.pending.len()}
    fn source_kinds(&self)->startup::SourceKinds {
        let mut kinds=startup::SourceKinds::default();
        for source in self.pending.iter().skip(self.next).filter(|source|!matches!(source,SourcePage::Output(_))).take(PREFETCH) {
            match source {
                SourcePage::Base(_,base)=>kinds.include(startup::SourceKinds::for_source(base.image.storage())),
                SourcePage::Raster(key,tile) if key.plane==RasterPlane::Color=>kinds.include(startup::SourceKinds::for_descriptor(tile.descriptor())),
                _=>{},
            }
        }
        kinds
    }
    fn ready(&self,r:&WgpuRasterizer)->bool {
        if self.projected_bytes(r).is_ok_and(|bytes|bytes!=0) {return false;}
        let Some(source)=self.pending.get(self.next) else {return false;};
        if let SourcePage::Raster(key,tile)=source && key.plane!=RasterPlane::Color {
            let Some(Ok(blob))=tile.try_backing() else {return false;};
            return self.held(&blob).is_some() && r.source_tiles.borrow().can_upload_bytes(native_tiles::scalar::COVERAGE_TILE_BYTES);
        }
        source.inputs(r.document_color().space).ok().flatten().is_some_and(|inputs|self.inputs.ready_for(&inputs))
    }

    fn advance(&mut self,r:&mut WgpuRasterizer,encoder:&mut submission::CommandEncoder)->Result<(),GpuRasterError> {
        let space=r.document_color().space;
        self.inputs.prepare(r)?;
        for source in self.pending.iter().skip(self.next).filter(|source|!matches!(source,SourcePage::Output(_))).take(PREFETCH) {
            if let SourcePage::Raster(key,tile)=source && key.plane!=RasterPlane::Color {
                let Some(blob)=tile.try_backing().transpose().map_err(GpuRasterError::Effect)? else {break;};
                if self.held(&blob).is_none() && !r.request_scalar_decode(blob,space)? {break;}
            }
        }
        let mut uploads=0;
        while let Some(source)=self.pending.get(self.next) {
            if matches!(source,SourcePage::Output(_)) {self.next+=1;continue;}
            if uploads==UPLOADS {break;}
            uploads+=1;
            if let SourcePage::Base(coordinate,base)=source {
                let Some(read)=scene::sources::accepted_base(&self.inputs,base,*coordinate)? else {self.next+=1;continue;};
                if !read.encode(r,encoder)? {break;}
                let page=self.pages.colors.iter_mut().find(|page|page.coordinate==*coordinate).unwrap();page.primary_needs_clear=false;
                let receipt=encoder.submission();let write=page.primary.initialize(&receipt).ok_or(GpuRasterError::Invariant("prepared output was already initialized"))?;
                encoder.copy_texture_to_texture(read.tile.texture.as_image_copy(),write.value().texture.as_image_copy(),write.value().texture.size());
                write.publish(super::contents::fallback(&self.defaults[0],*coordinate));
                self.next+=1;continue;
            }
            let SourcePage::Raster(key,tile)=source else {unreachable!()};
            let Some(blob)=tile.try_backing().transpose().map_err(GpuRasterError::Effect)? else {break;};
            if blob.descriptor!=tile.descriptor() {return Err(GpuRasterError::Effect("Raster plane has the wrong pixel representation".into()));}
            if key.plane==RasterPlane::Color {
                let read=scene::sources::accepted_raster(&self.inputs,&blob,space,space)?;
                if !read.encode(r,encoder)? {break;}
                let page=self.pages.colors.iter_mut().find(|page|page.coordinate==key.coordinate).unwrap();page.primary_needs_clear=false;
                let receipt=encoder.submission();let write=page.primary.initialize(&receipt).ok_or(GpuRasterError::Invariant("prepared output was already initialized"))?;
                encoder.copy_texture_to_texture(read.tile.texture.as_image_copy(),write.value().texture.as_image_copy(),write.value().texture.size());
                write.publish(tile.content_version());
                self.next+=1;continue;
            }
            let Some(prepared)=self.held(&blob) else {r.request_scalar_decode(blob,space)?;break;};
            let samples=match &prepared.pixels {
                Ok(scene::sources::PreparedSourcePixels::NativeSamples(samples))=>samples.clone(),
                Ok(_)=>return Err(GpuRasterError::Color("Coverage requires native samples".into())),
                Err(error)=>return Err(GpuRasterError::Color(error.clone())),
            };
            if !r.source_tiles.borrow().can_upload_bytes(native_tiles::scalar::COVERAGE_TILE_BYTES) {break;}
            if key.plane==RasterPlane::Mask {
                let page=&mut self.pages.masks.iter_mut().find(|(c,_)|*c==key.coordinate).unwrap().1;
                let receipt=encoder.submission();let write=page.initialize(&receipt)?;
                native_tiles::scalar::upload_coverage(r,encoder,&write.value().texture,blob.descriptor.depth(),&samples,4)?;
                write.publish(tile.content_version());
            } else {
                let page=self.pages.wetness.iter_mut().find(|page|page.coordinate==key.coordinate).unwrap();
                let receipt=encoder.submission();let write=page.primary.initialize(&receipt).ok_or(GpuRasterError::Invariant("prepared output was already initialized"))?;
                native_tiles::scalar::upload_coverage(r,encoder,&write.value().texture,blob.descriptor.depth(),&samples,4)?;
                write.publish(tile.content_version());page.primary_needs_clear=false;
            }
            self.next+=1;
        }
        let mut scalars=std::mem::take(&mut self.scalars);
        scalars.retain(|prepared|self.awaits(&prepared.blob));
        self.scalars=scalars;
        let mut wanted=scene::sources::Inputs::default();
        for source in self.pending.iter().skip(self.next).filter(|source|!matches!(source,SourcePage::Output(_))).take(PREFETCH) {
            if let Some(inputs)=source.inputs(space)? {wanted.extend(inputs);}
        }
        self.inputs.retain(&wanted);
        Ok(())
    }

}
pub(crate) struct RestorePreflight {
    key:Key,
    commit:Option<paint_transform::PreparedTransformCommit>,
    pub(crate) native:Option<super::native_edit::NativeAdmission>,
    targets:Vec<Option<TargetRestore>>,
    failure:Option<GpuRasterError>,
    consumed:bool,
    starts:Vec<(SourceTarget,[u32;2],layer_core::StrokeId,bool,resident::Resident<PageSurface>)>,
}
impl RestorePreflight {
    pub(crate) fn outputs(&self)->&[(SourceTarget,Output)] {
        match &self.key {Key::Frame {outputs,..}=>outputs,_=>&[]}
    }
    pub(crate) fn dependencies(&self)->&[(SourceTarget,Vec<[u32;2]>)] {
        match &self.key {
            Key::Frame {required,..}=>required,
            _=>&[],
        }
    }
    fn new(r:&mut WgpuRasterizer,key:Key,requests:Vec<Request>,commit:Option<paint_transform::PreparedTransformCommit>)->Self {
        let mut targets=Vec::new();let mut failure=None;
        for request in requests {
            match TargetRestore::new(r,request,commit.as_ref()) {
                Ok(target)=>targets.push(Some(target)),
                Err(error)=>{failure=Some(error);break;},
            }
        }
        if let Key::Frame {outputs,reset,clear_starts,..}=&key {for target in targets.iter_mut().flatten() {target.add_outputs(r,outputs,*reset,*clear_starts,commit.as_ref());}}
        Self {key,commit,native:None,targets,failure,consumed:false,starts:Vec::new()}
    }
    fn complete(&self)->bool {self.failure.is_some() || self.targets.iter().flatten().all(TargetRestore::complete)}
}
impl WgpuRasterizer {
    pub(crate) fn accepted_raster_output(&self,target:SourceTarget,output:Output)->Result<(),GpuRasterError> {
        let accepted=self.restore_preflight.as_ref().is_some_and(|restore|match &restore.key {
            Key::Frame {outputs,..}=>outputs.binary_search(&(target,output)).is_ok(),_=>false,
        });
        prepared_raster_output(accepted.then_some(()))
    }
    pub(crate) fn accepted_material_tiles<'a>(&self,batch:&DabBatch,context:BrushEncodingContext<'a>)->Result<std::borrow::Cow<'a,[BrushTile]>,GpuRasterError> {
        let preview=context.target.is_preview();
        let rectangular=context.target==(BrushEncodingTarget::Preview {from_persistent:true}) && self.preview_contact_tiles.is_none();
        let tiles=if rectangular {
            let Some(restore)=&self.restore_preflight else {return prepared_raster_output(None);};
            let Key::Frame {outputs,..}=&restore.key else {return prepared_raster_output(None);};
            let damage=batch_pixel_rect(batch,context.target_extent);
            std::borrow::Cow::Owned(outputs.iter().filter_map(|(target,output)|match output {
                Output::PreviewColor(coordinate) if *target==batch.target && !page_rect(*coordinate).intersect(damage).is_empty()=>Some(BrushTile {
                    coordinate:*coordinate,local:PixelRect::full([PAGE_SIZE;2]),
                    dabs:context.tiles.iter().find(|tile|tile.coordinate==*coordinate).map_or(batch.first_dab..batch.first_dab,|tile|tile.dabs.clone()),indices:Vec::new(),
                }),_=>None,
            }).collect())
        } else {std::borrow::Cow::Borrowed(context.tiles)};
        for tile in tiles.iter() {
            self.accepted_raster_output(batch.target,if preview {Output::PreviewColor(tile.coordinate)}else {Output::Color(tile.coordinate)})?;
        }
        Ok(tiles)
    }
    pub(crate) fn prepare_preview_outputs(&mut self,level:u32)->Result<(),GpuRasterError> {
        let outputs=self.restore_preflight.as_ref().and_then(|restore|match &restore.key {
            Key::Frame {outputs,..}=>Some(outputs.iter().map(|(_,output)|*output).filter(Output::preview).collect::<BTreeSet<_>>()),_=>None,
        }).unwrap_or_default();
        for output in &outputs {
            if output.preview_present(self,level) {continue;}
            let coordinate=output.coordinate();
            match output {
                Output::PreviewColor(_)=>{
                    if let Some(page)=self.preview_pages.iter_mut().find(|page|!outputs.contains(&Output::PreviewColor(page.coordinate))
                        && page.primary.size().width==PAGE_SIZE>>level) {
                        page.coordinate=coordinate;page.active_secondary=false;page.clear_stroke_start();
                    }
                },
                Output::PreviewCoverage(_,slabs,rgba)=>{
                    if let Some(page)=self.preview_coverage_pages.iter_mut().find(|page|
                        !outputs.contains(&Output::PreviewCoverage(page.coordinate,page.primary.size().height/PAGE_SIZE,page.primary.format()==wgpu::TextureFormat::Rgba32Float))
                        && page.primary.size().height==PAGE_SIZE*slabs && (page.primary.format()==wgpu::TextureFormat::Rgba32Float)==*rgba) {
                        page.coordinate=coordinate;page.owner=None;page.active_secondary=false;
                    }
                },
                Output::PreviewWetness(_)=>{
                    if let Some(page)=self.preview_watercolor_wetness_pages.iter_mut().find(|page|!outputs.contains(&Output::PreviewWetness(page.coordinate))) {
                        page.coordinate=coordinate;page.active_secondary=false;
                    }
                },_=>{},
            }
        }
        let missing=outputs.into_iter().filter(|output|!output.preview_present(self,level)).collect::<Vec<_>>();
        let bytes=missing.iter().map(|output|output.bytes(self,level)).sum();
        if bytes==0 {self.preview_pages.retain(|page|page.primary.size().width==PAGE_SIZE>>level);return Ok(());}
        let reservation=self.admit_raster_working(bytes).ok_or(GpuRasterError::NotReady)?;
        for output in missing {match output {
            Output::PreviewColor(coordinate)=>self.preview_pages.push(LayerPage {derived:None,coordinate,
                primary:resident::Resident::unwritten(create_page_surface(&self.device,&self.float_texture_layout,&self.sampler,[PAGE_SIZE>>level;2],self.device.working_format(),"prepared preview color")),
                secondary:None,active_secondary:false,primary_needs_clear:true,stroke_start:None}),
            Output::PreviewCoverage(coordinate,slabs,rgba)=>self.preview_coverage_pages.push(self.create_stroke_coverage_page(coordinate,slabs,rgba)),
            Output::PreviewWetness(coordinate)=>self.preview_watercolor_wetness_pages.push(WatercolorWetnessPage {coordinate,derived:None,pending:None,
                primary:resident::Resident::unwritten(self.create_scalar_page_surface("prepared preview wetness primary")),secondary:resident::Resident::unwritten(self.create_scalar_page_surface("prepared preview wetness companion")),
                active_secondary:false,primary_needs_clear:false,secondary_needs_clear:false}),
            Output::PreviewCompanion(coordinate)=>{
                let surface=resident::Resident::unwritten(self.create_page_surface("prepared preview companion"));
                let page=prepared_raster_output(self.preview_pages.iter_mut().find(|page|page.coordinate==coordinate))?;
                page.secondary=Some(surface);
            },_=>unreachable!(),
        }}
        self.commit_raster_working(reservation)?;
        self.preview_pages.retain(|page|page.primary.size().width==PAGE_SIZE>>level);
        Ok(())
    }
    fn raster_outputs(&self,packet:FramePacket<'_>,plans:&[Vec<BrushTile>])->Vec<(SourceTarget,Output)> {
        let mut outputs=BTreeSet::new();
        let from_persistent=self.preview_from_persistent(packet.dab_batches);
        let rectangular=from_persistent && !Self::sparse_preview(packet.dab_batches);
        for (batch,tiles) in packet.dab_batches.iter().zip(plans) {
            let target=batch.target;
            if target.is_coverage() {
                let owner=packet.scene.source_owner(target).unwrap();let (_,source)=packet.scene.mask(owner).unwrap();
                let transform=matches!(batch.kind,DabBatchKind::RasterOperation(index) if source.operations.get(index as usize)
                    .is_some_and(|operation|matches!(operation.kind,layer_core::RasterOperationKind::Transform(_))));
                if !transform {for c in page_coordinates(batch_pixel_rect(batch,source.domain)) {outputs.insert((target,Output::Mask(c,source.default_coverage.to_bits())));}}
                continue;
            }
            let pass=BrushPassPlan::for_device(&batch.style,&self.device);
            let destinations=brush_tiles::destinations(tiles,batch_pixel_rect(batch,packet.scene.target_extent(target)),
                batch.kind==DabBatchKind::Preview && rectangular,batch.first_dab);
            if batch.kind==DabBatchKind::Persistent {
                for tile in destinations.iter() {
                    let c=tile.coordinate;outputs.insert((target,Output::Color(c)));
                    if pass.state.coverage {outputs.insert((target,Output::Coverage(c,stroke_coverage_slabs(&batch.style),stable_blender(&batch.style))));}
                    if pass.state.watercolor_wetness {outputs.insert((target,Output::Wetness(c)));}
                }
            }
            if batch.kind==DabBatchKind::Preview && Self::brush_work_planned(batch,tiles,packet.scene.target_extent(target)) {
                let damage=Self::brush_display_damage(batch,tiles,packet.scene.target_extent(target));
                let coordinates=if Self::sparse_preview(packet.dab_batches) {tiles.iter().map(|tile|tile.coordinate).collect::<BTreeSet<_>>()}
                    else {page_coordinates(damage).collect()};
                for coordinate in &coordinates {
                    outputs.insert((target,Output::PreviewColor(*coordinate)));
                    if pass.state.watercolor_wetness {outputs.insert((target,Output::PreviewWetness(*coordinate)));}
                }
                if !from_persistent {
                    for coordinate in page_coordinates(batch_pixel_rect(batch,packet.scene.target_extent(target))).filter(|coordinate|coordinates.contains(coordinate)) {
                        if pass.state.coverage {outputs.insert((target,Output::PreviewCoverage(coordinate,stroke_coverage_slabs(&batch.style),stable_blender(&batch.style))));}
                        if batch.style.execution!=BrushExecution::Liquify && pass.requires_destination() {outputs.insert((target,Output::PreviewCompanion(coordinate)));}
                    }
                }
            }
            if matches!(batch.kind,DabBatchKind::Persistent|DabBatchKind::Preview) && Self::brush_work_planned(batch,tiles,packet.scene.target_extent(target))
                && Self::tracks_stroke_start(batch) {
                let source=if batch.kind==DabBatchKind::Preview {BrushEncodingTarget::Preview {from_persistent}}else {BrushEncodingTarget::Persistent};
                for tile in destinations.iter() {outputs.insert((target,Output::StrokeStart(tile.coordinate,batch.stroke_id,source.material_source().is_preview())));}
            }
            if let DabBatchKind::RasterOperation(index)=batch.kind && let Some(operation)=packet.scene.operations(target).and_then(|operations|operations.get(index as usize)) {
                let masking=matches!(operation.kind,layer_core::RasterOperationKind::ColorMode(_)|layer_core::RasterOperationKind::ApplyMask|layer_core::RasterOperationKind::Erase {..});
                if matches!(operation.kind,layer_core::RasterOperationKind::Fill {..}|layer_core::RasterOperationKind::Gradient {..}|layer_core::RasterOperationKind::Figure(_)|layer_core::RasterOperationKind::Bake {..}|layer_core::RasterOperationKind::FrequencyDetail {..})
                    || masking {
                    for c in page_coordinates(batch_pixel_rect(batch,packet.scene.target_extent(target))) {
                        if !masking || self.native_backing(target).is_some_and(|data|data.tiles.contains_key(&TileKey {plane:RasterPlane::Color,coordinate:c}))
                            || packet.scene.paint_base(target).is_some_and(|base|source_access::paint_base_contains(base,c))
                            || self.paint_layers.iter().find(|layer|layer.id==target).is_some_and(|layer|layer.pages.iter().any(|page|page.coordinate==c)) {outputs.insert((target,Output::Color(c)));}
                    }
                }
            }
        }
        if let Some(fill)=&self.fill_preview {
            let damage=pixel_rect(fill.operation.bounds(packet.scene.target_extent(fill.target)),packet.scene.target_extent(fill.target));
            for coordinate in page_coordinates(damage) {outputs.insert((fill.target,Output::PreviewColor(coordinate)));}
        }
        for batch in packet.dab_batches.iter().filter(|batch|batch.kind==DabBatchKind::Preview
            && !batch.target.is_coverage() && BrushPassPlan::for_device(&batch.style,&self.device).state.watercolor_wetness) {
            let extent=packet.scene.target_extent(batch.target);
            let damage=packet.dab_batches.iter().zip(plans).filter(|(other,_)|other.kind==DabBatchKind::Preview && other.target==batch.target)
                .fold(PixelRect::EMPTY,|damage,(other,tiles)|damage.union(Self::brush_display_damage(other,tiles,extent)));
            for coordinate in page_coordinates(damage.expand(WatercolorLayerStyle::from_dab_style(&batch.style).radius(),extent)) {
                outputs.insert((batch.target,Output::PreviewWetness(coordinate)));
            }
        }
        for (target,c) in self.destination_pages(packet.dab_batches,plans) {
            outputs.insert((target,Output::Color(c)));
            outputs.insert((target,Output::Companion(c)));
        }
        outputs.into_iter().collect()
    }
    fn retire_restore(&mut self) {
        let bytes=self.staged_restore_bytes();
        if bytes==0 {self.restore_preflight=None;return;}
        let charge=self.raster_buffers.transfer.clone();charge.fetch_add(bytes,std::sync::atomic::Ordering::AcqRel);
        let restore=self.restore_preflight.take().unwrap();
        let pages=restore.targets.into_iter().flatten().map(|target|target.pages).collect::<Vec<_>>();
        let starts=restore.starts;
        #[cfg(target_arch="wasm32")] drop((pages,starts));
        self.queue.on_submitted_work_done(move||{
            #[cfg(not(target_arch="wasm32"))] drop((pages,starts));
            charge.fetch_sub(bytes,std::sync::atomic::Ordering::AcqRel);
        });
    }
    pub(crate) fn take_prepared_stroke_start(&mut self,target:SourceTarget,coordinate:[u32;2],stroke:layer_core::StrokeId,preview:bool)->Option<resident::Resident<PageSurface>> {
        let starts=&mut self.restore_preflight.as_mut()?.starts;
        let index=starts.iter().position(|(id,c,old,p,_)|*id==target && *c==coordinate && *old==stroke && *p==preview)?;
        Some(starts.swap_remove(index).4)
    }
    pub(crate) fn admit_preparation_bytes(&mut self,restore:u64,bake:u64)->Result<(),GpuRasterError> {
        self.admit_preparation_with_transforms(restore,bake,None)
    }
    fn retained_preparation_bytes(&self,transforms:Option<&paint_transform::PaintTransforms>)->Result<u64,GpuRasterError> {
        let mut textures=RetainedTextures::default();
        let mut bytes=0u64;
        for layer in &self.paint_layers {
            for page in &layer.pages {
                for surface in std::iter::once(&page.primary).chain(page.secondary.iter()).chain(page.stroke_start.iter().map(|start|&start.surface)) {
                    if surface.account_texture(&mut textures) {bytes=bytes.checked_add(surface.storage_bytes()).ok_or(GpuRasterError::SizeOverflow)?;}
                }
            }
            for page in &layer.watercolor_wetness_pages {
                for surface in [&page.primary,&page.secondary] {
                    if surface.account_texture(&mut textures) {bytes=bytes.checked_add(surface.storage_bytes()).ok_or(GpuRasterError::SizeOverflow)?;}
                }
            }
        }
        if let Some(commit)=self.restore_preflight.as_ref().and_then(|restore|restore.commit.as_ref()) {
            bytes=bytes.checked_add(commit.preparation_bytes(&mut textures)).ok_or(GpuRasterError::SizeOverflow)?;
        }
        let mut masks=std::collections::BTreeSet::new();
        for page in self.layer_masks.pages.values() {if masks.insert(page.allocation_identity()) {bytes=bytes.checked_add(page.storage_bytes()).ok_or(GpuRasterError::SizeOverflow)?;}}
        bytes=bytes.checked_add(self.scale_display.as_ref().map_or(0,|cache|cache.resident_bytes()))
            .and_then(|bytes|bytes.checked_add(transforms.or(self.transforms.as_ref()).map_or(0,|transforms|transforms.preparation_bytes(&mut textures))))
            .ok_or(GpuRasterError::SizeOverflow)?;
        Ok(bytes.saturating_add(self.staged_restore_bytes()))
    }
    pub(crate) fn admit_preparation_with_transforms(&mut self,restore:u64,bake:u64,transforms:Option<&paint_transform::PaintTransforms>)->Result<(),GpuRasterError> {
        if self.native_edit.is_none() {return Ok(());}
        self.refresh_preparation_memory();
        let [scene, paint] = self.memory.preparation_limits();
        let limit=scene.checked_add(paint).ok_or(GpuRasterError::SizeOverflow)?
            .saturating_sub(self.retired_display_bytes.load(std::sync::atomic::Ordering::Acquire));
        let projected=restore.checked_add(bake).ok_or(GpuRasterError::SizeOverflow)?;
        let mut retained=self.retained_preparation_bytes(transforms)?;
        let mut bytes=projected.checked_add(retained).ok_or(GpuRasterError::SizeOverflow)?;
        if bytes>limit.max(retained) {
            if self.factory.device_memory_limit().is_some_and(|limit|projected.saturating_add(self.pinned_paint_bytes()).saturating_add(self.source_tiles.borrow().non_slot_bytes())>limit) {
                self.preparation_memory(0);
                return Err(GpuRasterError::SourceWorkingSetExceeded);
            }
            let encoder=submission::CommandEncoder::new(&self.device,&Default::default());
            let mut retired=0u64;
            let mut pages=Vec::new();
            if bytes>limit.max(retained) && !transforms.is_some_and(|transforms|transforms.has_preview()) {
                loop {
                    let batch=self.trim_native_color_cache_to(&[],&[],0);
                    if batch.is_empty() {break;}
                    pages.extend(batch);
                }
                self.refresh_storage_metrics();
                retired=retired.checked_add(pages.iter().map(PageSurface::storage_bytes).sum::<u64>()).ok_or(GpuRasterError::SizeOverflow)?;
                retained=self.retained_preparation_bytes(transforms)?;
                bytes=projected.checked_add(retained).ok_or(GpuRasterError::SizeOverflow)?;
            }
            if retired!=0 {
                self.retired_display_bytes.fetch_add(retired,std::sync::atomic::Ordering::AcqRel);
                let charge=DisplayRetirement {bytes:retired,charge:self.retired_display_bytes.clone(),
                    #[cfg(not(target_arch="wasm32"))] _scratch:(pages,Vec::new())};
                #[cfg(not(target_arch="wasm32"))]
                let wake=self.preparation_wake.clone();
                encoder.on_submitted_work_done(move||{drop(charge);#[cfg(not(target_arch="wasm32"))] if let Some(wake)=wake {wake();}});
                self.last_submission=Some(encoder.submit(&self.queue));
                self.publish_memory(true);
            }
            self.preparation_memory(bytes);
            self.factory.memory_budget().flush_reports();
            let [scene, raster] = self.preparation_memory_limits();
            let retiring=self.retired_display_bytes.load(std::sync::atomic::Ordering::Acquire);
            let fresh = scene.checked_add(raster).ok_or(GpuRasterError::SizeOverflow)?.saturating_sub(retiring);
            if bytes>fresh.max(retained) && retiring==0 {
                self.preparation_memory(0);
                return Err(GpuRasterError::SourceWorkingSetExceeded);
            }
            Err(GpuRasterError::NotReady)
        } else {Ok(())}
    }
    pub(crate) fn staged_restore_bytes(&self)->u64 {
        self.restore_preflight.as_ref().map_or(0,|restore|restore.targets.iter().flatten().map(|target|target.pages.storage_bytes()).sum::<u64>()
            +restore.starts.iter().map(|(_,_,_,_,page)|page.storage_bytes()).sum::<u64>())
    }
    pub(crate) fn projected_restore_bytes(&self)->u64 {self.restore_preflight.as_ref().map_or(0,|restore|restore.targets.iter().flatten().try_fold(0u64,|bytes,target|bytes.checked_add(target.projected_bytes(self)?).ok_or(GpuRasterError::SizeOverflow)).unwrap_or(u64::MAX))}
    pub(crate) fn staged_transform_source(&self,target:SourceTarget)->Option<(&Arc<RasterData>,&PreparedPages)> {
        self.restore_preflight.as_ref()?.targets.iter().flatten().find(|entry|entry.replace && entry.target==target && entry.complete()).map(|entry|(&entry.data,&entry.pages))
    }
    pub(crate) fn prepare_transform_preview(&mut self,packet:FramePacket<'_>,preview:Option<&layer_render::TransformPreview>)->bool {
        if self.snapshot_worker {return true;}
        let Some(preview)=preview else {
            self.transforms.as_mut().unwrap().cancel_preparation();return true;
        };
        let request=match scene::scale::request(self,packet) {Ok(request)=>request,Err(_)=>return true};
        if request.plan.level==0 || request.evaluation==scene::scale::Evaluation::Native {self.transforms.as_mut().unwrap().cancel_preparation();return true;}
        let pipelines=if request.evaluation.fine_capacity().is_some() {&self.scene_pipelines.fine_resample} else {&self.scene_pipelines.resample};
        let entries:&[usize]=if preview.transform.placement.mesh.is_some() {&[0,1,2,3]} else {&[4,5]};
        if !self.compilation.require(entries.iter().map(|&i|&pipelines.mesh[i]),startup::BRUSH) {return false;}
        let mut transforms=self.transforms.take().unwrap();
        let result=self.record_restore_uploads(|r,encoder|transforms.prepare_preview(r,encoder,preview,packet.scene,request.plan.level,request.evaluation.fine_capacity().is_some()));
        self.transforms=Some(transforms);
        match result {
            Ok(())=>true,
            Err(error) if error.is_preparation_pending()=>false,
            Err(error)=>{if let Some(restore)=&mut self.restore_preflight {restore.failure=Some(error);}true},
        }
    }
    fn record_restore_uploads<T>(&mut self,work:impl FnOnce(&mut Self,&mut submission::CommandEncoder)->Result<T,GpuRasterError>)->Result<T,GpuRasterError> {
        let mut encoder=submission::CommandEncoder::new(&self.device,&Default::default());
        std::mem::swap(&mut self.uploads,&mut self.restore_uploads);
        let result=work(self,&mut encoder);
        if encoder.has_work() {
            self.preparation_progress=self.preparation_progress.saturating_add(1);
            #[cfg(not(target_arch="wasm32"))]
            if let Some(wake)=self.preparation_wake.clone() {encoder.on_submitted_work_done(move||wake());}
            self.uploads.finish(&encoder);self.last_submission=Some(encoder.submit(&self.queue));self.metrics.native_restore_submissions+=1;
        }
        std::mem::swap(&mut self.uploads,&mut self.restore_uploads);result
    }
    fn restore_pipelines_ready(&self,kinds:startup::SourceKinds)->bool {
        self.compilation.require(kinds.pipelines(&self.scene_pipelines.source),startup::VALIDATION)
    }
    pub(crate) fn prepare_raster_uploads(&mut self,requests:&[(Arc<TileBlob>,layer_core::color::RgbSpace,layer_core::color::RgbSpace)])->Result<(),GpuRasterError> {
        if self.snapshot_worker {return Ok(());}
        let mut kinds=startup::SourceKinds::default();
        for (blob,..) in requests {kinds.include(startup::SourceKinds::for_descriptor(blob.descriptor));}
        if !self.restore_pipelines_ready(kinds) {return Err(GpuRasterError::NotReady);}
        let ready=self.record_restore_uploads(|r,encoder| {
            r.drain_image_decodes(encoder)?;let mut ready=true;
            for (blob,source,destination) in requests {
                let view=r.source_tiles.borrow().prepared_raster_any_view(blob,*source,*destination).cloned();
                if let Some(view)=view {
                    scene::Scene::promote_source(r,encoder,source_access::RawTile {texture:view.texture().clone(),view})?;
                } else {
                    ready=false;if !r.request_raster_decode_to(blob.clone(),*source,*destination)? {break;}
                }
            }
            Ok(ready)
        })?;
        if ready {Ok(())}else {Err(GpuRasterError::NotReady)}
    }
    pub(crate) fn restore_preparation_pending(&self)->bool {self.restore_preflight.as_ref().is_some_and(|restore|!restore.complete())}
    pub(crate) fn restore_pages_ready(&self)->bool {self.restore_preflight.as_ref()
        .and_then(|restore|restore.targets.iter().flatten().find(|target|!target.complete())).is_some_and(|target|target.ready(self))}
    fn replace_restore(&mut self,key:Key,requests:Vec<Request>,commit:Option<paint_transform::PreparedTransformCommit>) {
        if self.restore_preflight.as_ref().is_none_or(|restore|restore.key!=key || restore.consumed) {
            self.retire_restore();
            self.restore_preflight=Some(RestorePreflight::new(self,key,requests,commit));
        }
    }
    fn accept_restore_inputs(&mut self)->Result<bool,GpuRasterError> {
        let Some(index)=self.restore_preflight.as_ref().and_then(|restore|restore.targets.iter().position(|target|target.as_ref().is_some_and(|target|!target.complete()))) else {return Ok(true);};
        let target=self.restore_preflight.as_ref().unwrap().targets[index].as_ref().unwrap();
        let first=target.next;
        let remaining=PREFETCH-usize::from(!matches!(target.pending[first],SourcePage::Output(_)));
        let indices=std::iter::once(first).chain(target.pending.iter().enumerate().skip(first)
            .filter(|(index,source)|*index!=first && !matches!(source,SourcePage::Output(_))).take(remaining).map(|(index,_)|index)).collect::<Vec<_>>();
        for at in indices {
            let target=self.restore_preflight.as_ref().unwrap().targets[index].as_ref().unwrap();
            let Some(inputs)=target.pending[at].inputs(self.document_color().space)? else {break;};
            if target.pages.storage_bytes()!=0 && target.inputs.contains(&inputs) {continue;}
            let output_bytes=target.projected_bytes(self)?;
            let accepted=inputs.accept(self,&mut (),|_,r,inputs| {
                let raster=if output_bytes==0 {None}else {Some(r.admit_raster_working(output_bytes)?)};
                let bytes=inputs.additional_bytes(&r.source_tiles.borrow());
                r.memory.admit_working(layer_core::MemoryOwnerKind::Scene,r.current_display_memory_report(),bytes)
                    .map(|source|((source,raster),bytes))
            },|_,r| {
                if output_bytes!=0 {r.record_restore_uploads(|r,encoder| {
                    let mut restore=r.restore_preflight.take().unwrap();
                    restore.targets[index].as_mut().unwrap().allocate(r,encoder)?;
                    r.restore_preflight=Some(restore);Ok(())
                })?;}
                Ok(())
            },|_,r,(source,raster)| {
                let source=r.memory.commit_reservation(layer_core::MemoryOwnerKind::Scene,source,r.current_display_memory_report())
                    .map_err(|error|GpuRasterError::Color(error.into()));
                let raster=raster.map_or(Ok(()),|reservation|r.commit_raster_working(reservation));
                source.and(raster)
            });
            match accepted {
                Ok(Some(((),inputs)))=>self.restore_preflight.as_mut().unwrap().targets[index].as_mut().unwrap().inputs.extend(inputs),
                Ok(None)=>break,
                Err(error) if error.is_preparation_pending()=>break,
                Err(error)=>return Err(error),
            }
        }
        let target=self.restore_preflight.as_ref().unwrap().targets[index].as_ref().unwrap();
        Ok(target.pending[first].inputs(self.document_color().space)?.is_some_and(|inputs|
            target.inputs.contains(&inputs) && target.projected_bytes(self).is_ok_and(|bytes|bytes==0)))
    }
    fn advance_restore(&mut self)->Result<bool,GpuRasterError> {
        if let Some(target)=self.restore_preflight.as_ref().and_then(|restore|restore.targets.iter().flatten().find(|target|!target.complete()))
            && !self.restore_pipelines_ready(target.source_kinds()) {return Ok(false);}
        if self.restore_preflight.is_none() {return Ok(true);}
        if !self.accept_restore_inputs()? {return Ok(false);}
        let result=self.record_restore_uploads(|r,encoder| {
            r.drain_image_decodes(encoder)?;
            let mut restore=r.restore_preflight.take().unwrap();
            let result=restore.targets.iter_mut().flatten().find(|target|!target.complete()).map_or(Ok(()),|target|target.advance(r,encoder));
            r.restore_preflight=Some(restore);result
        });
        let restore=self.restore_preflight.as_mut().unwrap();
        if let Err(error)=result {
            if error.is_preparation_pending() {return Ok(false);}
            restore.failure=Some(error);
        }
        Ok(restore.complete())
    }
    fn restore_ready(&mut self)->bool {
        match self.advance_restore() {
            Ok(ready)=>ready,
            Err(error) if error.is_preparation_pending()=>false,
            Err(error)=>{self.restore_preflight.as_mut().unwrap().failure=Some(error);true},
        }
    }
    pub(crate) fn hold_restore_scalar(&mut self,prepared:object_image_mips::PreparedRasterTile) {
        if let Some(target)=self.restore_preflight.as_mut().and_then(|restore|restore.targets.iter_mut().flatten().find(|target|!target.complete()))
            && target.awaits(&prepared.blob) && target.held(&prepared.blob).is_none() {target.scalars.push(prepared);}
    }
    pub(crate) fn prepare_raster_restores(&mut self,packet:FramePacket<'_>)->bool {
        if self.snapshot_worker && packet.dab_batches.is_empty() {return self.raster_restore_backing_ready(packet);}
        let mut plans=match brush_tiles::plan_packet(packet,|target|packet.scene.target_extent(target)) {Ok(plans)=>plans,Err(_)=>return true,};
        liquify::Liquify::extend_required_plans(self,packet,&mut plans);
        self.prepare_raster_outputs(packet,&plans)
    }
    pub(crate) fn prepare_raster_outputs(&mut self,packet:FramePacket<'_>,plans:&[Vec<BrushTile>])->bool {
        let retained=source_access::retained_targets(packet.scene);
        let reset=packet.reset_layers||packet.document_extent!=self.document_extent;
        let mut required:BTreeMap<SourceTarget,BTreeSet<[u32;2]>>=BTreeMap::new();
        for (index,(batch,tiles)) in packet.dab_batches.iter().zip(plans).enumerate() {
            if matches!(batch.target,SourceTarget::Paint(_)) {
                let coordinates=required.entry(batch.target).or_default();
                coordinates.extend(tiles.iter().flat_map(|tile|source_access::neighborhood(tile.coordinate,self.material_offsets(batch))));
                coordinates.extend(tiles.iter().map(|tile|tile.coordinate));
                if BrushPassPlan::for_device(&batch.style,&self.device).state.watercolor_wetness && is_last_watercolor_update_batch(packet.dab_batches,index) {
                    coordinates.extend(watercolor_update_damages(packet.dab_batches,index,packet.scene.target_extent(batch.target)).into_iter()
                        .flat_map(page_coordinates).flat_map(|coordinate|source_access::neighborhood(coordinate,[[0,0],[-1,0],[1,0],[0,-1],[0,1]].map(Some))));
                }
                if let DabBatchKind::RasterOperation(index)=batch.kind {
                    let extent=packet.scene.target_extent(batch.target);let bounds=batch_pixel_rect(batch,extent);
                    let bounds=packet.scene.operations(batch.target).and_then(|operations|operations.get(index as usize))
                        .map_or(bounds,|operation|scene::operation_read_bounds(operation,bounds,extent));
                    coordinates.extend(page_coordinates(bounds));
                }
            }
        }
        for batch in packet.dab_batches.iter().filter(|batch|batch.target.is_coverage()) {
            required.entry(batch.target).or_default().extend(page_coordinates(batch_pixel_rect(batch,packet.scene.target_extent(batch.target))));
        }
        if let Some(fill)=&self.fill_preview {
            let extent=packet.scene.target_extent(fill.target);
            let bounds=scene::operation_read_bounds(&fill.operation,pixel_rect(fill.operation.bounds(extent),extent),extent);
            required.entry(fill.target).or_default().extend(page_coordinates(bounds));
        }
        liquify::Liquify::required_sources(self,packet,plans,&mut required);
        self.required_retouch_sources(packet,plans,&mut required);
        for batch in packet.dab_batches {
            if let DabBatchKind::RasterOperation(index)=batch.kind
                && let Some(operation)=packet.scene.operations(batch.target).and_then(|operations|operations.get(index as usize))
                && let layer_core::RasterOperationKind::FrequencyDetail {low,..}=&operation.kind {
                required.entry(SourceTarget::Paint(*low)).or_default().extend(page_coordinates(batch_pixel_rect(batch,packet.scene.target_extent(batch.target))));
            }
        }
        let roots=retained.iter().filter_map(|(target,scene)| {
            let root=scene.raster(*target)?;
            Some(FrameRoot {target:*target,revision:root.identity(),published:root.try_data().and_then(Result::ok).map(|data|Arc::as_ptr(&data) as usize),
                restore:packet.restore_rasters.iter().find(|(id,_)|id==target).map(|(_,root)|root.identity()),extent:scene.target_extent(*target)})
        }).collect();
        let bases=retained.iter().filter_map(|(target,scene)|scene.paint_base(*target).map(|base|(*target,Arc::as_ptr(base.image.storage()) as usize,base.offset))).collect();
        let outputs=self.raster_outputs(packet,plans);
        for (target,output) in outputs.iter().filter(|(_,output)|!output.preview()) {required.entry(*target).or_default().insert(output.coordinate());}
        let clear_starts=packet.dab_batches.iter().zip(plans).any(|(batch,tiles)|self.clears_stroke_starts(batch,tiles,packet.scene.target_extent(batch.target)));
        let key=Key::Frame {color:self.document_color(),extent:packet.document_extent,reset,clear_starts,roots,bases,
            required:required.iter().map(|(target,coordinates)|(*target,coordinates.iter().copied().collect())).collect(),outputs};
        if self.restore_preflight.as_ref().is_some_and(|restore|restore.key==key && !restore.consumed && restore.commit.as_ref().is_none_or(|commit|commit.matches(self,packet))) {
            if !self.prepare_native_admission_for_frame(packet) {return false;}
            self.refresh_memory_limits();return self.restore_ready();}
        self.retire_restore();
        let commit=match self.transforms.as_ref().unwrap().prepare_commit(self,packet) {
            Ok(commit)=>commit,Err(error)=>{self.restore_preflight=Some(RestorePreflight {key,commit:None,native:None,targets:Vec::new(),failure:Some(error),consumed:false,starts:Vec::new()});return true;},
        };
        let mut requests=Vec::new();
        for (target,scene) in retained {
            let Some(root)=scene.raster(target) else {continue;};
            let current=self.raster.as_ref().and_then(|runtime|runtime.targets.get(&target));
            let explicit=packet.restore_rasters.iter().find(|(id,_)|*id==target).map(|(_,root)|root);
            let required=required.remove(&target).unwrap_or_default();
            let replace=reset || explicit.is_some() || current.is_none_or(|current|current.revision!=*root);
            let wanted=match root.try_data() {
                Some(Ok(data))=>Some(data),Some(Err(_))=>return true,None=>None,
            };
            let resolved=wanted.is_some();
            let data=if let Some(data)=wanted {
                if !replace && required.is_empty() {continue;}
                data
            } else if let Some(root)=explicit {
                match root.try_data() {Some(Ok(data))=>data,Some(Err(_))=>return true,None=>return false,}
            } else if reset && let Some(current)=current {current.contents.data().clone()
            } else if !required.is_empty() {current.map_or_else(||Arc::new(RasterData::default()),|current|current.contents.data().clone())}else {continue;};
            let replace=if let Some(current)=current {reset || explicit.is_some() || resolved && current.revision!=*root}else {!data.tiles.is_empty()};
            if !replace && required.is_empty() {continue;}
            let before=if reset {RasterData::default()}else {current.map_or_else(RasterData::default,|current|current.contents.restorable_data())};
            requests.push(Request {target,extent:scene.target_extent(target),before,data,required,base:scene.paint_base(target).cloned(),replace});
        }
        self.replace_restore(key,requests,commit);
        if !self.prepare_native_admission_for_frame(packet) {return false;}
        self.refresh_memory_limits();self.restore_ready()
    }
    fn prepare_native_admission_for_frame(&mut self,packet:FramePacket<'_>)->bool {
        if self.native_edit.is_none() || self.restore_preflight.as_ref().is_some_and(|restore|restore.native.is_some()) {return true;}
        let outputs=match self.restore_preflight.as_ref().map(|restore|&restore.key) {
            Some(Key::Frame {roots,..}) if roots.iter().all(|root|root.published.is_some())=>return true,
            Some(Key::Frame {outputs,..})=>outputs.clone(),_=>return true,
        };
        match self.prepare_native_admission(packet.scene,&outputs) {
            Ok(admission)=>{self.restore_preflight.as_mut().unwrap().native=Some(admission);true},
            Err(error) if error.is_preparation_pending()=>false,
            Err(error)=>{self.restore_preflight.as_mut().unwrap().failure=Some(error);true},
        }
    }
    pub(crate) fn has_transform_commit(&self)->bool {self.restore_preflight.as_ref().is_some_and(|restore|restore.commit.is_some())}
    pub(crate) fn take_transform_commit(&mut self)->Option<paint_transform::PreparedTransformCommit> {self.restore_preflight.as_mut()?.commit.take()}
    pub(super) fn consume_restore_preflight(&mut self) {if let Some(restore)=&mut self.restore_preflight {restore.consumed=true;}}
    pub(crate) fn take_restore_failure(&mut self)->Option<GpuRasterError> {self.restore_preflight.as_mut().and_then(|restore|restore.failure.take())}
    pub(super) fn validate_restore_publication(&self)->Result<(),GpuRasterError> {
        if let Some(restore)=&self.restore_preflight {
            if !restore.complete() {return Err(GpuRasterError::NotReady);}
            for target in restore.targets.iter().flatten() {
                if !target.target.is_coverage() && !self.paint_layers.iter().any(|layer|layer.id==target.target) {return Err(GpuRasterError::MissingPaintLayer(target.target));}
            }
        }
        Ok(())
    }
    pub(crate) fn validate_raster_outputs(&self)->Result<(),GpuRasterError> {
        let Some(restore)=&self.restore_preflight else {return Ok(());};
        let Key::Frame {outputs,..}=&restore.key else {return Ok(());};
        for (target,output) in outputs {
            let coordinate=output.coordinate();
            let layer=self.paint_layers.iter().find(|layer|layer.id==*target);
            let page=layer.and_then(|layer|layer.pages.iter().find(|page|page.coordinate==coordinate));
            let ready=match output {
                Output::Color(_)=>page.is_some(),
                Output::Companion(_)=>page.is_some_and(|page|page.secondary.is_some()),
                Output::Coverage(_,slabs,rgba)=>layer.is_some_and(|layer|layer.coverage_pages.iter().any(|page|
                    page.coordinate==coordinate && page.primary.size().height==PAGE_SIZE*slabs
                        && (page.primary.format()==wgpu::TextureFormat::Rgba32Float)==*rgba)),
                Output::Wetness(_)=>layer.is_some_and(|layer|layer.watercolor_wetness_pages.iter().any(|page|page.coordinate==coordinate)),
                Output::Mask(..)=>self.layer_masks.pages.contains_key(&(*target,coordinate)),
                Output::StrokeStart(_,stroke,preview)=>!*preview && page.is_none()
                    || page.is_some_and(|page|page.stroke_start.as_ref().is_some_and(|start|start.state.stroke==*stroke))
                    || restore.starts.iter().any(|(id,c,s,p,_)|id==target && *c==coordinate && s==stroke && (*preview || p==preview)),
                output if output.preview()=>output.preview_present(self,self.preview_level),
                _=>unreachable!(),
            };
            prepared_raster_output(ready.then_some(()))?;
        }
        Ok(())
    }
    pub(super) fn publish_prepared_restore(&mut self,target:SourceTarget,data:&Arc<RasterData>)->Result<(),GpuRasterError> {
        if let Some(error)=self.take_restore_failure() {return Err(error);}
        self.validate_restore_publication()?;
        let restore=self.restore_preflight.as_mut().ok_or(GpuRasterError::Invariant("prepared publish prepared restore state is inconsistent"))?;
        let index=restore.targets.iter().position(|entry|entry.as_ref().is_some_and(|entry|entry.replace && entry.target==target && Arc::ptr_eq(&entry.data,data)))
            .ok_or(GpuRasterError::Invariant("prepared publish prepared restore state is inconsistent"))?;
        if !restore.complete() {return Err(GpuRasterError::NotReady);}
        let target=restore.targets[index].take().unwrap();restore.consumed=true;
        self.publish_restored_pages(target.target,&target.selected,target.pages)?;
        if let Some(native)=&mut self.native_edit {native.backing.insert(target.target,data.clone());}
        Ok(())
    }
    pub(super) fn publish_warm_restore_pages(&mut self)->Result<(),GpuRasterError> {
        self.validate_restore_publication()?;
        let Some(restore)=&mut self.restore_preflight else {return Ok(());};
        let outputs=restore.targets.iter_mut().filter_map(|entry|entry.as_ref().is_some_and(|target|!target.replace)
            .then(||entry.take().unwrap())).collect::<Vec<_>>();
        if !outputs.is_empty() {restore.consumed=true;}
        for target in outputs {self.append_prepared_pages(target.target,target.pages)?;}
        Ok(())
    }
    pub(super) fn append_prepared_pages(&mut self,target:SourceTarget,pages:PreparedPages)->Result<(),GpuRasterError> {
        let PreparedPages {colors,masks,wetness,coverage,companions,starts}=pages;
        if !colors.is_empty() || !wetness.is_empty() || !coverage.is_empty() || !companions.is_empty() {
            let layer=self.paint_layers.iter_mut().find(|layer|layer.id==target).ok_or(GpuRasterError::MissingPaintLayer(target))?;
            for page in colors {layer.pages.retain(|old|old.coordinate!=page.coordinate);layer.pages.push(page);}
            for page in wetness {layer.watercolor_wetness_pages.retain(|old|old.coordinate!=page.coordinate);layer.watercolor_wetness_pages.push(page);}
            layer.coverage_pages.extend(coverage);
            for (c,surface) in companions {layer.pages.iter_mut().find(|page|page.coordinate==c).ok_or(GpuRasterError::InvalidExtent)?.secondary=Some(surface);}
        }
        for (c,page) in masks {self.layer_masks.pages.insert((target,c),page);}
        if !starts.is_empty() {self.restore_preflight.as_mut().ok_or(GpuRasterError::InvalidExtent)?.starts.extend(starts.into_iter().map(|(c,id,preview,page)|(target,c,id,preview,page)));}
        Ok(())
    }
    pub(super) fn restore_direct(&mut self,target:SourceTarget,before:&RasterData,data:&RasterData)->Result<(),GpuRasterError> {
        let key=Key::Direct {color:self.document_color(),target,extent:self.target_extent(target),before:DataKey::new(before),after:DataKey::new(data)};
        if self.restore_preflight.as_ref().is_none_or(|restore|restore.key!=key || restore.consumed) {
            self.replace_restore(key,vec![Request {target,extent:self.target_extent(target),before:before.clone(),data:Arc::new(data.clone()),
                required:BTreeSet::new(),base:None,replace:true}],None);
        }
        if !self.advance_restore()? {
            return Err(GpuRasterError::NotReady);
        }
        if let Some(error)=self.take_restore_failure() {return Err(error);}
        let data=self.restore_preflight.as_ref().unwrap().targets.iter().flatten().find(|entry|entry.target==target).unwrap().data.clone();
        self.publish_prepared_restore(target,&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resident_destination_outside_the_accepted_unit_never_encodes() {
        let extent=[2*PAGE_SIZE,PAGE_SIZE];
        let document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Ink".into(),paper:"Paper".into()});
        let target=document.active_target().unwrap();
        let mut r=WgpuRasterizer::new_native_headless(document.composition().color).unwrap();
        let mut seed=crate::tests::test_dab([PAGE_SIZE as f32,128.],[0.25,0.5,0.75,1.],1.);seed.radii=[300.;2];
        let style=crate::tests::test_style(BrushExecution::Dry);
        let seed_batch=crate::test_support::dab_batch(target,style.clone(),seed.bounds());
        let frame=crate::test_support::packet(document.scene(),extent);
        crate::test_support::submit_ready(&mut r,FramePacket {dabs:&[seed],dab_batches:&[seed_batch],..frame}).unwrap();
        assert!(r.paint_layers.iter().find(|layer|layer.id==target).unwrap().pages.iter().any(|page|page.coordinate==[1,0]));
        let dab=crate::tests::test_dab([64.,64.],[1.,0.,0.,1.],1.);
        let batch=crate::test_support::dab_batch(target,style,dab.bounds());
        let packet=FramePacket {dabs:std::slice::from_ref(&dab),dab_batches:std::slice::from_ref(&batch),reset_layers:false,composite_all:false,..frame};
        crate::test_support::prepare_raster(&mut r,|r|if r.raster_dependencies_ready(packet,None) {Ok(())}else {Err(GpuRasterError::NotReady)}).unwrap();
        let tile=BrushTile {coordinate:[1,0],local:PixelRect::full([PAGE_SIZE;2]),dabs:0..1,indices:Vec::new()};
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        let before=crate::test_support::PassCounts::snapshot();
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||r.encode_material_batch(&mut encoder,0,&batch,
            BrushEncodingContext {source:packet.scene,batches:std::slice::from_ref(&batch),dabs:std::slice::from_ref(&dab),tiles:std::slice::from_ref(&tile),target_extent:extent,target:BrushEncodingTarget::Persistent})));
        if cfg!(debug_assertions) {assert!(result.is_err());}
        else {assert!(matches!(result,Ok(Err(GpuRasterError::Invariant(_)))));}
        assert_eq!(crate::test_support::PassCounts::snapshot().delta(before).raster_output_misses,1);
        assert!(!encoder.has_work(),"unaccepted output must wait before any unit commands encode");
    }

    #[test]
    fn cold_authored_wetness_is_restored_instead_of_cleared() {
        let extent=[PAGE_SIZE;2];
        let doc=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Ink".into(),paper:"Paper".into()});
        let mut r=crate::memory::memory_tests::renderer(doc.composition().color,Default::default()).unwrap();
        r.ensure_document_metadata(extent,doc.scene()).unwrap();
        let target=doc.active_target().unwrap();let coordinate=[0;2];
        let key=TileKey {plane:RasterPlane::WatercolorWetness,coordinate};
        let blob=TileBlob::encode(key.plane.descriptor(doc.composition().color),&vec![93;PAGE_SIZE.pow(2) as usize]).unwrap();
        let data=Arc::new(RasterData {tiles:[(key,RasterTile::backed(blob))].into(),
            watercolor:Some(RasterWatercolor {wet_edge:0.5,burnt_edge:0.5,edge_width:2.})});
        let mut restore=TargetRestore::new(&mut r,Request {target,extent,before:(*data).clone(),data,
            required:[coordinate].into(),base:None,replace:false},None).unwrap();
        restore.add_outputs(&r,&[(target,Output::Wetness(coordinate))],false,false,None);
        assert_eq!(restore.pending.len(),1);
        assert!(matches!(&restore.pending[0],SourcePage::Raster(key,_) if key.plane==RasterPlane::WatercolorWetness));
    }
}
