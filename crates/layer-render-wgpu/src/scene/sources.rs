//! GPU decoded tile slots have fixed ownership. Jobs defer decoding until their
//! ordered upload; eviction never creates another retained GPU tile. Built-in
//! profiles decode on the GPU, while embedded profiles use the native CMM.
use super::*;
use crate::source_access::RawTile;
use layer_core::color::{
    AlphaAssociation, ColorProfile, SampleDepth, PixelDescriptor, RgbSpace, TransferEncoding,
    source::{SourceChannels, SourceImage},
};
use layer_core::raster::TileBlob;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
pub(crate) type RequiredSource = (Arc<SourceImage>, [u32; 2]);
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum SourceUse { Display, Exact }
pub(crate) const NORMALIZED_PAGE_BYTES: u64 = PAGE_SIZE as u64 * PAGE_SIZE as u64 * 16;
const DECODERS: usize = 4;
use crate::native_tiles::transfer;

#[derive(Clone, Copy, Default)]
struct SourceLimits {
    bytes: u64,
    upload_bytes: u64,
}
impl SourceLimits {
    fn admitted(decoded_bytes: u64, upload_bytes: u64) -> Self {
        Self { bytes: decoded_bytes, upload_bytes }
    }
}

#[derive(Clone,PartialEq,Eq,Hash)]
enum RasterIdentity {
    Encoded([u8; 32]),
    Owner(u64),
}
impl RasterIdentity {
    fn of(blob: &TileBlob) -> Self {
        blob.encoded_fingerprint().map_or_else(|| Self::Owner(blob.owner_identity()), Self::Encoded)
    }
}
pub(crate) fn same_raster_samples(a: &TileBlob, b: &TileBlob) -> bool { RasterIdentity::of(a) == RasterIdentity::of(b) }
#[derive(Clone)]
enum Key {
    Image(Weak<SourceImage>, [u32; 2]),
    PaintBase(Weak<SourceImage>, [u32; 2], [u32; 2]),
    Raster(RasterIdentity, RgbSpace, RgbSpace, PixelDescriptor),
}
impl PartialEq for Key {fn eq(&self,other:&Self)->bool {self.matches(other)}}
impl Eq for Key {}
impl std::hash::Hash for Key {
    fn hash<H:std::hash::Hasher>(&self,state:&mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Image(source,coordinate)=>(source.as_ptr(),coordinate).hash(state),
            Self::PaintBase(source,offset,coordinate)=>(source.as_ptr(),offset,coordinate).hash(state),
            Self::Raster(identity,space,destination,descriptor)=> {
                (identity,space,destination,descriptor.channels,descriptor.bits_per_channel).hash(state);
                std::mem::discriminant(&descriptor.sample).hash(state);
                std::mem::discriminant(&descriptor.encoding).hash(state);
                std::mem::discriminant(&descriptor.alpha).hash(state);
            },
        }
    }
}
#[cfg(test)]
std::thread_local! {
    static READS:std::cell::RefCell<Vec<Key>>=const {std::cell::RefCell::new(Vec::new())};
    static KEY_COMPARISONS:std::cell::Cell<Option<u64>>=const {std::cell::Cell::new(None)};
}
#[cfg(test)]
std::thread_local! {
    static ACCEPTED_READ_SET:std::cell::RefCell<Vec<Key>>=const {std::cell::RefCell::new(Vec::new())};
    static ACCEPTED_READ_RESULTS:std::cell::RefCell<Option<Vec<(usize,Vec<String>)>>>=const {std::cell::RefCell::new(None)};
}
#[derive(Default)]
pub(crate) struct Inputs {
    tiles: Vec<(Key,Option<SourceColor>,Option<Pixels>)>,
    bases: Vec<(layer_core::authored::PaintBase,[u32;2])>,
}
impl Inputs {
    pub(crate) fn extend(&mut self,other:Self) {
        for (base,coordinate) in other.bases {
            if !self.tiles.iter().any(|(key,_,_)|key.matches(&Key::base(&base,coordinate))) {self.bases.push((base,coordinate));}
        }
        for (key,color,pixels) in other.tiles {self.insert(key,color,pixels);}
    }
    pub(crate) fn accept<C,T,R>(&self,r:&mut WgpuRasterizer,context:&mut C,
        reserve:impl FnOnce(&mut C,&mut WgpuRasterizer,&Self)->Option<(R,u64)>,
        materialize:impl FnOnce(&mut C,&mut WgpuRasterizer)->Result<T,GpuRasterError>,
        commit:impl FnOnce(&mut C,&mut WgpuRasterizer,R)->Result<(),GpuRasterError>,
    )->Result<Option<(T,AcceptedInputs)>,GpuRasterError> {
        let Some((reservation,bytes))=reserve(context,r,self) else {return Ok(None);};
        let result=self.acquire(r,bytes).and_then(|inputs|materialize(context,r).map(|output|(output,inputs)));
        commit(context,r,reservation)?;
        result.map(Some)
    }
    fn insert(&mut self,key:Key,color:Option<SourceColor>,pixels:Option<Pixels>) {
        if !self.tiles.iter().any(|(stored,_,_)|stored.matches(&key)) {self.tiles.push((key,color,pixels));}
    }
    pub(crate) fn image(&mut self,source:&Arc<SourceImage>,coordinate:[u32;2],destination:RgbSpace) {
        self.insert(Key::Image(Arc::downgrade(source),coordinate),builtin_settings(source,coordinate,destination),Some(Pixels::Image(source.clone(),coordinate)));
    }
    pub(crate) fn raster(&mut self,blob:Arc<TileBlob>,space:RgbSpace,destination:RgbSpace) {
        let mut color=rgb_settings(space,destination,blob.descriptor.depth(),[PAGE_SIZE;2],blob.descriptor.alpha);
        color.scalar=u32::from(blob.descriptor.channels==1);color.scalar_scale=if color.scalar!=0 {1./blob.descriptor.depth().maximum() as f32}else {0.};
        self.insert(Key::raster(&blob,space,destination),Some(color),Some(Pixels::Raster(blob,space,destination)));
    }
    pub(crate) fn base(&mut self,base:&layer_core::authored::PaintBase,coordinate:[u32;2],destination:RgbSpace) {
        let bounds=crate::source_access::paint_base_source_bounds(base,coordinate);
        if bounds.is_empty() {return;}
        if base.offset.iter().any(|offset|offset%PAGE_SIZE!=0) {
            let key=Key::PaintBase(Arc::downgrade(base.image.storage()),base.offset,coordinate);
            if !self.tiles.iter().any(|(stored,_,_)|stored.matches(&key)) {self.bases.push((base.clone(),coordinate));}
            self.insert(key,None,None);
        }
        for coordinate in page_coordinates(bounds) {self.image(base.image.storage(),coordinate,destination);}
    }
    pub(in crate::scene) fn color(&mut self,r:&WgpuRasterizer,scene:SceneView<'_>,wanted:&super::SignatureFrame,consumer:Option<&crate::submission::Submission>,id:SourceTarget,coordinate:[u32;2])->Result<(),GpuRasterError> {
        if let Some(capture)=wanted.capture_for(id) {
            let transforms=r.transforms.as_ref().ok_or(GpuRasterError::NotReady)?;
            if transforms.read_native(r,id,layer_core::raster::RasterPlane::Color,coordinate,capture,consumer)?.is_some() {return Ok(());}
            if !transforms.display_source(id) {return Err(GpuRasterError::NotReady);}
        }
        let persistent=r.paint_layers.iter().find(|layer|layer.id==id).and_then(|layer|layer.pages.iter().find(|page|page.coordinate==coordinate));
        let preview=(r.preview_layer_id==Some(id) && r.preview_requires_base).then(||r.preview_page(coordinate)).flatten();
        if let Some(page)=preview.or(persistent) && r.read_scene_color_input(scene,id,page,preview.is_some(),Some(wanted),consumer)?.is_some() {return Ok(());}
        let space=r.document_color().space;
        if let Some(blob)=r.raster_tile_for(scene,id,layer_core::raster::RasterPlane::Color,coordinate)? {self.raster(blob,space,space);}
        else if let Some(base)=scene.paint_base(id) {self.base(base,coordinate,space);}
        Ok(())
    }
    pub(in crate::scene) fn scalar(&mut self,r:&WgpuRasterizer,scene:SceneView<'_>,wanted:&super::SignatureFrame,consumer:Option<&crate::submission::Submission>,id:SourceTarget,plane:layer_core::raster::RasterPlane,coordinate:[u32;2])->Result<(),GpuRasterError> {
        let resident=if plane==layer_core::raster::RasterPlane::Mask {
            let source=scene.source_owner(id).and_then(|owner|scene.mask(owner)).map(|(_,source)|source)
                .or_else(||r.layer_masks.definitions.get(&id)).ok_or(GpuRasterError::MissingPaintLayer(id))?;
            r.read_mask(id,source,coordinate,Some(wanted),consumer)?.is_some()
        } else {
            let predicted=r.preview_watercolor_wetness_pages.iter().find(|page|r.preview_layer_id==Some(id) && page.coordinate==coordinate
                && !r.preview_damage.intersect(page_rect(coordinate)).is_empty());
            let page=predicted.or_else(||r.paint_layers.iter().find(|layer|layer.id==id)
                .and_then(|layer|layer.watercolor_wetness_pages.iter().find(|page|page.coordinate==coordinate)));
            page.map(|page|r.read_scene_wetness_input(scene,id,page,predicted.is_some(),Some(wanted),consumer))
                .transpose()?.flatten().is_some()
        };
        if !resident && let Some(blob)=r.raster_tile_for(scene,id,plane,coordinate)? {let space=r.document_color().space;self.raster(blob,space,space);}
        Ok(())
    }
    pub(in crate::scene) fn region(&mut self,r:&WgpuRasterizer,packet:FramePacket<'_>,wanted:&super::SignatureFrame,consumer:Option<&crate::submission::Submission>,id:SourceTarget,bounds:PixelRect)->Result<(),GpuRasterError> {
        let owner=packet.scene.source_owner(id);
        let stored=r.paint_layers.iter().find(|layer|layer.id==id);
        let material=owner.is_some_and(|owner|world_offset(packet.scene,owner,false)!=[0;2])
            && r.watercolor_style(id,packet.dab_batches).is_some();
        for coordinate in page_coordinates(bounds) {
            if matches!(id,SourceTarget::Coverage(_)) {self.scalar(r,packet.scene,wanted,consumer,id,layer_core::raster::RasterPlane::Mask,coordinate)?;continue;}
            let preview=r.preview_layer_id==Some(id) && r.preview_page(coordinate).is_some();
            let watercolor=material || owner.is_some() && Scene::watercolor_nearby(r,packet,id,stored,coordinate,preview);
            let neighbors=if watercolor {[[0,0],[-1,0],[1,0],[0,-1],[0,1]].as_slice()}else {[[0,0]].as_slice()};
            for &[x,y] in neighbors {
                let Some(x)=coordinate[0].checked_add_signed(x) else {continue;};
                let Some(y)=coordinate[1].checked_add_signed(y) else {continue;};
                self.color(r,packet.scene,wanted,consumer,id,[x,y])?;
                if material || watercolor {self.scalar(r,packet.scene,wanted,consumer,id,layer_core::raster::RasterPlane::WatercolorWetness,[x,y])?;}
            }
        }
        Ok(())
    }
    pub(crate) fn additional_bytes(&self,cache:&DecodedTiles)->u64 {
        let missing=self.tiles.iter().filter(|(key,_,_)|!cache.slots.iter().any(|slot|
            slot.texture.format()==wgpu::TextureFormat::Rgba32Float && slot.valid.load(Ordering::Acquire)
                && slot.key.as_ref().is_some_and(|stored|stored.matches(key)))).count();
        let mut bytes=missing as u64*NORMALIZED_PAGE_BYTES;
        let mut formats=std::collections::HashSet::new();let mut tables=std::collections::BTreeSet::new();
        for (key,color,_) in &self.tiles {
            if cache.slots.iter().any(|slot|slot.texture.format()==wgpu::TextureFormat::Rgba32Float && slot.ready()
                && slot.key.as_ref().is_some_and(|stored|stored.matches(key))) {continue;}
            let Some(color)=color else {continue;};
            let format=color.input_format();
            if !cache.inputs.iter().any(|input|input.texture.format()==format) && formats.insert(format) {bytes+=u64::from(PAGE_SIZE).pow(2)*u64::from(format.block_copy_size(None).unwrap());}
            let space=color.space();
            if !cache.transfer.contains(space) && tables.insert(transfer::Tables::index(space)) {bytes+=transfer::TABLE_BYTES;}
        }
        bytes
    }
    #[cfg(test)]
    pub(in crate::scene) fn take_reads(&self)->(usize,usize) {
        READS.with(|reads| {let reads=std::mem::take(&mut *reads.borrow_mut());
            (reads.len(),reads.iter().filter(|read|!self.tiles.iter().any(|(key,_,_)|key.matches(read))).count())})
    }
    #[cfg(test)]
    pub(in crate::scene) fn begin_accepted_reads() {
        ACCEPTED_READ_RESULTS.with(|results|*results.borrow_mut()=Some(Vec::new()));
    }
    #[cfg(test)]
    pub(in crate::scene) fn take_accepted_read_results()->Vec<(usize,Vec<String>)> {
        ACCEPTED_READ_RESULTS.with(|results|results.borrow_mut().take().unwrap_or_default())
    }
    pub(crate) fn acquire(&self,r:&WgpuRasterizer,bytes:u64)->Result<AcceptedInputs,GpuRasterError> {
        let mut cache=r.source_tiles.borrow_mut();assert_eq!(cache.reserved_bytes,0);cache.reserved_bytes=bytes;
        let held=cache.slots.iter().filter(|slot|slot.valid.load(Ordering::Acquire)
            && slot.key.as_ref().is_some_and(|key|self.tiles.iter().any(|(input,_,_)|input.matches(key))))
            .map(|slot|Lease {owner:slot.lease.clone(),_encoded:slot.encoded.clone(),read:None}).collect::<Vec<_>>();
        drop(cache);
        let result=(|| {
            let mut accepted=Vec::new();
            for (key,color,pixels) in &self.tiles {
                let mut cache=r.source_tiles.borrow_mut();
                if let Some(color)=color && !cache.slots.iter().any(|slot|slot.texture.format()==wgpu::TextureFormat::Rgba32Float
                    && slot.ready() && slot.key.as_ref().is_some_and(|stored|stored.matches(key))) {cache.prepare_color(&r.device,*color)?;}
                let (tile,pending)=match pixels {
                    Some(Pixels::Image(source,coordinate))=>cache.plan_exact(r,source,*coordinate)?,
                    Some(Pixels::Raster(blob,space,destination))=>cache.plan_raster(r,blob,*space,*destination)?,
                    _=>continue,
                };
                let (tile,promotion)=cache.promote(r,tile)?;
                let mut lease=cache.lease(&tile.view).unwrap();
                lease.read=Some(Arc::new(Read {key:key.clone(),tile,pending:pending.into_iter().chain(promotion).collect(),assembly:None}));
                accepted.push(lease);
            }
            for (base,coordinate) in &self.bases {
                let Some(read)=crate::source_access::plan_paint_base(r,base,*coordinate)? else {continue;};
                let mut lease=r.source_tiles.borrow().lease(&read.tile.view).unwrap();
                lease.read=Some(Arc::new(Read {key:Key::base(base,*coordinate),tile:read.tile,pending:read.pending,assembly:read.assembly}));
                accepted.push(lease);
            }
            Ok(AcceptedInputs(accepted))
        })();
        r.source_tiles.borrow_mut().reserved_bytes=0;drop(held);
        #[cfg(test)]
        if result.is_ok() {ACCEPTED_READ_SET.with(|set|*set.borrow_mut()=self.tiles.iter().map(|(key,_,_)|key.clone()).collect());}
        result
    }
}
fn accepted(inputs:&[Lease],key:Key)->Result<source_access::SourceRead,GpuRasterError> {
    #[cfg(test)]
    if ACCEPTED_READ_RESULTS.with(|results|results.borrow().is_some()) {READS.with(|reads|reads.borrow_mut().push(key.clone()));}
    let read=inputs.iter().filter_map(|input|input.read.as_ref()).find(|read|read.key.matches(&key));
    #[cfg(test)]
    if read.is_none() {crate::test_support::PassCounts {source_input_misses:1,..Default::default()}.record();}
    debug_assert!(read.is_some(),"accepted input is missing");
    let read=read.ok_or(GpuRasterError::Invariant("accepted source input is missing"))?;
    Ok(read.source())
}
pub(crate) fn accepted_image(inputs:&[Lease],source:&Arc<SourceImage>,coordinate:[u32;2])->Result<source_access::SourceRead,GpuRasterError> {
    accepted(inputs,Key::Image(Arc::downgrade(source),coordinate))
}
pub(crate) fn accepted_raster(inputs:&[Lease],blob:&Arc<TileBlob>,space:RgbSpace,destination:RgbSpace)->Result<source_access::SourceRead,GpuRasterError> {
    accepted(inputs,Key::raster(blob,space,destination))
}
pub(crate) fn accepted_base(inputs:&[Lease],base:&layer_core::authored::PaintBase,coordinate:[u32;2])->Result<Option<source_access::SourceRead>,GpuRasterError> {
    if !source_access::paint_base_contains(base,coordinate) {return Ok(None);}
    accepted(inputs,Key::base(base,coordinate)).map(Some)
}
#[cfg(test)]
pub(super) fn begin_read_record()->bool {
    let observing=ACCEPTED_READ_RESULTS.with(|results|results.borrow().is_some());
    if observing {READS.with(|reads|reads.borrow_mut().clear());}observing
}
#[cfg(test)]
pub(super) fn finish_read_record() {
    ACCEPTED_READ_SET.with(|set|READS.with(|reads| {
        let reads=std::mem::take(&mut *reads.borrow_mut());
        let unexpected=reads.iter().filter(|read|!set.borrow().iter().any(|key|key.matches(read)))
            .map(|key|match key {
                Key::Image(_,coordinate)=>format!("image {coordinate:?}"),
                Key::PaintBase(_,offset,coordinate)=>format!("base offset={offset:?} tile={coordinate:?}"),
                Key::Raster(_,_,_,descriptor)=>format!("raster {descriptor:?}"),
            }).collect();
        ACCEPTED_READ_RESULTS.with(|results|if let Some(results)=results.borrow_mut().as_mut() {results.push((reads.len(),unexpected));});
    }));

}

impl Key {
    fn base(base:&layer_core::authored::PaintBase,coordinate:[u32;2])->Self {
        if base.offset.iter().all(|value|value%PAGE_SIZE==0) {Self::Image(Arc::downgrade(base.image.storage()),std::array::from_fn(|axis|coordinate[axis]-base.offset[axis]/PAGE_SIZE))}
        else {Self::PaintBase(Arc::downgrade(base.image.storage()),base.offset,coordinate)}
    }
    fn raster(blob: &TileBlob, space: RgbSpace, destination: RgbSpace) -> Self {
        Self::Raster(RasterIdentity::of(blob), space, destination, blob.descriptor)
    }
    fn matches(&self, other: &Self) -> bool {
        #[cfg(test)]
        KEY_COMPARISONS.with(|count|if let Some(value)=count.get() {count.set(Some(value+1));});
        match (self, other) {
            (Self::Image(a, x), Self::Image(b, y)) => a.ptr_eq(b) && x == y,
            (Self::PaintBase(a, x, p), Self::PaintBase(b, y, q)) => a.ptr_eq(b) && x == y && p == q,
            (Self::Raster(a, x, d, p), Self::Raster(b, y, e, q)) => a == b && x == y && d == e && p == q,
            _ => false,
        }
    }
    fn samples(&self, destination: RgbSpace) -> Option<Samples> {
        match self {
            Self::Image(source, coordinate) => {
                let source = source.upgrade()?;
                let ColorProfile::Builtin(space) = source.interpretation.profile else { return None; };
                if source.interpretation.channels == SourceChannels::Cmyk || source.interpretation.depth.is_float() { return None; }
                Some(Samples { data: builtin_settings(&source, *coordinate, destination)?, space })
            }
            Self::Raster(_, space, destination, descriptor) if descriptor.channels > 1 && !descriptor.depth().is_float() => {
                Some(Samples { data: rgb_settings(*space, *destination, descriptor.depth(), [PAGE_SIZE; 2], descriptor.alpha), space: *space })
            }
            _ => None,
        }
    }
}
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Representation { Unpacked, Unorm8, Packed16, Srgb8 }
impl Representation {
    fn for_format(format:wgpu::TextureFormat)->Self {
        match format {wgpu::TextureFormat::Rgba8Unorm=>Self::Unorm8,wgpu::TextureFormat::Rg32Uint=>Self::Packed16,_=>Self::Unpacked}
    }
    fn format(self)->wgpu::TextureFormat {
        match self {Self::Unorm8|Self::Srgb8=>wgpu::TextureFormat::Rgba8Unorm,Self::Packed16=>wgpu::TextureFormat::Rg32Uint,Self::Unpacked=>wgpu::TextureFormat::Rgba32Float}
    }
}
#[repr(u32)]
#[derive(Clone, Copy)]
enum TransferCurve { Srgb=0, AdobeRgb=2, ProPhoto=3 }
impl From<RgbSpace> for TransferCurve {
    fn from(space:RgbSpace)->Self {match space {RgbSpace::Srgb|RgbSpace::DisplayP3=>Self::Srgb,RgbSpace::AdobeRgb=>Self::AdobeRgb,RgbSpace::ProPhoto=>Self::ProPhoto}}
}
#[repr(C,align(16))]
#[derive(Clone, Copy)]
pub(crate) struct SourceColor {
    red:[f32;4],green:[f32;4],blue:[f32;4],
    premultiplied:u32,integer_max:f32,extent:[f32;2],
    float_bits:u32,identity:u32,scalar:u32,scalar_scale:f32,
    representation:Representation,display:u32,transfer:TransferCurve,padding:u32,
}
impl SourceColor {
    fn space(self)->RgbSpace {match self.transfer {TransferCurve::AdobeRgb=>RgbSpace::AdobeRgb,TransferCurve::ProPhoto=>RgbSpace::ProPhoto,_=>RgbSpace::Srgb}}
    fn input_format(self)->wgpu::TextureFormat {
        match self.representation {
            Representation::Unorm8 if self.scalar==0=>wgpu::TextureFormat::Rgba8Unorm,
            Representation::Unorm8=>wgpu::TextureFormat::Rgba8Uint,
            Representation::Packed16 if self.scalar==0=>wgpu::TextureFormat::Rg32Uint,
            Representation::Packed16=>wgpu::TextureFormat::Rgba16Uint,
            _ if self.float_bits!=0=>wgpu::TextureFormat::Rgba32Uint,
            _=>wgpu::TextureFormat::Rgba16Uint,
        }
    }
    pub(crate) fn write_uniform(self,bytes:&mut [u8]) {
        let mut words=[0;24];
        for (output,value) in words[..12].iter_mut().zip(self.red.into_iter().chain(self.green).chain(self.blue)) {*output=value.to_bits();}
        words[12..].copy_from_slice(&[self.premultiplied,self.integer_max.to_bits(),self.extent[0].to_bits(),self.extent[1].to_bits(),
            self.float_bits,self.identity,self.scalar,self.scalar_scale.to_bits(),self.representation as u32,self.display,self.transfer as u32,self.padding]);
        for (output,word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {*output=word.to_ne_bytes();}
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Samples { data:SourceColor,space:RgbSpace }
impl Samples {
    pub(crate) fn for_source(source:&SourceImage,coordinate:[u32;2],destination:RgbSpace)->Option<Self> {
        let ColorProfile::Builtin(space)=source.interpretation.profile else {return None;};
        if source.interpretation.depth.is_float() {return None;}
        Some(Self {data:builtin_settings(source,coordinate,destination)?,space})
    }
    pub(crate) fn format(self)->wgpu::TextureFormat {self.data.representation.format()}
    fn parameters(mut self,format:wgpu::TextureFormat)->SourceColor {
        self.data.representation=Representation::for_format(format);
        self.data
    }
    pub(crate) fn display_parameters(mut self)->SourceColor {
        self.data.display=1;
        if self.srgb_view() {self.data.representation=Representation::Srgb8;}
        self.data
    }
    pub(crate) fn srgb_view(self)->bool {self.data.representation==Representation::Unorm8 && matches!(self.space,RgbSpace::Srgb|RgbSpace::DisplayP3)}
}

#[derive(Clone)]
pub(crate) struct Lease { owner:Arc<()>,_encoded:Option<Arc<()>>,read:Option<Arc<Read>> }
#[derive(Clone,Default)]
pub(crate) struct AcceptedInputs(Vec<Lease>);
impl std::ops::Deref for AcceptedInputs {type Target=[Lease];fn deref(&self)->&Self::Target {&self.0}}
impl IntoIterator for AcceptedInputs {
    type Item=Lease;type IntoIter=std::vec::IntoIter<Lease>;
    fn into_iter(self)->Self::IntoIter {self.0.into_iter()}
}
impl Extend<Lease> for AcceptedInputs {
    fn extend<T:IntoIterator<Item=Lease>>(&mut self,inputs:T) {
        for input in inputs {if !self.0.iter().any(|old|Arc::ptr_eq(&old.owner,&input.owner)) {self.0.push(input);}}
    }
}
impl From<Vec<Lease>> for AcceptedInputs {fn from(inputs:Vec<Lease>)->Self {Self(inputs)}}
impl AcceptedInputs {
    pub(crate) fn cpu_bytes(&self)->u64 {
        (self.0.capacity()*std::mem::size_of::<Lease>()+self.iter().filter_map(|input|input.read.as_ref()).map(|read|
            std::mem::size_of::<Read>()+2*std::mem::size_of::<usize>()+read.pending.capacity()*std::mem::size_of::<Arc<PendingTile>>()).sum::<usize>()) as u64
    }
    pub(crate) fn contains(&self,inputs:&Inputs)->bool {
        inputs.tiles.iter().all(|(key,_,_)|self.iter().any(|input|input.read.as_ref().is_some_and(|read|read.key.matches(key))))
    }
    pub(crate) fn retain(&mut self,inputs:&Inputs) {
        self.0.retain(|input|input.read.as_ref().is_some_and(|read|inputs.tiles.iter().any(|(key,_,_)|read.key.matches(key))));
    }
    pub(crate) fn finish_image(&mut self,source:&Arc<SourceImage>,coordinate:[u32;2]) {
        let key=Key::Image(Arc::downgrade(source),coordinate);
        self.0.retain(|input|input.read.as_ref().is_none_or(|read|!read.key.matches(&key)));
    }
    pub(crate) fn ready_for(&self,inputs:&Inputs)->bool {
        inputs.tiles.iter().all(|(key,_,_)|self.iter().filter_map(|input|input.read.as_ref())
            .any(|read|read.key.matches(key) && read.pending.iter().all(|pending|pending.ready())))
    }
    pub(crate) fn prepare(&self,r:&mut WgpuRasterizer)->Result<bool,GpuRasterError> {
        let mut ready=true;
        for read in self.iter().filter_map(|input|input.read.as_ref()) {
            for pending in &read.pending {match pending.prepare(r) {
                Ok(())=>{},Err(error) if error.is_preparation_pending()=>ready=false,Err(error)=>return Err(error),
            }}
        }
        Ok(ready && self.iter().filter_map(|input|input.read.as_ref()).all(|read|read.pending.iter().all(|pending|pending.ready())))
    }
}
struct Read {key:Key,tile:RawTile,pending:Vec<Arc<PendingTile>>,assembly:Option<Arc<source_access::PaintBaseAssembly>>}
impl Read {
    fn source(&self)->source_access::SourceRead {
        source_access::SourceRead {tile:RawTile {texture:self.tile.texture.clone(),view:self.tile.view.clone()},pending:self.pending.clone(),assembly:self.assembly.clone()}
    }
}
impl std::ops::Deref for Lease {type Target=Arc<()>;fn deref(&self)->&Self::Target {&self.owner}}
impl From<Arc<()>> for Lease {fn from(owner:Arc<()>)->Self {Self {owner,_encoded:None,read:None}}}
impl Lease {
    pub(crate) fn track(leases:Vec<Self>,encoder:&mut crate::submission::CommandEncoder) {
        let pins=leases.into_iter().map(|lease|(lease.owner,lease._encoded)).collect::<Vec<_>>();
        encoder.on_submitted_work_done(move||drop(pins));
    }
}
struct Slot {
    bytes:u64,
    key: Option<Key>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    used: u64,
    created: u64,
    valid: Arc<std::sync::atomic::AtomicBool>,
    lease: Arc<()>,
    encoded:Option<Arc<()>>,
    samples: Option<Samples>,
    pending: Option<Arc<PendingTile>>,
    assembly: Option<Arc<source_access::PaintBaseAssembly>>,
}
struct RetiredSlot { _texture: wgpu::Texture, encoded: Option<Arc<()>>, retired: Arc<AtomicU64>, bytes: u64, completed: Arc<std::sync::atomic::AtomicBool> }
impl Slot {
    fn ready(&self)->bool {self.valid.load(Ordering::Acquire) && self.pending.as_ref().is_none_or(|pending|pending.recorded())
        && self.assembly.as_ref().is_none_or(|assembly|assembly.recorded())}
}
impl Drop for RetiredSlot {
    fn drop(&mut self) { self.retired.fetch_sub(self.bytes, Ordering::Release); }
}
struct RetirementCompletion(Arc<std::sync::atomic::AtomicBool>);
impl Drop for RetirementCompletion {
    fn drop(&mut self) { self.0.store(true, Ordering::Release); }
}
enum Pixels {
    Image(Arc<SourceImage>, [u32; 2]),
    Raster(Arc<TileBlob>, RgbSpace,RgbSpace),
    Resident,
}
#[derive(Clone)]
pub enum PreparedSourcePixels { NativeSamples(Arc<Vec<u8>>), PremultipliedWorkingPixels(Arc<Vec<u8>>) }

pub(crate) struct PendingTile {
    pixels: Pixels,
    pub texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub data: Option<SourceColor>,
    write: crate::submission::CacheWrite,
    receipt: std::sync::Mutex<Option<PreparedSourcePixels>>,
    input: Option<(wgpu::Texture, RgbSpace, std::sync::Mutex<Option<RetirementCompletion>>)>,
    dependency: Option<Arc<PendingTile>>,
}
impl PendingTile {
    pub fn prepared(&self)->Option<PreparedSourcePixels> {self.receipt.lock().unwrap().clone()}
    pub fn set_prepared(&self,pixels:Option<PreparedSourcePixels>) {
        if let Some(input)=self.dependency() {input.set_prepared(pixels);}else {*self.receipt.lock().unwrap()=pixels;}
    }
    pub fn recorded(&self)->bool {self.write.recorded()}
    pub(super) fn dependency(&self)->Option<&Arc<Self>> {self.dependency.as_ref().filter(|input|!input.recorded())}
    pub fn ready(&self) -> bool {self.dependency().is_none_or(|input|input.ready()) && (self.input.is_some() || self.prepared().is_some())}
    pub(super) fn validate_prepared(&self)->Result<(),GpuRasterError> {
        if self.input.is_some() {return Ok(());}
        if self.data.is_some() {
            let samples=self.pixels.native()?;
            if samples.tile.descriptor!=samples.descriptor {return Err(GpuRasterError::Color("Source tile has the wrong sample representation".into()));}
            match self.prepared() {
                Some(PreparedSourcePixels::NativeSamples(bytes)) if bytes.len()!=(PAGE_SIZE*PAGE_SIZE) as usize*4*samples.depth.bytes()=>
                    return Err(GpuRasterError::Color("Prepared source samples have the wrong size".into())),
                Some(PreparedSourcePixels::PremultipliedWorkingPixels(_))=>return Err(GpuRasterError::Color("Working source pixels have native decode settings".into())),
                _=>{},
            }
        } else {
            match self.prepared() {
                Some(PreparedSourcePixels::PremultipliedWorkingPixels(bytes)) if bytes.len() as u64!=NORMALIZED_PAGE_BYTES=>return Err(GpuRasterError::InvalidExtent),
                Some(PreparedSourcePixels::NativeSamples(_))=>return Err(GpuRasterError::Color("Native source samples require decode settings".into())),
                _=>{},
            }
        }
        Ok(())
    }
    fn input_format(&self)->Option<wgpu::TextureFormat> {
        let color=self.data?;
        if self.texture.format()!=wgpu::TextureFormat::Rgba32Float {return None;}
        Some(color.input_format())
    }
    pub(crate) fn normalization_parameters(&self)->Option<SourceColor> {
        self.data.map(|mut color| {
            if self.input.is_none() && let Some(format)=self.input_format() {color.representation=Representation::for_format(format);}
            color
        })
    }
    pub(crate) fn pipeline<'a>(&self, pipelines: &'a Pipelines) -> Option<&'a Deferred<wgpu::RenderPipeline>> {
        if let Some((input, ..)) = &self.input { Some(pipelines.for_format(input.format())) }
        else if let Some(format)=self.input_format() {Some(pipelines.for_format(format))}
        else { None }
    }
    pub fn matches_image(&self,source:&Arc<SourceImage>,coordinate:[u32;2])->bool {matches!(&self.pixels,Pixels::Image(stored,c) if Arc::ptr_eq(stored,source) && *c==coordinate)}
    pub fn matches_raster(&self,blob:&Arc<TileBlob>,space:RgbSpace,destination:RgbSpace)->bool {matches!(&self.pixels,Pixels::Raster(stored,from,to) if Arc::ptr_eq(stored,blob) && *from==space && *to==destination)}
    pub fn accept_image(&self,source:&Arc<SourceImage>,coordinate:[u32;2],pixels:&PreparedSourcePixels)->bool {
        if !self.matches_image(source,coordinate) {return false;}
        *self.receipt.lock().unwrap()=Some(pixels.clone());true
    }
    pub fn accept_raster(&self,blob:&Arc<TileBlob>,space:RgbSpace,destination:RgbSpace,pixels:&PreparedSourcePixels)->bool {
        if !self.matches_raster(blob,space,destination) {return false;}
        *self.receipt.lock().unwrap()=Some(pixels.clone());true
    }
    pub fn prepare(&self,r:&mut WgpuRasterizer)->Result<(),GpuRasterError> {
        if self.input.is_some() { return Ok(()); }
        if self.prepared().is_some() {return Ok(());}
        match &self.pixels {
            Pixels::Image(source,coordinate)=> {
                let id=source.tiles.get(coordinate).ok_or(GpuRasterError::InvalidImage)?.resource_id();
                if self.texture.format()==wgpu::TextureFormat::Rgba32Float {r.request_exact_image_decode(id,source.clone(),*coordinate)?;}
                else {r.request_image_decode(id,source.clone(),*coordinate)?;}
            },
            Pixels::Raster(blob,space,destination)=> {r.request_raster_decode_to(blob.clone(),*space,*destination)?;},
            Pixels::Resident => unreachable!(),
        }
        Ok(())
    }
}
struct NativeSamples<'a> {
    tile: &'a Arc<TileBlob>,
    descriptor: PixelDescriptor,
    channels: SourceChannels,
    depth: SampleDepth,
    space: RgbSpace,
}
impl Pixels {
    fn native(&self) -> Result<NativeSamples<'_>, GpuRasterError> {
        match self {
            Self::Image(source, coordinate) => {
                let interpretation = &source.interpretation;
                let ColorProfile::Builtin(space) = interpretation.profile else {
                    unreachable!()
                };
                let tile = source
                    .tiles
                    .get(coordinate)
                    .ok_or_else(|| GpuRasterError::Color("Missing source tile".into()))?;
                Ok(NativeSamples {
                    tile,
                    descriptor: interpretation.descriptor(),
                    channels: interpretation.channels,
                    depth: interpretation.depth,
                    space,
                })
            }
            Self::Raster(tile, space,_) => Ok(NativeSamples {
                tile,
                descriptor: tile.descriptor,
                channels: match tile.descriptor.channels { 1 => SourceChannels::Gray, 2 => SourceChannels::GrayAlpha, _ => SourceChannels::Rgba },
                depth: tile.descriptor.depth(),
                space: *space,
            }),
            Self::Resident => unreachable!(),
        }
    }
}
struct EncodedInput {
    texture: wgpu::Texture,
}
#[derive(Default)]
struct InFlight {
    bytes: AtomicU64,
}
// Dropping an unsubmitted encoder also releases its charge. A failed frame
// cannot strand capacity while device-loss recovery replaces the renderer.
struct UploadCharge(Arc<InFlight>, u64);
impl Drop for UploadCharge {
    fn drop(&mut self) {
        self.0.bytes.fetch_sub(self.1, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct DecodedTiles {
    destination: RgbSpace,
    limits: SourceLimits,
    reserved_bytes:u64,
    slots: Vec<Slot>,
    views:std::collections::HashMap<wgpu::TextureView,usize>,
    slot_bytes:u64,
    clock: u64,
    decoders: VecDeque<(Weak<SourceImage>, layer_color::WorkingDecoder)>,
    pixels: Vec<[f32; 4]>,
    inputs: Vec<EncodedInput>,
    transfer: transfer::Tables,
    in_flight: Arc<InFlight>,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    mip_allowance: u64,
    required: std::collections::HashMap<Key,usize>,
    retired_bytes: Arc<AtomicU64>,
    retired_slots: std::cell::RefCell<Vec<RetiredSlot>>,
    retiring: Vec<RetirementCompletion>,
}
impl DecodedTiles {
    fn remove_slot(&mut self,index:usize)->Slot {
        let slot=self.slots.swap_remove(index);self.slot_bytes-=slot.bytes;self.views.remove(&slot.view);
        if let Some(moved)=self.slots.get(index) {self.views.insert(moved.view.clone(),index);}
        slot
    }
    pub fn new(destination: RgbSpace) -> Self {
        Self {
            destination,
            ..Self::default()
        }
    }
    pub fn admit_limits(&mut self, decoded_bytes: u64, upload_bytes: u64, mip_bytes: u64) {
        self.reap_retired();
        self.limits = SourceLimits::admitted(decoded_bytes, upload_bytes);
        self.mip_allowance = mip_bytes;
        self.evict_to(decoded_bytes,None,self.protected_sources(NORMALIZED_PAGE_BYTES));
    }
    pub fn admitted_bytes(&self) -> [u64; 2] {
        [self.limits.bytes.saturating_sub(self.non_slot_bytes()), self.limits.upload_bytes]
    }
    pub fn mip_budget(&self) -> u64 { self.mip_allowance }
    pub(crate) fn require_images(&mut self, tiles: impl IntoIterator<Item = RequiredSource>) {
        self.required.clear();
        for (source, coordinate) in tiles {
            if self.required.len() >= self.slot_limit().saturating_sub(1) { break; }
            let key = Key::Image(Arc::downgrade(&source), coordinate);
            let rank=self.required.len();
            self.required.entry(key).or_insert(rank);
        }
    }
    fn protected_sources(&self, reserved:u64) -> usize {
        self.reap_retired();
        let leased=self.slots.iter().filter(|slot|Arc::strong_count(&slot.lease)>1 && !self.required(slot,self.required.len()))
            .map(|slot|slot.bytes).sum::<u64>();
        (self.limits.bytes.saturating_sub(self.non_slot_bytes())
            .saturating_sub(leased).saturating_sub(reserved)/NORMALIZED_PAGE_BYTES) as usize
    }
    fn required(&self, slot: &Slot, count: usize) -> bool {
        slot.key.as_ref().is_some_and(|key|self.required.get(key).is_some_and(|rank|*rank<count))
    }
    pub fn can_upload_bytes(&self, bytes: u64) -> bool {
        self.in_flight.bytes.load(Ordering::Acquire).checked_add(bytes).is_some_and(|n| n <= self.limits.upload_bytes)
    }
    fn slot_limit(&self) -> usize {
        self.reap_retired();
        (self.limits.bytes.saturating_sub(self.non_slot_bytes()) / NORMALIZED_PAGE_BYTES) as usize
    }
    pub fn reserve_mips(&mut self, bytes: u64, _encoder: &crate::submission::CommandEncoder) -> Result<bool, GpuRasterError> {
        Ok(bytes <= self.mip_allowance)
    }
    fn evict_to(&mut self,limit:u64,except:Option<&wgpu::TextureView>,protected:usize)->u64 {
        let mut released=0;
        while self.live_bytes()>limit {
            let Some(index)=self.slots.iter().enumerate().filter(|(_,slot)|Arc::strong_count(&slot.lease)==1
                && except!=Some(&slot.view) && !self.required(slot,protected)).min_by_key(|(_,slot)|(slot.ready(),slot.used)).map(|(index,_)|index) else {break;};
            let slot=self.remove_slot(index);
            released+=slot.bytes;self.evictions+=u64::from(slot.valid.load(Ordering::Acquire));
            let completion=self.retire(slot);self.retiring.push(completion);
        }
        released
    }
    fn reap_retired(&self) {
        self.retired_slots.borrow_mut().retain(|slot| !slot.completed.load(Ordering::Acquire)
            || slot.encoded.as_ref().is_some_and(|encoded|Arc::strong_count(encoded)>1));
    }
    fn retire(&mut self, slot: Slot) -> RetirementCompletion {
        let bytes = slot.bytes;
        let retired = self.retired_bytes.clone();
        retired.fetch_add(bytes, Ordering::Relaxed);
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.retired_slots.get_mut().push(RetiredSlot { _texture: slot.texture, encoded: slot.encoded, retired, bytes, completed: completed.clone() });
        RetirementCompletion(completed)
    }
    pub(crate) fn flush_retired(&mut self, encoder: &mut crate::submission::CommandEncoder) {
        if self.retiring.is_empty() { return; }
        encoder.retain_resources();
        for completion in self.retiring.drain(..) { encoder.on_submitted_work_done(move || drop(completion)); }
    }
    pub fn prepare_transfer(
        &mut self,
        device: &PipelineDevice,
        space: RgbSpace,
    ) -> Result<crate::native_tiles::NativeTransfer, GpuRasterError> {
        self.transfer.prepare(device, space).cloned()
    }
    pub(crate) fn invalidate(&mut self) {for slot in &self.slots {slot.valid.store(false,Ordering::Release);}}
    pub(crate) fn free_slots(&self) -> usize {
        self.reap_retired();
        let leased=self.slots.iter().filter(|slot|Arc::strong_count(&slot.lease)>1).map(|slot|slot.bytes).sum::<u64>();
        (self.limits.bytes.saturating_sub(self.non_slot_bytes())
            .saturating_sub(leased)/NORMALIZED_PAGE_BYTES) as usize
    }
    #[cfg(test)]
    pub(crate) fn allocated_slots(&self)->usize {self.slots.len()}
    pub fn prepared_view(
        &self,
        source: &Arc<SourceImage>,
        coordinate: [u32; 2],
    ) -> Option<&wgpu::TextureView> {
        let key = Key::Image(Arc::downgrade(source), coordinate);
        self.slots
            .iter()
            .find(|s| {
                s.key.as_ref().is_some_and(|k| k.matches(&key)) && s.ready()
            })
            .map(|s| &s.view)
    }
    pub fn prepared_base_view(&self, base: &layer_core::authored::PaintBase, coordinate: [u32; 2]) -> Option<&wgpu::TextureView> {
        if !crate::source_access::paint_base_contains(base, coordinate) { return None; }
        if base.offset.iter().all(|v| v % PAGE_SIZE == 0) {
            return self.prepared_view(base.image.storage(), std::array::from_fn(|i| coordinate[i] - base.offset[i] / PAGE_SIZE))
                .filter(|view| view.texture().format() == wgpu::TextureFormat::Rgba32Float);
        }
        let key = Key::PaintBase(Arc::downgrade(base.image.storage()), base.offset, coordinate);
        self.slots.iter().find(|s| s.key.as_ref().is_some_and(|k| k.matches(&key)) && s.ready()).map(|s| &s.view)
    }
    pub(crate) fn plan_base(&mut self, r: &WgpuRasterizer, base: &layer_core::authored::PaintBase, coordinate: [u32; 2]) -> Result<(RawTile, Option<crate::submission::CacheWrite>), GpuRasterError> {
        self.plan_key(r, Key::PaintBase(Arc::downgrade(base.image.storage()), base.offset, coordinate))
    }
    pub fn uploads_full(&self) -> bool {
        // Reserve room for one Float32 tile within the bounded staging window.
        // U8/U16 inputs consume less; charge their actual encoded byte size.
        !self.can_upload_bytes(NORMALIZED_PAGE_BYTES)
    }
    pub fn prepared_raster_view(&self, blob: &Arc<TileBlob>, space: RgbSpace,destination:RgbSpace) -> Option<&wgpu::TextureView> {
        self.prepared_raster_any_view(blob,space,destination).filter(|view|view.texture().format()==wgpu::TextureFormat::Rgba32Float)
    }
    pub(crate) fn prepared_raster_any_view(&self, blob: &Arc<TileBlob>, space: RgbSpace,destination:RgbSpace) -> Option<&wgpu::TextureView> {
        let key = Key::raster(blob, space, destination);
        self.slots.iter().find(|s| s.key.as_ref().is_some_and(|k| k.matches(&key)) && s.ready())
            .map(|s| &s.view)
    }

    pub fn charge_upload(&self, encoder: &crate::submission::CommandEncoder, bytes: u64) -> u64 {
        let total = self.in_flight.bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let charge = UploadCharge(self.in_flight.clone(), bytes);
        encoder.on_submitted_work_done(move || drop(charge));
        total
    }
    pub(crate) fn non_slot_bytes(&self) -> u64 {
        self.transfer.gpu_bytes()
            + self
                .inputs
                .iter()
                .map(|input| {
                    PAGE_SIZE as u64
                        * PAGE_SIZE as u64
                        * input.texture.format().block_copy_size(None).unwrap() as u64
                })
                .sum::<u64>()
    }
    fn live_bytes(&self) -> u64 {
        self.slot_bytes + self.non_slot_bytes()
    }
    pub fn gpu_bytes(&self) -> u64 {
        self.reap_retired();
        self.live_bytes() + self.retired_bytes.load(Ordering::Acquire)
    }
    pub fn cpu_bytes(&self) -> u64 {
        self.pixels.capacity() as u64 * std::mem::size_of::<[f32; 4]>() as u64
            + self.slots.capacity() as u64 * std::mem::size_of::<Slot>() as u64
            + self.views.capacity() as u64 * std::mem::size_of::<(wgpu::TextureView,usize)>() as u64
            + self.inputs.capacity() as u64 * std::mem::size_of::<EncodedInput>() as u64
            + self.required.capacity() as u64 * (std::mem::size_of::<(Key,usize)>()+1) as u64
            + self.decoders.capacity() as u64 * std::mem::size_of::<(Weak<SourceImage>, layer_color::WorkingDecoder)>() as u64
            + self.retired_slots.borrow().capacity() as u64 * std::mem::size_of::<RetiredSlot>() as u64
    }
    pub fn pinned_bytes(&self) -> u64 {
        let protected = self.protected_sources(NORMALIZED_PAGE_BYTES);
        self.slots.iter().filter(|slot| Arc::strong_count(&slot.lease) > 1 || self.required(slot, protected))
            .map(|slot| slot.bytes).sum::<u64>()
            + self.retired_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn plan(
        &mut self,
        r: &WgpuRasterizer,
        source: &Arc<SourceImage>,
        coordinate: [u32; 2],
    ) -> Result<(RawTile, Option<Arc<PendingTile>>), GpuRasterError> {
        self.plan_image(r,source,coordinate,SourceUse::Display)
    }
    pub(crate) fn plan_exact(&mut self,r:&WgpuRasterizer,source:&Arc<SourceImage>,coordinate:[u32;2])->Result<(RawTile,Option<Arc<PendingTile>>),GpuRasterError> {
        self.plan_image(r,source,coordinate,SourceUse::Exact)
    }
    fn plan_image(&mut self,r:&WgpuRasterizer,source:&Arc<SourceImage>,coordinate:[u32;2],use_:SourceUse)->Result<(RawTile,Option<Arc<PendingTile>>),GpuRasterError> {
        let (tile,write)=self.plan_key_for(r,Key::Image(Arc::downgrade(source),coordinate),use_)?;
        let pending = write.map(|write| PendingTile {
            pixels: Pixels::Image(source.clone(), coordinate),
            texture: tile.texture.clone(),
            view: tile.view.clone(),
            data: builtin_settings(source, coordinate, self.destination),
            write,
            receipt:Default::default(),input:None,dependency:None,
        });
        let pending=self.pending(&tile.view,pending);
        Ok((tile, pending))
    }

    pub(super) fn plan_raster(
        &mut self,
        r: &WgpuRasterizer,
        blob: &Arc<TileBlob>,
        space: RgbSpace,
        destination: RgbSpace,
    ) -> Result<(RawTile, Option<Arc<PendingTile>>), GpuRasterError> {
        validate_raster(blob, space)?;
        let d = blob.descriptor;
        let depth = d.depth();
        let mut data = rgb_settings(space, destination, depth, [PAGE_SIZE; 2], d.alpha);
        data.scalar = u32::from(d.channels == 1);
        data.scalar_scale = if d.channels == 1 { 1. / depth.maximum() as f32 } else { 0. };
        let (tile, write) =
            self.plan_key_for(r,Key::raster(blob,space,destination),SourceUse::Exact)?;
        let pending = write.map(|write| PendingTile {
            pixels: Pixels::Raster(blob.clone(), space,destination),
            texture: tile.texture.clone(),
            view: tile.view.clone(),
            data: Some(data),
            write,
            receipt:Default::default(),input:None,dependency:None,
        });
        let pending=self.pending(&tile.view,pending);
        Ok((tile, pending))
    }

    fn pending(&mut self,view:&wgpu::TextureView,pending:Option<PendingTile>)->Option<Arc<PendingTile>> {
        let slot=self.slots.iter_mut().find(|slot|slot.view==*view).unwrap();
        if let Some(pending)=pending {slot.pending=Some(Arc::new(pending));}
        slot.pending.as_ref().filter(|pending|!pending.recorded()).cloned()
    }
    pub(crate) fn base_pending(&self,view:&wgpu::TextureView)->Option<Arc<source_access::PaintBaseAssembly>> {
        self.slots.iter().find(|slot|slot.view==*view)?.assembly.as_ref().filter(|assembly|!assembly.recorded()).cloned()
    }
    pub(crate) fn remember_base(&mut self,view:&wgpu::TextureView,assembly:Arc<source_access::PaintBaseAssembly>) {
        self.slots.iter_mut().find(|slot|slot.view==*view).unwrap().assembly=Some(assembly);
    }
    pub(crate) fn finish_base(&mut self,view:&wgpu::TextureView) {
        if let Some(slot)=self.slots.iter_mut().find(|slot|slot.view==*view) {slot.assembly=None;}
    }
    pub fn batch_checkpoint(&self) -> u64 { self.clock }
    pub fn release_batch(&mut self, checkpoint:u64) {
        for slot in self.slots.iter_mut().filter(|slot|slot.created>checkpoint) {
            slot.key=None;
            slot.valid=Arc::new(std::sync::atomic::AtomicBool::new(false));
            slot.created=0;
            slot.used=0;
        }
    }
    pub fn lease(&self, view: &wgpu::TextureView) -> Option<Lease> {
        self.views.get(view).map(|index|&self.slots[*index]).map(|slot|Lease {owner:slot.lease.clone(),_encoded:slot.encoded.clone(),read:None})
    }

    fn make_room(&mut self, bytes: u64, except: Option<&wgpu::TextureView>) -> Result<(), GpuRasterError> {
        self.reap_retired();
        let limit=self.limits.bytes.saturating_add(self.reserved_bytes);
        if bytes==0 || self.live_bytes().saturating_add(bytes) <= limit { return Ok(()); }
        let input=except.and_then(|view|self.slots.iter().find(|slot|slot.view==*view)).map_or(0,|slot|slot.bytes);
        self.evict_to(limit.saturating_sub(bytes),except,self.protected_sources(bytes.saturating_add(input)));
        if self.live_bytes().saturating_add(bytes) <= limit { return Ok(()); }
        Err(GpuRasterError::SourceWorkingSetExceeded)
    }
    fn texture(r: &WgpuRasterizer, format: wgpu::TextureFormat) -> wgpu::Texture {
        r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bounded source tile"), size: wgpu::Extent3d { width: PAGE_SIZE, height: PAGE_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC
                | if format == wgpu::TextureFormat::Rgba32Float { wgpu::TextureUsages::RENDER_ATTACHMENT } else { wgpu::TextureUsages::empty() },
            view_formats: if format == wgpu::TextureFormat::Rgba8Unorm { &[wgpu::TextureFormat::Rgba8UnormSrgb] } else { &[] },
        })
    }
    fn allocate_slot(&mut self, r: &WgpuRasterizer, format: wgpu::TextureFormat, samples: Option<Samples>) -> usize {
        let texture = Self::texture(r, format);
        let view = texture.create_view(&Default::default());
        let bytes=u64::from(PAGE_SIZE).pow(2)*u64::from(format.block_copy_size(None).unwrap());
        self.slot_bytes+=bytes;self.views.insert(view.clone(),self.slots.len());
        self.slots.push(Slot { bytes,key: None, texture, view, used: 0, created: 0,
            valid: Arc::new(std::sync::atomic::AtomicBool::new(false)), lease: Arc::new(()),encoded:(format!=wgpu::TextureFormat::Rgba32Float).then(||Arc::new(())), samples,pending:None,assembly:None });
        self.slots.len() - 1
    }
    fn plan_key(&mut self, r: &WgpuRasterizer, key: Key) -> Result<(RawTile, Option<crate::submission::CacheWrite>), GpuRasterError> {
        self.plan_key_for(r,key,SourceUse::Display)
    }
    fn plan_key_for(&mut self,r:&WgpuRasterizer,key:Key,use_:SourceUse)->Result<(RawTile,Option<crate::submission::CacheWrite>),GpuRasterError> {
        #[cfg(test)]
        if ACCEPTED_READ_RESULTS.with(|results|results.borrow().is_some()) {READS.with(|reads|reads.borrow_mut().push(key.clone()));}
        let limit=self.limits.bytes.saturating_add(self.reserved_bytes);
        self.clock = self.clock.wrapping_add(1);
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.key.as_ref().is_some_and(|stored| stored.matches(&key))
            && slot.valid.load(Ordering::Acquire)) {
            slot.used = self.clock; self.hits += 1;
            return Ok((RawTile { texture: slot.texture.clone(), view: slot.view.clone() }, None));
        }
        for slot in self.slots.iter_mut().filter(|slot|!slot.valid.load(Ordering::Acquire)
            && slot.key.as_ref().is_some_and(|stored|stored.matches(&key))) {slot.key=None;}
        let mut samples=key.samples(self.destination);
        let format=if use_==SourceUse::Exact {samples=None;wgpu::TextureFormat::Rgba32Float}else{samples.map_or(wgpu::TextureFormat::Rgba32Float,Samples::format)};
        let bytes = u64::from(PAGE_SIZE).pow(2) * u64::from(format.block_copy_size(None).unwrap());
        let protected = self.protected_sources(NORMALIZED_PAGE_BYTES);
        let unused = self.slots.iter().position(|slot| Arc::strong_count(&slot.lease) == 1 && !slot.valid.load(Ordering::Acquire)
            && slot.texture.format() == format && (!self.required(slot, protected) || slot.key.as_ref().is_some_and(|stored| stored.matches(&key))));
        let index = if let Some(index) = unused { index } else if self.live_bytes().saturating_add(bytes) <= limit {
            self.allocate_slot(r, format, samples)
        } else {
            let Some(index) = self.slots.iter().enumerate().filter(|(_, slot)| Arc::strong_count(&slot.lease) == 1 && !self.required(slot, protected))
                .min_by_key(|(_, slot)| slot.used).map(|(index, _)| index) else {
                return Err(GpuRasterError::SourceWorkingSetExceeded);
            };
            if self.slots[index].texture.format() != format {
                self.make_room(bytes, None)?;
                self.allocate_slot(r, format, samples)
            } else {
                index
            }
        };
        let slot = &mut self.slots[index];
        self.evictions += u64::from(slot.valid.load(Ordering::Acquire));
        slot.pending=None;slot.assembly=None;slot.key = Some(key); slot.samples = samples; slot.used = self.clock; slot.created = self.clock;
        let write = crate::submission::CacheWrite::new(); slot.valid = write.validity();
        Ok((RawTile { texture: slot.texture.clone(), view: slot.view.clone() }, Some(write)))
    }
    pub(crate) fn promote(&mut self, r: &WgpuRasterizer, tile: RawTile) -> Result<(RawTile, Option<Arc<PendingTile>>), GpuRasterError> {
        if tile.texture.format() == wgpu::TextureFormat::Rgba32Float { return Ok((tile, None)); }
        let limit=self.limits.bytes.saturating_add(self.reserved_bytes);
        let index = self.slots.iter().position(|slot| slot.view == tile.view).ok_or(GpuRasterError::InvalidImage)?;
        let samples = self.slots[index].samples.ok_or(GpuRasterError::InvalidImage)?;
        let table_bytes = if self.transfer.contains(samples.space) { 0 } else { transfer::TABLE_BYTES };
        let protected=self.protected_sources(NORMALIZED_PAGE_BYTES.saturating_add(texture_bytes(&tile.texture)));
        let reusable=(table_bytes==0 && self.live_bytes().saturating_add(NORMALIZED_PAGE_BYTES)>limit).then(||
            self.slots.iter().enumerate().filter(|(_,slot)|slot.texture.format()==wgpu::TextureFormat::Rgba32Float
                && Arc::strong_count(&slot.lease)==1 && !self.required(slot,protected))
                .min_by_key(|(_,slot)|slot.used).map(|(index,_)|index)).flatten();
        let (texture,view)=if let Some(index)=reusable {
            let slot=self.remove_slot(index);
            self.evictions+=u64::from(slot.valid.load(Ordering::Acquire));
            (slot.texture,slot.view)
        } else {
            self.make_room(NORMALIZED_PAGE_BYTES + table_bytes, Some(&tile.view))?;
            let texture=Self::texture(r,wgpu::TextureFormat::Rgba32Float);
            let view=texture.create_view(&Default::default());
            (texture,view)
        };
        self.transfer.prepare(&r.device,samples.space)?;
        let index=self.slots.iter().position(|slot|slot.view==tile.view).ok_or(GpuRasterError::InvalidImage)?;
        let write = crate::submission::CacheWrite::new();
        let slot = &mut self.slots[index];
        let bytes=u64::from(PAGE_SIZE).pow(2)*16;
        self.slot_bytes=self.slot_bytes-slot.bytes+bytes;self.views.remove(&slot.view);self.views.insert(view.clone(),index);
        let replacement = Slot { bytes,key: slot.key.take(), texture: texture.clone(), view: view.clone(), used: slot.used,
            created: slot.created, valid: write.validity(), lease: slot.lease.clone(),encoded:None, samples: None, pending:None,assembly:None };
        let old = std::mem::replace(slot, replacement);
        let dependency=old.pending.clone();
        let completion = self.retire(old);
        let data = samples.parameters(tile.texture.format());
        let pending=PendingTile {
            pixels: Pixels::Resident, texture:texture.clone(), view:view.clone(), data: Some(data), write, receipt: Default::default(), dependency,
            input: Some((tile.texture, samples.space, std::sync::Mutex::new(Some(completion)))),
        };
        let pending=self.pending(&view,Some(pending));
        Ok((RawTile {texture,view},pending))
    }
    pub(crate) fn display(&mut self, r: &WgpuRasterizer, encoder: &mut crate::submission::CommandEncoder,
        samples:Samples,input:&wgpu::Texture,output:&wgpu::TextureView,uniforms:&wgpu::BindGroup,offset:u32,
    ) -> Result<(), GpuRasterError> {
        let pipelines = &r.scene_pipelines.source;
        if !r.compilation.require([pipelines.for_format(input.format())], startup::VALIDATION) { return Err(GpuRasterError::NotReady); }
        let admission=self.make_room(if self.transfer.contains(samples.space) {0}else{transfer::TABLE_BYTES},
            self.slots.iter().find(|slot|slot.texture==*input).map(|slot|slot.view.clone()).as_ref());
        self.flush_retired(encoder);
        admission?;
        let srgb=samples.srgb_view();
        let transfer = self.transfer.prepare(&r.device, samples.space)?;
        let view = input.create_view(&wgpu::TextureViewDescriptor { format: srgb.then_some(wgpu::TextureFormat::Rgba8UnormSrgb), ..Default::default() });
        pipelines.normalize(&r.device,encoder,&view,output,transfer,uniforms,offset);
        Ok(())
    }
    fn prepare_color(&mut self,device:&crate::PipelineDevice,color:SourceColor)->Result<(),GpuRasterError> {
        let format=color.input_format();let space=color.space();
        let missing=!self.inputs.iter().any(|input|input.texture.format()==format);
        let extra=if missing {u64::from(PAGE_SIZE).pow(2)*u64::from(format.block_copy_size(None).unwrap())}else {0}
            +if self.transfer.contains(space) {0}else{transfer::TABLE_BYTES};
        self.make_room(extra,None)?;
        if missing {self.inputs.push(EncodedInput {texture:device.create_texture(&wgpu::TextureDescriptor {
            label:Some("bounded exact source input"),size:wgpu::Extent3d {width:PAGE_SIZE,height:PAGE_SIZE,depth_or_array_layers:1},mip_level_count:1,sample_count:1,
            dimension:wgpu::TextureDimension::D2,format,usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,view_formats:&[],
        })});}
        self.transfer.prepare(device,space)?;Ok(())
    }
    pub(super) fn preflight(&mut self,device:&crate::PipelineDevice,pending:&PendingTile)->Result<(),GpuRasterError> {
        pending.validate_prepared()?;
        if let Some((_,space,_))=&pending.input {self.transfer.prepare(device,*space)?;}
        else if pending.input_format().is_some() && let Some(color)=pending.data {self.prepare_color(device,color)?;}
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(
        &mut self,
        device: &crate::PipelineDevice,
        uploads: &mut crate::Uploads,
        pipelines: &Pipelines,
        encoder: &mut crate::submission::CommandEncoder,
        pending: &PendingTile,
        snapshot_worker:bool,
        uniforms: Option<&wgpu::BindGroup>,
        offset: u32,
    ) -> Result<u64, GpuRasterError> {
        if pending.recorded() {return Ok(0);}
        if let Some((input, space, retirement)) = &pending.input {
            let transfer = self.transfer.prepare(device, *space)?.clone();
            pipelines.normalize(device, encoder, &input.create_view(&Default::default()), &pending.view, &transfer,
                uniforms.ok_or(GpuRasterError::InvalidExtent)?, offset);
            pending.write.track(encoder);
            if let Some(slot)=self.slots.iter_mut().find(|slot|slot.view==pending.view) {slot.pending=None;}
            if let Some(completion) = retirement.lock().unwrap().take() { encoder.on_submitted_work_done(move || drop(completion)); }
            return Ok(0);
        }
        let prepared_pixels=pending.prepared();
        if (!snapshot_worker || cfg!(target_arch="wasm32")) && prepared_pixels.is_none() {return Err(GpuRasterError::NotReady);}
        let bytes = if pending.data.is_some() {
            let samples = pending.pixels.native()?;
            let target = if pending.texture.format() == wgpu::TextureFormat::Rgba32Float {
                let format=pending.input_format().ok_or(GpuRasterError::InvalidImage)?;
                self.inputs.iter().find(|input|input.texture.format()==format).ok_or(GpuRasterError::InvalidImage)?.texture.clone()
            } else { pending.texture.clone() };
            let decoded = match &prepared_pixels {
                Some(PreparedSourcePixels::NativeSamples(samples)) => samples.clone(),
                Some(PreparedSourcePixels::PremultipliedWorkingPixels(_)) => return Err(GpuRasterError::Color("Working source pixels have native decode settings".into())),
                None => {
                    #[cfg(not(target_arch="wasm32"))]
                    {device.source_samples.decode_on_worker(samples.tile).map_err(GpuRasterError::Color)?}
                    #[cfg(target_arch="wasm32")]
                    {return Err(GpuRasterError::NotReady);}
                },
            };
            let step = samples.depth.bytes();
            let prepared = prepared_pixels.is_some();
            let channels = if prepared { 4 } else { samples.channels.count() };
            if !prepared && decoded.len() != (PAGE_SIZE * PAGE_SIZE) as usize * channels * step {
                return Err(GpuRasterError::Color("Prepared source samples have the wrong size".into()));
            }
            let bytes = (PAGE_SIZE * PAGE_SIZE) as usize * 4 * step;
            uploads.write_texture(encoder, &target, PAGE_SIZE * 4 * step as u32, |mapped| {
                if prepared || samples.channels == SourceChannels::Rgba {
                    mapped.copy_from_slice(&decoded);
                } else {
                    // Expand channels in one row of scratch. Alpha is coverage;
                    // grayscale uses the declared RGB transfer on equal R/G/B.
                    let mut row = [0u8; PAGE_SIZE as usize * 16];
                    let bpp = samples.channels.count() * step;
                    let row_bytes = PAGE_SIZE as usize * 4 * step;
                    for (y, input) in decoded.chunks_exact(PAGE_SIZE as usize * bpp).enumerate() {
                        expand_source_row(input, &mut row[..row_bytes], samples.channels, samples.depth);
                        mapped
                            .slice(y * row_bytes..(y + 1) * row_bytes)
                            .copy_from_slice(&row[..row_bytes]);
                    }
                }
            })?;
            if pending.texture.format() == wgpu::TextureFormat::Rgba32Float {
                let transfer = self.transfer.prepare(device, samples.space)?.clone();
                pipelines.normalize(device, encoder, &target.create_view(&Default::default()), &pending.view, &transfer,
                    uniforms.ok_or(GpuRasterError::InvalidExtent)?, offset);
            }
            bytes as u64
        } else {
            let Pixels::Image(source, coordinate) = &pending.pixels else {
                unreachable!()
            };
            match &prepared_pixels {
                Some(PreparedSourcePixels::PremultipliedWorkingPixels(bytes)) => {
                    uploads.write_texture(encoder, &pending.texture, PAGE_SIZE * 16, |mapped| mapped.copy_from_slice(bytes))?;
                }
                Some(PreparedSourcePixels::NativeSamples(_)) => return Err(GpuRasterError::Color("Native source samples require decode settings".into())),
                None => self.upload_icc(device, uploads, source, *coordinate, encoder, &pending.texture)?,
            }
            NORMALIZED_PAGE_BYTES
        };
        pending.write.track(encoder);
        if let Some(slot)=self.slots.iter_mut().find(|slot|slot.view==pending.view) {slot.pending=None;}
        self.misses += 1;
        Ok(bytes)
    }

    fn upload_icc(
        &mut self,
        device: &crate::PipelineDevice,
        uploads: &mut crate::Uploads,
        source: &Arc<SourceImage>,
        coordinate: [u32; 2],
        encoder: &mut crate::submission::CommandEncoder,
        texture: &wgpu::Texture,
    ) -> Result<(), GpuRasterError> {
        let weak = Arc::downgrade(source);
        let index = if let Some(index) = self.decoders.iter().position(|(s, _)| s.ptr_eq(&weak)) {
            index
        } else {
            // Keep the source native; decode into the document's working primaries.
            let decoder = layer_color::WorkingDecoder::new(
                &source.interpretation,
                self.destination,
                Default::default(),
            )
            .map_err(GpuRasterError::Color)?;
            if self.decoders.len() == DECODERS {
                self.decoders.pop_front();
            }
            self.decoders.push_back((weak, decoder));
            self.decoders.len() - 1
        };
        let decoder = self.decoders.remove(index).unwrap();
        self.pixels
            .resize((PAGE_SIZE * PAGE_SIZE) as usize, [0.; 4]);
        decoder
            .1
            .decode_tile_cached(source, coordinate, &mut self.pixels, &device.source_samples)
            .map_err(GpuRasterError::Color)?;
        self.decoders.push_back(decoder);
        upload_working_pixels(uploads, encoder, texture, &self.pixels)
    }
}

fn upload_working_pixels(uploads: &mut crate::Uploads, encoder: &mut crate::submission::CommandEncoder,
    texture: &wgpu::Texture, pixels: &[[f32; 4]],
) -> Result<(), GpuRasterError> {
    if pixels.len() != (PAGE_SIZE * PAGE_SIZE) as usize { return Err(GpuRasterError::InvalidExtent); }
        uploads.write_texture(encoder, texture, PAGE_SIZE * 16, |mapped| {
            let mut row = [0u8; PAGE_SIZE as usize * 16];
            for (y, pixels) in pixels.as_chunks::<{ PAGE_SIZE as usize }>().0.iter().enumerate() {
                for (bytes, pixel) in row.as_chunks_mut::<16>().0.iter_mut().zip(pixels) {
                    for (channel, bytes) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let value = if channel == 3 {
                            pixel[3]
                        } else {
                            pixel[channel] * pixel[3]
                        };
                        bytes.copy_from_slice(&value.to_ne_bytes());
                    }
                }
                mapped
                    .slice(y * row.len()..(y + 1) * row.len())
                    .copy_from_slice(&row);
            }
        })
}

pub(crate) fn validate_raster(blob: &TileBlob, space: RgbSpace) -> Result<(), GpuRasterError> {
    let d = blob.descriptor;
    if d.channels == 1 && d.sample == layer_core::color::SampleType::Unsigned
        && matches!(d.bits_per_channel, 8 | 16) && d.encoding == TransferEncoding::Linear
        && d.alpha == AlphaAssociation::None { return Ok(()); }
    if !matches!(d.channels, 2 | 4)
        || d.bytes_per_pixel().is_none()
        || !matches!(
            d.alpha,
            AlphaAssociation::Straight | AlphaAssociation::PremultipliedLinear
        )
        || !(d.sample == layer_core::color::SampleType::Float && d.encoding == TransferEncoding::Linear && matches!(d.bits_per_channel, 16 | 32)
            || d.encoding == TransferEncoding::Profile
            || (d.encoding == TransferEncoding::Srgb && space == RgbSpace::Srgb))
    {
        return Err(GpuRasterError::Color(
            "Invalid native raster decode representation".into(),
        ));
    }
    Ok(())
}

/// Expand integer source channels without changing their codes or byte order.
/// Fixed pixel widths keep the hot upload path out of per-channel dynamic copies.
pub(crate) fn expand_source_row(input: &[u8], output: &mut [u8], channels: SourceChannels, depth: SampleDepth) {
    match (depth, channels) {
        (SampleDepth::U8, SourceChannels::Rgb) => {
            for (p, o) in input.as_chunks::<3>().0.iter().zip(output.as_chunks_mut::<4>().0.iter_mut()) {
                o.copy_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        (SampleDepth::U8, SourceChannels::Gray) => {
            for (p, o) in input.iter().zip(output.as_chunks_mut::<4>().0.iter_mut()) {
                o.copy_from_slice(&[*p, *p, *p, 255]);
            }
        }
        (SampleDepth::U8, SourceChannels::GrayAlpha) => {
            for (p, o) in input.as_chunks::<2>().0.iter().zip(output.as_chunks_mut::<4>().0.iter_mut()) {
                o.copy_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        (SampleDepth::U16, SourceChannels::Rgb) => {
            for (p, o) in input.as_chunks::<6>().0.iter().zip(output.as_chunks_mut::<8>().0.iter_mut()) {
                o.copy_from_slice(&[p[0], p[1], p[2], p[3], p[4], p[5], 255, 255]);
            }
        }
        (SampleDepth::U16, SourceChannels::Gray) => {
            for (p, o) in input.as_chunks::<2>().0.iter().zip(output.as_chunks_mut::<8>().0.iter_mut()) {
                o.copy_from_slice(&[p[0], p[1], p[0], p[1], p[0], p[1], 255, 255]);
            }
        }
        (SampleDepth::U16, SourceChannels::GrayAlpha) => {
            for (p, o) in input.as_chunks::<4>().0.iter().zip(output.as_chunks_mut::<8>().0.iter_mut()) {
                o.copy_from_slice(&[p[0], p[1], p[0], p[1], p[0], p[1], p[2], p[3]]);
            }
        }
        (SampleDepth::F32, SourceChannels::Rgb) => {
            for (p, o) in input.as_chunks::<12>().0.iter().zip(output.as_chunks_mut::<16>().0.iter_mut()) {
                o[..12].copy_from_slice(p);
                o[12..].copy_from_slice(&1f32.to_le_bytes());
            }
        }
        (SampleDepth::F16, SourceChannels::Rgb) => {
            for (p,o) in input.as_chunks::<6>().0.iter().zip(output.as_chunks_mut::<8>().0.iter_mut()) { o[..6].copy_from_slice(p); o[6..].copy_from_slice(&0x3c00u16.to_le_bytes()); }
        }
        (SampleDepth::F16 | SampleDepth::F32, SourceChannels::GrayAlpha) => {
            let step = depth.bytes();
            for (p, o) in input.chunks_exact(2 * step).zip(output.chunks_exact_mut(4 * step)) {
                for channel in o[..3 * step].chunks_exact_mut(step) { channel.copy_from_slice(&p[..step]); }
                o[3 * step..].copy_from_slice(&p[step..]);
            }
        }
        (SampleDepth::F16 | SampleDepth::F32, SourceChannels::Gray) => unreachable!("invalid HDR gray source"),
        (_, SourceChannels::Rgba | SourceChannels::Cmyk) => unreachable!("RGBA copies directly; CMYK uses its ICC transform"),
    }
}

fn builtin_settings(
    source: &SourceImage,
    coordinate: [u32; 2],
    destination: RgbSpace,
) -> Option<SourceColor> {
    let ColorProfile::Builtin(space) = source.interpretation.profile else {
        return None;
    };
    if source.interpretation.channels == SourceChannels::Cmyk {
        return None;
    }
    Some(rgb_settings(
        space,
        destination,
        source.interpretation.depth,
        std::array::from_fn(|i| {
            source.extent[i]
                .saturating_sub(coordinate[i] * PAGE_SIZE)
                .min(PAGE_SIZE)
        }),
        AlphaAssociation::Straight,
    ))
}
fn rgb_settings(
    space: RgbSpace,
    destination: RgbSpace,
    depth: SampleDepth,
    extent: [u32; 2],
    alpha: AlphaAssociation,
) -> SourceColor {
    let rows=space.linear_transform(destination).map(|row|[row[0] as f32,row[1] as f32,row[2] as f32,0.]);
    SourceColor {red:rows[0],green:rows[1],blue:rows[2],premultiplied:u32::from(alpha==AlphaAssociation::PremultipliedLinear),
        integer_max:if depth.is_float() {0.}else{depth.maximum() as f32},extent:extent.map(|side|side as f32),
        float_bits:u32::from(depth==SampleDepth::F32),identity:u32::from(space==destination),scalar:0,scalar_scale:0.,
        representation:match depth {SampleDepth::U8=>Representation::Unorm8,SampleDepth::U16=>Representation::Packed16,_=>Representation::Unpacked},
        display:0,transfer:space.into(),padding:0}

}

#[derive(Clone)]
pub(crate) struct Pipelines {
    layouts: [wgpu::BindGroupLayout; 2],
    pub pipeline: Deferred<wgpu::RenderPipeline>,
    pub integer: Deferred<wgpu::RenderPipeline>,
}
impl Pipelines {
    pub fn new(device: &PipelineDevice, uniforms: &wgpu::BindGroupLayout) -> Self {
        let layouts = [false, true].map(|integer| crate::bindings::layout(device, "source normalization", &[
            crate::bindings::texture_of(0, wgpu::ShaderStages::FRAGMENT,
                if integer { wgpu::TextureSampleType::Uint } else { wgpu::TextureSampleType::Float { filterable: false } }, wgpu::TextureViewDimension::D2),
            crate::bindings::buffer(1, wgpu::ShaderStages::FRAGMENT, wgpu::BufferBindingType::Storage { read_only: true }, false,
                wgpu::BufferSize::new(transfer::TABLE_BYTES)),
        ]));
        let [pipeline, integer] = std::array::from_fn(|index| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("source normalization"), bind_group_layouts: &[Some(uniforms), Some(&layouts[index])], immediate_size: 0,
            });
            let input = if index == 0 { r#"
@group(1) @binding(0) var encoded: texture_2d<f32>;
fn native_samples(p: vec2<i32>) -> vec4<u32> { return vec4<u32>(round(clamp(textureLoad(encoded,p,0),vec4(0.),vec4(1.))*255.)); }
fn display_samples(p: vec2<i32>) -> vec4<f32> { return textureLoad(encoded,p,0); }
"# } else { r#"
@group(1) @binding(0) var encoded: texture_2d<u32>;
fn native_samples(p: vec2<i32>) -> vec4<u32> {
    let value=textureLoad(encoded,p,0);
    if settings.representation==SOURCE_PACKED16 { return vec4(value.r&65535u,value.r>>16u,value.g&65535u,value.g>>16u); }
    return value;
}
fn display_samples(p: vec2<i32>) -> vec4<f32> { let code=native_samples(p); return vec4(vec3<f32>(code.rgb)/settings.integer_max, select(f32(code.a)/settings.integer_max,1.,code.a==u32(settings.integer_max))); }
"# };
            let shader = Deferred::wgsl(device, "source normalization", format!("{}\n{}\n{}",
                include_str!("../native_tiles/validity.wgsl"), include_str!("source_color.wgsl"), include_str!("source_decode.wgsl").replace("SOURCE_INPUT", input)));
            let device = device.for_recipe();
            Deferred::pipeline(move |mode| fullscreen_pipeline_recipe(mode, &device, &layout, &shader, "fragment_main", None,
                wgpu::TextureFormat::Rgba32Float, "normalize source tile"))
        });
        Self { layouts, pipeline, integer }
    }
    fn index(texture: &wgpu::Texture) -> usize {
        usize::from(Self::integer_format(texture.format()))
    }
    fn integer_format(format: wgpu::TextureFormat) -> bool { matches!(format, wgpu::TextureFormat::Rg32Uint
        | wgpu::TextureFormat::Rgba8Uint | wgpu::TextureFormat::Rgba16Uint | wgpu::TextureFormat::Rgba32Uint) }
    pub(crate) fn for_format(&self, format: wgpu::TextureFormat) -> &Deferred<wgpu::RenderPipeline> {
        if Self::integer_format(format) { &self.integer } else { &self.pipeline }
    }
    fn normalize(&self, device: &PipelineDevice, encoder: &mut crate::submission::CommandEncoder, input: &wgpu::TextureView,
        output: &wgpu::TextureView, transfer: &transfer::NativeTransfer, uniforms: &wgpu::BindGroup, offset: u32,
    ) {
        let index = Self::index(input.texture());
        let binding = crate::bindings::group(device, "source normalization input", &self.layouts[index], [
            wgpu::BindingResource::TextureView(input), transfer.as_entire_binding(),
        ]);
        let attachments = [Some(attachment(output, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)))];
        #[cfg(test)]
        crate::test_support::PassCounts {normalizations:1,..Default::default()}.record();
        let mut pass = encoder.begin_render_pass(&descriptor(&attachments));
        pass.set_pipeline(if index == 0 { &self.pipeline } else { &self.integer });
        pass.set_bind_group(0, uniforms, &[offset]); pass.set_bind_group(1, &binding, &[]); pass.draw(0..3,0..1);
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod grant_tests {
    use super::*;
    #[test]
    fn source_color_uniform_matches_the_shader_layout() {
        assert_eq!(std::mem::size_of::<SourceColor>(),96);
        assert_eq!(std::mem::align_of::<SourceColor>(),16);
        assert_eq!(std::mem::offset_of!(SourceColor,integer_max),52);
        assert_eq!(std::mem::offset_of!(SourceColor,float_bits),64);
        assert_eq!(std::mem::offset_of!(SourceColor,representation),80);
        assert_eq!(std::mem::offset_of!(SourceColor,transfer),88);
    }
    #[test]
    fn decoded_slots_round_down_without_manufacturing_a_working_grant() {
        for (bytes, slots) in [(0, 0), (NORMALIZED_PAGE_BYTES - 1, 0), (NORMALIZED_PAGE_BYTES, 1), (3 * NORMALIZED_PAGE_BYTES + 1, 3)] {
            let limits = SourceLimits::admitted(bytes, 0);
            assert_eq!(limits.bytes / NORMALIZED_PAGE_BYTES, slots);
            assert!(limits.bytes <= bytes);
            assert_eq!(limits.upload_bytes, 0);
        }
    }
    #[test]
    fn tiny_upload_grants_defer_a_float_tile_without_underflow() {
        let mut cache = DecodedTiles::default();
        cache.admit_limits(2 * NORMALIZED_PAGE_BYTES, NORMALIZED_PAGE_BYTES - 1, 0);
        assert!(cache.uploads_full());
        assert!(cache.can_upload_bytes(NORMALIZED_PAGE_BYTES / 4));
        assert!(!cache.can_upload_bytes(NORMALIZED_PAGE_BYTES));
        cache.admit_limits(2 * NORMALIZED_PAGE_BYTES, NORMALIZED_PAGE_BYTES, 0);
        assert!(!cache.uploads_full());
    }
    #[test]
    fn mip_grants_do_not_consume_decoded_tile_slots() {
        let mut cache = DecodedTiles::default();
        cache.admit_limits(3 * NORMALIZED_PAGE_BYTES, NORMALIZED_PAGE_BYTES, 100 * NORMALIZED_PAGE_BYTES);
        assert_eq!(cache.slot_limit(), 3);
        assert_eq!(cache.mip_budget(), 100 * NORMALIZED_PAGE_BYTES);
        cache.admit_limits(NORMALIZED_PAGE_BYTES, NORMALIZED_PAGE_BYTES, 2 * NORMALIZED_PAGE_BYTES);
        assert_eq!(cache.slot_limit(), 1);
        assert_eq!(cache.mip_budget(), 2 * NORMALIZED_PAGE_BYTES);
    }
}

pub(crate) fn encode_pending(r: &mut WgpuRasterizer, encoder: &mut crate::submission::CommandEncoder,
    pending: Arc<PendingTile>,
) -> Result<(), GpuRasterError> {
        if pending.recorded() {return Ok(());}
        let admission = r.source_tiles.get_mut().preflight(&r.device, &pending);
        r.source_tiles.get_mut().flush_retired(encoder);
        admission?;
        if let Some(pipeline) = pending.pipeline(&r.scene_pipelines.source)
            && !r.compilation.require([pipeline],startup::VALIDATION) { return Err(GpuRasterError::NotReady); }
        if let Some(input)=pending.dependency() {encode_pending(r,encoder,input.clone())?;}
        let binding = if pending.pipeline(&r.scene_pipelines.source).is_some() {
            let records = r.device.create_buffer(&wgpu::BufferDescriptor { label: Some("source normalization records"),
                size: 144, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let binding = uniform_binding(&r.device, &r.scene_pipelines.uniforms, &records);
            let mut data = [0; 144];
            if let Some(values)=pending.normalization_parameters() {values.write_uniform(&mut data);}
            r.uploads.write_at(encoder, &records, 0, &data)?;
            Some(binding)
        } else { None };
        let bytes = r.source_tiles.get_mut().encode(&r.device, &mut r.uploads, &r.scene_pipelines.source, encoder, &pending,r.snapshot_worker, binding.as_ref(), 0)?;
        let in_flight = r.source_tiles.borrow().charge_upload(encoder, bytes);
        r.metrics.source_upload_peak_bytes = r.metrics.source_upload_peak_bytes.max(in_flight);
    Ok(())
}
