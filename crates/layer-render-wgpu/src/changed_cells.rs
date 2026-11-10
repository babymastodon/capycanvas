use super::*;
use std::collections::{BTreeMap, BTreeSet};
use wgpu::util::DeviceExt;

const LIMIT:u64=32*1024*1024;
type Key=(SourceTarget,[u32;2]);

pub(super) struct Preview<'a> {
    pub id:Option<SourceTarget>, pub level:u32, pub contribution:bool, pub damage:PixelRect,
    pub tiles:Option<&'a BTreeSet<[u32;2]>>,
}
impl Preview<'_> {
    fn contains(&self,c:[u32;2])->bool { !page_rect(c).intersect(self.damage).is_empty() && self.tiles.is_none_or(|s|s.contains(&c)) }
}

pub(super) struct ChangedCells {
    pub disabled:wgpu::Buffer,
    pages:BTreeMap<Key,wgpu::Buffer>,
    forced:BTreeSet<Key>,
    retired:Vec<wgpu::Buffer>,
    side:u32,
    admission:Option<(display_mips::Plan,u64)>,
}
impl ChangedCells {
    pub fn new(device:&PipelineDevice)->Self {
        let disabled=device.create_buffer(&wgpu::BufferDescriptor {
            label:Some("disabled changed cells"),size:20,usage:wgpu::BufferUsages::STORAGE,mapped_at_creation:false,
        });
        Self {disabled,pages:BTreeMap::new(),forced:BTreeSet::new(),retired:Vec::new(),side:1,admission:None}
    }
    pub fn storage_bytes(&self)->u64 {
        self.disabled.size()+self.pages.first_key_value().map_or(0,|(_,b)|b.size()*self.pages.len() as u64)
    }
    pub fn take_retired(&mut self)->Vec<wgpu::Buffer> {std::mem::take(&mut self.retired)}
    fn retire(&mut self,r:&WgpuRasterizer,wanted:&BTreeSet<Key>,side:u32) {
        let retired:BTreeSet<_>=self.pages.keys().copied().filter(|key|side!=self.side || !wanted.contains(key)).collect();
        for layer in &r.paint_layers {
            for page in &layer.pages {
                if retired.contains(&(layer.id,page.coordinate)) {
                    page.primary.clear_bindings();
                    if let Some(secondary)=&page.secondary {secondary.clear_bindings();}
                }
            }
        }
        if !retired.is_empty() {for page in &r.preview_pages {
            page.primary.clear_bindings();
            if let Some(secondary)=&page.secondary {secondary.clear_bindings();}
        }}
        self.retired.extend(self.pages.extract_if(..,|key,_|retired.contains(key)).map(|(_,buffer)|buffer));
    }
    pub fn buffer(&self,id:SourceTarget,coordinate:[u32;2])->Option<&wgpu::Buffer> {
        self.pages.get(&(id,coordinate))
    }
    pub fn reusable(&self,id:SourceTarget,coordinate:[u32;2])->Option<&wgpu::Buffer> {
        (!self.forced.contains(&(id,coordinate))).then(||self.buffer(id,coordinate)).flatten()
    }
    pub fn force_all(&mut self) {self.forced.extend(self.pages.keys().copied());}
    fn force_source(&mut self,id:SourceTarget) {
        self.forced.extend(self.pages.range((id,[0,0])..=(id,[u32::MAX;2])).map(|(key,_)|*key));
    }
    pub fn force(&mut self,id:SourceTarget,region:PixelRect) {
        self.forced.extend(page_coordinates(region).map(|c|(id,c)));
    }
    pub fn prepare(&mut self,r:&WgpuRasterizer,packet:FramePacket<'_>,tiles:&[Vec<brush_tiles::BrushTile>],unchanged:bool,encoder:&mut submission::CommandEncoder) {
        let Some(cache)=r.scale_display.as_ref().filter(|c|c.evaluation==scene::scale::Evaluation::Display && c.plan.level>0) else {
            self.retire(r,&BTreeSet::new(),self.side);self.admission=None;return;
        };
        let admission=(cache.plan,cache.working_bytes());
        if unchanged && self.admission==Some(admission) && !packet.reset_layers && !packet.composite_all
            && packet.restore_rasters.is_empty() && packet.dabs.is_empty() && packet.dab_batches.is_empty()
            && r.transform_damage.is_empty() && r.document_damage.is_empty() && r.transform_preview.is_none() && r.moving_layer.is_none() && r.moving_pixels.is_none()
            && r.preview_layer_id.is_none() {return;}
        self.admission=None;
        self.forced.clear();
        let side=(1<<cache.plan.level).min(PAGE_SIZE);
        let allowed=!packet.reset_layers && !packet.composite_all && packet.restore_rasters.is_empty()
            && scene::same_metadata(r,packet);
        let wanted:BTreeSet<_>=r.paint_layers.iter().filter(|l|allowed && cache.native_preview_input(packet,l.id)
            && packet.scene.target_offset(l.id)==[0; 2]).flat_map(|l|l.pages.iter().map(|p|(l.id,p.coordinate)))
            .filter(|&(id,c)|!r.base_copy(packet.scene,id,c))
            .chain(tiles.iter().zip(packet.dab_batches).filter(|(_,b)|allowed && cache.native_preview_input(packet,b.target)
                && packet.scene.target_offset(b.target)==[0; 2]).flat_map(|(t,b)|t.iter().map(|t|(b.target,t.coordinate))))
            .chain(r.preview_layer_id.into_iter().filter(|id|!packet.commit_rasters && allowed && r.preview_contribution && cache.native_preview_input(packet,*id))
                .flat_map(|id|r.preview_pages.iter().filter(|p|r.preview_contact_tiles.as_ref().is_none_or(|s|s.contains(&p.coordinate)))
                    .map(move |p|(id,p.coordinate)))).collect();
        let bytes=16+u64::from((PAGE_SIZE/side).pow(2))*4;
        let required=self.disabled.size()+wanted.len() as u64*bytes;
        let available=r.memory.limit(layer_core::MemoryOwnerKind::Scene,false)
            .saturating_sub(cache.storage_bytes()+r.scene.as_ref().map_or(0,|scene|scene.working_bytes()));
        if required>LIMIT || required>available {
            self.retire(r,&BTreeSet::new(),self.side);return;
        }
        self.retire(r,&wanted,side);
        let missing=wanted.iter().filter(|key|!self.pages.contains_key(key)).count() as u64*bytes;
        let composition=[r.composition_storage_bytes()+self.storage_bytes(),cache.presentation_bytes()+self.storage_bytes()];
        r.memory.sync_report(layer_core::MemoryOwnerKind::Scene,r.display_memory_report(r.scene.as_deref(),composition));
        let Some(reservation)=r.memory.display_owner().admit_working(layer_core::MemoryGrant {ram_bytes:0,gpu_bytes:missing}) else {return;};
        self.admission=Some(admission);
        self.side=side;
        for key in wanted {
            let buffer=self.pages.entry(key).or_insert_with(|| {
                let mut data=vec![0u8;bytes as usize];
                data[..4].copy_from_slice(&side.to_le_bytes());data[4..8].copy_from_slice(&1u32.to_le_bytes());
                r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {label:Some("source changed cells"),contents:&data,
                    usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::COPY_SRC})
            });
            encoder.clear_buffer(buffer,16,None);
        }
        let composition=[r.composition_storage_bytes()+self.storage_bytes(),cache.presentation_bytes()+self.storage_bytes()];
        r.memory.commit_reservation(layer_core::MemoryOwnerKind::Scene,reservation,r.display_memory_report(r.scene.as_deref(),composition)).expect("changed-cell reservation belongs to the display owner");
        for batch in packet.dab_batches {
            if batch.kind!=DabBatchKind::Preview && (!r.in_place_dry_material(batch) || revisits_stroke(&batch.style)) {
                self.force_source(batch.target);
            }
        }
        for &(id,region) in &r.transform_damage {self.force(id,region);}
        if packet.dab_batches.iter().any(|b|!matches!(b.target,SourceTarget::Paint(_)))
            || r.transform_damage.iter().any(|(id,_)|!matches!(id,SourceTarget::Paint(_))) {self.force_all();}
    }
    pub fn prepare_preview(&mut self,r:&WgpuRasterizer,old:Preview<'_>,new:Preview<'_>) {
        for page in &r.preview_pages {
            let warm=old.id==new.id && old.level==new.level && old.contribution && new.contribution
                && old.contains(page.coordinate) && new.contains(page.coordinate) && !page.active_secondary
                && old.id.is_some_and(|id|page.primary.preview()==Some((id,page.coordinate)))
                && page.primary.size().width==PAGE_SIZE>>new.level && page.primary.size().height==PAGE_SIZE>>new.level
                && page.primary.format()==r.device.working_format() && (1<<new.level)<=self.side
                && r.pipelines.dry_material.in_place();
            if !warm {page.primary.set_preview(None);}
            if let Some(secondary)=&page.secondary {secondary.set_preview(None);}
        }
        for state in [&old,&new] {
            let Some(id)=state.id else {continue;};
            for coordinate in page_coordinates(state.damage).filter(|c|state.contains(*c)) {
                if !r.preview_pages.iter().any(|p|p.coordinate==coordinate && p.primary.preview()==Some((id,coordinate))) {
                    self.forced.insert((id,coordinate));
                }
            }
        }
    }
    pub fn preview_reusable(&self,r:&WgpuRasterizer,id:SourceTarget,coordinate:[u32;2],surface:&resident::Resident<PageSurface>)->bool {
        surface.preview()==Some((id,coordinate)) && surface.format()==r.device.working_format()
            && surface.size().width==PAGE_SIZE>>r.preview_level && surface.size().height==PAGE_SIZE>>r.preview_level
            && (1<<r.preview_level)<=self.side && self.reusable(id,coordinate).is_some()
    }
}
