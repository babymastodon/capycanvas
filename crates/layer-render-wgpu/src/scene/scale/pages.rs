use super::*;
use layer_core::raster::ContentVersion;

#[derive(Default)]
pub(super) struct Pixels {
    root: Option<Window>,
    next: Option<Window>,
}
enum Window {
    Allocated(Image),
    Committed(CommittedImage),
}
pub(super) struct CommittedImage {
    image: Image,
    submission: submission::Submission,
    pages: Vec<Arc<Image>>,
    versions:Vec<Recorded>,
    authored:Option<layer_core::raster::ContentStamp>,
}
impl CommittedImage {
    pub(super) fn authored(&self)->Option<layer_core::raster::ContentStamp> {self.authored}
    pub fn image(&self)->&Image {&self.image}
    pub fn view(&self)->&wgpu::TextureView {&self.image.view}
}
impl Window {
    fn image(&self)->&Image {match self {Self::Allocated(image)=>image,Self::Committed(image)=>image.image()}}
    fn committed(&self)->Option<&CommittedImage> {
        match self {Self::Committed(image) if !image.submission.discarded()=>Some(image),_=>None}
    }
    fn commit(&mut self,submission:submission::Submission,pages:Vec<Arc<Image>>,versions:Vec<Recorded>,authored:Option<layer_core::raster::ContentStamp>) {
        *self=Self::Committed(CommittedImage {image:self.image().clone(),submission,pages,versions,authored});
    }
}
pub(crate) struct PresentedPage<'a> {
    pub(super) views: [&'a CommittedImage;3],
    pub(super) geometry: &'a wgpu::Buffer,
}
impl<'a> PresentedPage<'a> {
    pub fn views(&self)->[&'a wgpu::TextureView;3] {self.views.map(CommittedImage::view)}
    pub fn geometry(&self)->&'a wgpu::Buffer {self.geometry}
}
impl Pixels {
    pub fn root(&self) -> Option<&Image> { self.root.as_ref().map(Window::image) }
    pub fn next(&self) -> Option<&Image> { self.next.as_ref().map(Window::image) }
    pub fn committed_root(&self)->Option<&CommittedImage> {self.root.as_ref()?.committed()}
    pub fn committed_next(&self)->Option<&CommittedImage> {self.next.as_ref()?.committed()}
    fn commit_root(&mut self,submission:submission::Submission,pages:Vec<Arc<Image>>,versions:Vec<Recorded>,authored:Option<layer_core::raster::ContentStamp>) {self.root.as_mut().unwrap().commit(submission,pages,versions,authored);}
    pub fn commit_next(&mut self,submission:submission::Submission) {
        let root=self.committed_root().unwrap();let pages=root.pages.clone();let versions=root.versions.clone();let authored=root.authored;
        self.next.as_mut().unwrap().commit(submission,pages,versions,authored);
    }
    pub fn ensure_root(&mut self, r: &WgpuRasterizer, plan: display_mips::Plan, format:wgpu::TextureFormat, _label: &'static str) {
        self.root.get_or_insert_with(|| Window::Allocated(Image::with_format(r,plan,format)));
    }
    pub fn ensure(&mut self, r: &WgpuRasterizer, plan: display_mips::Plan, format:wgpu::TextureFormat) {
        self.ensure_root(r, plan, format,"composition window");
        self.next.get_or_insert_with(|| Window::Allocated(Image::with_format(r,
            display_mips::Plan::window(plan.extent, plan.level + 1, plan.bounds),format)));
    }
    pub fn storage_bytes(&self) -> u64 { self.root.iter().chain(&self.next).map(|window|window.image().bytes()).sum() }
    pub(super) fn take(&mut self)->Vec<Image> {
        self.root.take().into_iter().chain(self.next.take()).map(|window|window.image().clone()).collect()
    }
}

impl Cache {
    #[cfg(test)]
    pub(crate) fn assert_pending_capacity(&self,r:&WgpuRasterizer) {
        if let Some(overview)=self.overview.as_deref().or_else(||(self.plan.level==overview_plan(self.plan).level).then_some(self)) {
            let optional=self.pages.lock().unwrap().optional_bytes(overview.plan.level);
            let window=overview.window_bytes();let share=overview_share(cache_bytes(r));
            assert!(optional.saturating_add(window)<=share,"overview exceeds its share: optional={optional} window={window} share={share}");
        }
        if let Some(scene)=&r.scene {
            let optional=self.pages.lock().unwrap().pages.values().filter(|page|page.optional).map(|page|page.image.bytes()).sum::<u64>();
            assert!(optional==0 || self.store_limit(scene,r,[0;2],0)>0,
                "optional branch pages cannot consume the nearest pending unit's Store allowance: optional={optional}");
        }
    }
    #[cfg(test)]
    pub(crate) fn cached_branches(&self) -> usize {
        self.pages.lock().unwrap().retained_pages().into_iter()
            .filter(|page|page.key.source.is_none() && !page.written.is_empty()
                && Some(&page.key.expression)!=self.graph.root.as_ref() && Some(&page.key.expression)!=self.graph.content.as_ref()
                && self.graph.caches(&page.key.expression))
            .map(|page|page.key.expression).collect::<std::collections::HashSet<_>>().len()
    }
    #[cfg(test)]
    pub(in crate::scene) fn commit_state(&self)->Vec<(u64,[PageSet;3])> {
        let mut state=self.pages.lock().unwrap().retained_pages().into_iter()
            .filter(|page|page.key.source.is_none() && !page.written.is_empty())
            .map(|page|(crate::test_support::coverage_hash(&page.key),[page.written,page.current,page.exact])).collect::<Vec<_>>();
        state.sort_by_key(|(key,_)|*key);state
    }
    #[cfg(test)]
    pub(crate) fn storm_visible_coverage(&self,view:layer_render::ViewState)->[f64;4] {
        let total=crate::test_support::storm_surface_area(PixelRect::full(self.plan.extent),view);
        let Some(selected)=self.presentable() else {return [0.,total,0.,1.];};
        let Some(root)=selected.pixels.committed_root() else {return [0.,total,0.,1.];};
        let Some(published)=crate::test_support::published_coverage(crate::test_support::coverage_hash(&selected.geometry)) else {return [0.,total,0.,0.];};
        if published.receipt!=root.submission.observation_id() || published.pages!=root.pages.iter().map(|page|Arc::as_ptr(page) as usize).collect::<Vec<_>>() {return [0.,total,0.,0.];}
        let mut covered=0.;
        for (index,coordinate) in coordinates(published.plan.bounds,published.plan.level).enumerate() {
            if published.written.get(index/32).is_some_and(|word|word&(1<<(index%32))!=0) {
                let region=bounds(coordinate,published.plan.level,published.plan.extent).intersect(published.plan.bounds);
                covered+=crate::test_support::storm_surface_area(region,view);
            }
        }
        [covered,total,if total>0. {covered/total}else {0.},1.]
    }

    #[cfg(test)]
    pub(crate) fn presentable_pages(&self) -> usize {
        let coverage=self.coverage_snapshot();
        usize::from(self.pixels.committed_root().is_some())*coordinates(self.plan.bounds,self.plan.level)
            .filter(|coordinate|Coverage::covers(&coverage.written,bounds(*coordinate,self.plan.level,self.plan.extent))).count()
            +self.overview.as_ref().map_or(0,|cache|cache.presentable_pages())
    }
    #[cfg(test)]
    pub(crate) fn prepared_pages(&self) -> usize {
        self.graph.prepared.iter().flatten().map(|half| coordinates(self.plan.bounds,self.plan.level).filter(|coordinate|self.branch_ready(half,bounds(*coordinate,self.plan.level,self.plan.extent))).count()).sum()
    }
    pub(crate) fn resident_bytes(&self) -> u64 { self.pixels.storage_bytes() }
    pub(crate) fn working_bytes(&self) -> u64 { self.storage_bytes().saturating_sub(self.resident_bytes()) }
    pub(super) fn output_plan(&self) -> display_mips::Plan { self.pixels.root().map_or(self.plan, |i| i.plan) }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub source: Option<SourceTarget>,
    pub expression: graph::Node,
    pub extent: [u32; 2],
    pub color: layer_core::color::DocumentColor,
    pub blend_space: layer_core::BlendSpace,
    pub level: u32,
    pub coordinate: [u32; 2],
    pub format: wgpu::TextureFormat,
}
impl Key {
    pub fn plan(&self) -> display_mips::Plan {display_mips::Plan::window(self.extent,self.level,bounds(self.coordinate,self.level,self.extent))}
    fn finer(&self)->impl Iterator<Item=Self>+'_ {children(self.coordinate,self.level,self.extent).map(|coordinate|Self {coordinate,level:self.level-1,..self.clone()})}

}

#[derive(Clone)]
pub(super) struct Interest {
    pub key:Key,
    evaluation:Evaluation,
    region:PixelRect,
    input_level:u32,
}
impl Interest {
    pub(super) fn new(key:Key,evaluation:Evaluation,region:PixelRect,input_level:u32)->Self {Self {key,evaluation,region,input_level}}
    pub(super) fn region(&self)->PixelRect {self.region}
    fn contains(&self,accepted:&Self)->bool {
        let wanted=&self.key;let key=&accepted.key;
        key.source==wanted.source && key.expression==wanted.expression && key.extent==wanted.extent && key.color==wanted.color
            && key.blend_space==wanted.blend_space && key.level==wanted.level && key.format==wanted.format
            && accepted.input_level==self.input_level && (key.source.is_some() || accepted.evaluation==self.evaluation)
            && if key.source.is_some() {accepted.region.intersect(self.region)==accepted.region}else {!accepted.region.intersect(self.region).is_empty()}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::scene) enum Accuracy { Display, Exact }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Freshness { Stale, Current }

pub(super) fn coordinates(bounds:PixelRect,level:u32)->impl DoubleEndedIterator<Item=[u32;2]>+Clone {
    let side=1<<level;
    page_coordinates(PixelRect::new(bounds.min_x()/side,bounds.min_y()/side,bounds.max_x().div_ceil(side),bounds.max_y().div_ceil(side)))
}
pub(super) fn bounds(coordinate:[u32;2],level:u32,extent:[u32;2])->PixelRect {
    let span=PAGE_SIZE<<level;let [x,y]=coordinate.map(|value|value*span);
    PixelRect::new(x,y,x.saturating_add(span).min(extent[0]),y.saturating_add(span).min(extent[1]))
}
fn children(coordinate:[u32;2],level:u32,extent:[u32;2])->impl Iterator<Item=[u32;2]> {
    (0..2).flat_map(move|y|(0..2).map(move|x|[coordinate[0]*2+x,coordinate[1]*2+y]))
        .filter(move|coordinate|coordinate.into_iter().zip(extent).all(|(coordinate,extent)|u64::from(*coordinate)*u64::from(PAGE_SIZE<<(level-1))<u64::from(extent)))
}
#[derive(Clone,Default,Debug,PartialEq,Eq)]
pub(in crate::scene) struct PageSet {
    level:u32,
    entries:std::collections::HashSet<[u32;2]>,
}
impl std::ops::Deref for PageSet {
    type Target=std::collections::HashSet<[u32;2]>;
    fn deref(&self)->&Self::Target {&self.entries}
}
impl PageSet {
    fn new(level:u32)->Self {Self {level,entries:Default::default()}}
    pub(super) fn regions(&self,area:PixelRect)->Vec<PixelRect> {
        let side=1<<self.level;
        let texels=PixelRect::new(area.min_x()/side,area.min_y()/side,area.max_x().div_ceil(side),area.max_y().div_ceil(side));
        page_regions(self.entries.iter().copied(),texels).into_iter().map(|region|PixelRect::new(
            region.min_x()*side,region.min_y()*side,region.max_x()*side,region.max_y()*side).intersect(area)).collect()
    }
}
#[derive(Clone,Default)]
pub(in crate::scene) struct Coverage {
    pub written:PageSet,
    pub matching:PageSet,
    pub exact:PageSet,
    pending:PageSet,
}
impl Coverage {
    pub(super) fn new(level:u32)->Self {Self {written:PageSet::new(level),matching:PageSet::new(level),exact:PageSet::new(level),pending:PageSet::new(level)}}
    pub fn covers(pages:&PageSet,area:PixelRect)->bool {coordinates(area,pages.level).all(|coordinate|pages.contains(&coordinate))}
    pub fn ready(&self,area:PixelRect,accuracy:Accuracy)->bool {
        coordinates(area,self.matching.level).all(|coordinate|self.matching.contains(&coordinate) && (accuracy==Accuracy::Display || self.exact.contains(&coordinate)))
    }
    fn include(&mut self,coordinate:[u32;2],matches:Option<bool>,exact:bool) {
        match matches {
            Some(true)=>{self.matching.entries.insert(coordinate);if exact {self.exact.entries.insert(coordinate);}},
            None=>{self.pending.entries.insert(coordinate);},Some(false)=>{}
        }
    }
    pub fn needs(&self,area:PixelRect,accuracy:Accuracy)->bool {
        coordinates(area,self.matching.level).any(|coordinate|!self.pending.contains(&coordinate)
            && (!self.matching.contains(&coordinate) || accuracy==Accuracy::Exact && !self.exact.contains(&coordinate)))
    }
}
#[derive(Clone)]
struct Recorded {
    key:Key,
    region:PixelRect,
    written:PixelRect,
    input_level:u32,
    version:ContentVersion,
    authored:layer_core::raster::ContentStamp,
}
impl Recorded {
    fn clipped(&self,area:PixelRect)->Option<Self> {
        let written=self.written.intersect(area);
        (!written.is_empty()).then(||Self {written,..self.clone()})
    }
    fn matches(&self,frame:&SignatureFrame,required_input_level:u32)->Option<bool> {
        if self.input_level>required_input_level {return Some(false);}
        let expected=match frame.expected_key(&self.key,self.region,self.input_level) {Ok(expected)=>expected,Err(_)=>return Some(false)};
        if self.version.authored()!=expected.version().authored() {return Some(false);}
        frame.matches(&self.version,&expected)
    }
}
fn recorded_matches(records:&[Recorded],frame:&SignatureFrame,area:PixelRect,required_input_level:u32)->Option<bool> {
    let mut missing=vec![area];let mut pending=false;
    for recorded in records {
        if recorded.written.intersect(area).is_empty() {continue;}
        match recorded.matches(frame,required_input_level) {Some(false)=>return Some(false),None=>pending=true,Some(true)=>{}}
        missing=missing.into_iter().flat_map(|part|part.subtract(recorded.written)).filter(|part|!part.is_empty()).collect();
    }
    if !missing.is_empty() {Some(false)}else {(!pending).then_some(true)}
}
fn authored_matches(records:&[Recorded],frame:&SignatureFrame,authored:layer_core::raster::ContentStamp,area:PixelRect)->bool {
    let mut missing=vec![area];
    for recorded in records {
        if recorded.written.intersect(area).is_empty() {continue;}
        if recorded.authored!=authored {
            if frame.authored_revision(&recorded.key.expression).ok()!=Some(authored) {return false;}
            let Ok(expected)=frame.expected_key(&recorded.key,recorded.region,recorded.input_level) else {return false;};
            if recorded.version.authored()!=expected.version().authored() {return false;}
        }
        missing=missing.into_iter().flat_map(|part|part.subtract(recorded.written)).filter(|part|!part.is_empty()).collect();
    }
    missing.is_empty()
}
fn visible_projection_complete(plan:display_mips::Plan,visible:PixelRect,written:&[u32])->bool {
    let visible=visible.intersect(plan.bounds);
    coordinates(plan.bounds,plan.level).enumerate().all(|(index,coordinate)|
        bounds(coordinate,plan.level,plan.extent).intersect(visible).is_empty()
            || written.get(index/32).is_some_and(|word|word&(1<<(index%32))!=0))
}
fn publication_matches(frame:&SignatureFrame,key:&Key,area:PixelRect,input_level:u32,held:Option<(layer_core::raster::ContentStamp,&[Recorded])>)->Option<bool> {
    let Ok(authored)=frame.authored_revision(&key.expression) else {return Some(false);};
    match held {
        Some((previous,records)) if previous==authored=> {
            if records.iter().any(|record|record.key.level!=key.level && !record.written.intersect(area).is_empty()) {return Some(false);}
            recorded_matches(records,frame,area,input_level)
        },
        _=>Some(false),
    }
}
#[derive(Clone)]
pub(super) struct Accepted {
    frame:Arc<SignatureFrame>,
    recorded:Recorded,
}
#[derive(Clone)]
struct WrittenCoverage {
    written:PageSet,
    exact:PageSet,
}
impl WrittenCoverage {
    fn new(level:u32)->Self {Self {written:PageSet::new(level),exact:PageSet::new(level)}}
    fn commit(&mut self,area:PixelRect,accuracy:Accuracy) {
        for coordinate in coordinates(area,self.written.level) {
            self.written.entries.insert(coordinate);self.exact.entries.remove(&coordinate);
            if accuracy==Accuracy::Exact {self.exact.entries.insert(coordinate);}
        }
    }
}
struct Page {
    image: Arc<Image>,
    optional: bool,
    coverage: WrittenCoverage,
    versions:std::collections::HashMap<[u32;2],Recorded>,
    used: u64,
    written_at: u64,
    submission: Option<submission::Submission>,
    last_use: Option<submission::Submission>,
    inputs:Vec<Inputs>,
}
impl Page {
    fn matches(&self,frame:Option<&SignatureFrame>,area:PixelRect,required_input_level:u32)->Option<bool> {
        if self.submission.as_ref().is_some_and(submission::Submission::discarded)
            || !Coverage::covers(&self.coverage.written,area) {return Some(false);}
        let Some(frame)=frame else {return Some(false);};let mut pending=false;
        for coordinate in coordinates(area,self.coverage.written.level) {
            let cell=bounds(coordinate,self.coverage.written.level,self.image.plan.extent).intersect(area);
            let Some(recorded)=self.versions.get(&coordinate).filter(|recorded|recorded.written.intersect(cell)==cell) else {return Some(false);};
            match recorded.matches(frame,required_input_level) {Some(false)=>return Some(false),None=>pending=true,Some(true)=>{}}
        }
        (!pending).then_some(true)
    }
}
struct Inputs {
    region:PixelRect,
    evaluation:Evaluation,
    sources:crate::scene::sources::AcceptedInputs,
    pages:Vec<(Key,PixelRect,Arc<Image>)>,
    accepted:Accepted,
}

impl Inputs {
    fn interest(&self)->Interest {Interest::new(self.accepted.recorded.key.clone(),self.evaluation,self.region,self.accepted.recorded.input_level)}
}

type Columns = BTreeMap<u32,BTreeMap<u32,Vec<Key>>>;
#[derive(Default)]
struct Index {
    owners:std::collections::HashMap<(Option<SourceTarget>,[u32;2]),BTreeMap<u32,Columns>>,
    allocated:u64,
}
fn tree_entry_bytes<T>()->u64 {(std::mem::size_of::<T>()+3*std::mem::size_of::<usize>()) as u64}
impl Index {
    fn insert(&mut self,key:&Key) {
        let allocated=&mut self.allocated;
        let levels=self.owners.entry((key.source,key.extent)).or_default();
        let columns=levels.entry(key.level).or_insert_with(|| {*allocated+=tree_entry_bytes::<(u32,Columns)>();Default::default()});
        let rows=columns.entry(key.coordinate[0]).or_insert_with(|| {*allocated+=tree_entry_bytes::<(u32,BTreeMap<u32,Vec<Key>>)>();Default::default()});
        let keys=rows.entry(key.coordinate[1]).or_insert_with(|| {*allocated+=tree_entry_bytes::<(u32,Vec<Key>)>();Vec::new()});
        debug_assert!(!keys.contains(key));
        let before=keys.capacity();
        keys.push(key.clone());
        *allocated+=(keys.capacity()-before) as u64*std::mem::size_of::<Key>() as u64;
    }
    fn remove(&mut self,key:&Key) {
        let owner=(key.source,key.extent);
        let levels=self.owners.get_mut(&owner).unwrap();
        let columns=levels.get_mut(&key.level).unwrap();
        let rows=columns.get_mut(&key.coordinate[0]).unwrap();
        let keys=rows.get_mut(&key.coordinate[1]).unwrap();
        keys.retain(|candidate|candidate!=key);
        if keys.is_empty() {
            self.allocated-=tree_entry_bytes::<(u32,Vec<Key>)>()+keys.capacity() as u64*std::mem::size_of::<Key>() as u64;
            rows.remove(&key.coordinate[1]);
        }
        if rows.is_empty() {self.allocated-=tree_entry_bytes::<(u32,BTreeMap<u32,Vec<Key>>)>();columns.remove(&key.coordinate[0]);}
        if columns.is_empty() {self.allocated-=tree_entry_bytes::<(u32,Columns)>();levels.remove(&key.level);}
        if levels.is_empty() {self.owners.remove(&owner);}
    }
    fn overlapping<'a>(&'a self,key:&Key,area:PixelRect)->impl Iterator<Item=&'a Key>+'a {
        self.owners.get(&(key.source,key.extent)).into_iter().flat_map(|levels|levels.iter())
            .flat_map(move|(level,columns)| {
                let span=PAGE_SIZE<<level;
                columns.range(area.min_x()/span..area.max_x().div_ceil(span)).flat_map(move|(_,rows)|
                    rows.range(area.min_y()/span..area.max_y().div_ceil(span)).flat_map(|(_,keys)|keys))
            }).inspect(|_| {
                #[cfg(test)]
                crate::test_support::PassCounts {store_page_visits:1,..Default::default()}.record();
            })
    }
    fn storage_bytes(&self)->u64 {
        self.allocated+self.owners.capacity() as u64*(std::mem::size_of::<((Option<SourceTarget>,[u32;2]),BTreeMap<u32,Columns>)>()+1) as u64
    }
}
#[derive(Default)]
pub(in crate::scene) struct Store {
    pages: std::collections::HashMap<Key, Page>,
    index: Index,
    clock: u64,
    bytes: u64,
    expected:Option<Arc<SignatureFrame>>,
}
impl Store {
    pub(in crate::scene) fn set_expected(&mut self,frame:Arc<SignatureFrame>) {
        self.expected=Some(frame);
    }
    pub(in crate::scene) fn expected(&self)->Option<&Arc<SignatureFrame>> {self.expected.as_ref()}
    fn capture(&self,key:&Key,region:PixelRect,input_level:u32)->Result<Accepted,GpuRasterError> {
        let frame=self.expected.as_ref().ok_or(GpuRasterError::Invariant("signature frame is missing or inconsistent"))?.clone();
        let version=frame.expected_key(key,region,input_level)?.into_version();let authored=frame.authored_revision(&key.expression)?;
        Ok(Accepted {frame,recorded:Recorded {key:key.clone(),region,written:region,input_level,version,authored}})
    }
    pub(super) fn accept_version(&mut self,interest:&Interest)->Result<Option<Accepted>,GpuRasterError> {
        let Interest {key,region,input_level,..}=interest;let (region,input_level)=(*region,*input_level);
        let frame=self.expected.as_ref().ok_or(GpuRasterError::Invariant("signature frame is missing or inconsistent"))?;
        if let Some(page)=self.pages.get_mut(key) {
            page.inputs.retain(|input|input.region!=region || input.accepted.recorded.input_level==input_level
                && input.accepted.recorded.matches(frame,input_level)!=Some(false));
            if let Some(input)=page.inputs.iter().find(|input|input.region==region) {
                return Ok(input.accepted.recorded.matches(frame,input_level).map(|_|input.accepted.clone()));
            }
        }
        self.capture(key,region,input_level).map(Some)
    }
    pub(super) fn same_authored(&self,key:&Key,area:PixelRect)->bool {
        let Some(page)=self.pages.get(key) else {return false;};
        let Some(frame)=self.expected.as_ref() else {return false;};
        if !Coverage::covers(&page.coverage.written,area) {return false;}
        coordinates(area,page.coverage.written.level).all(|coordinate|page.versions.get(&coordinate).is_some_and(|recorded|
            frame.expected_key(&recorded.key,recorded.region,recorded.input_level).is_ok_and(|expected|recorded.version.authored()==expected.version().authored())))
    }
    pub(super) fn authored_mismatch(&self,key:&Key,area:PixelRect)->bool {
        let Some(page)=self.pages.get(key) else {return false;};
        let Some(frame)=self.expected.as_ref() else {return false;};
        page.versions.values().filter(|recorded|!recorded.written.intersect(area).is_empty()).any(|recorded|
            frame.expected_key(&recorded.key,recorded.region,recorded.input_level).is_ok_and(|expected|recorded.version.authored()!=expected.version().authored()))
    }
    pub(super) fn prepare_write(&self,key:Key,region:PixelRect,accuracy:Accuracy,freshness:Freshness,input_level:u32)->Result<Write,GpuRasterError> {
        let accepted=if freshness==Freshness::Current {
            Some(if let Some(input)=self.pages.get(&key).and_then(|page|page.inputs.iter().find(|input|input.region==region)) {
                if input.accepted.recorded.input_level!=input_level {return Err(GpuRasterError::Invariant("prepared prepare write state is inconsistent"));}
                input.accepted.clone()
            }else {self.capture(&key,region,input_level)?})
        }else {None};
        Ok(Write {key,region,accuracy,accepted})
    }
    pub(super) fn coverage(&self,key:&Key,area:PixelRect,required_input_level:u32)->Coverage {
        let mut coverage=Coverage::new(key.level);
        let mut written:Vec<_>=(0..=display_mips::MAX_LEVEL).map(PageSet::new).collect();
        for candidate in self.index.overlapping(key,area) {
            let page=&self.pages[candidate];
            if candidate.source!=key.source || candidate.extent!=key.extent || page.submission.as_ref().is_some_and(submission::Submission::discarded)
                || candidate.plan().bounds.intersect(area).is_empty() {continue;}
            if page.optional && candidate.expression!=key.expression {continue;}
            written[candidate.level as usize].entries.extend(page.coverage.written.iter().copied());
            if candidate.expression!=key.expression || candidate.color!=key.color || candidate.blend_space!=key.blend_space
                || candidate.level!=key.level || candidate.format!=key.format && candidate.format!=wgpu::TextureFormat::Rgba32Float {continue;}
            for coordinate in coordinates(candidate.plan().bounds.intersect(area),coverage.matching.level) {
                let region=bounds(coordinate,coverage.matching.level,key.extent).intersect(area);
                coverage.include(coordinate,page.matches(self.expected.as_deref(),region,required_input_level),
                    candidate.format==wgpu::TextureFormat::Rgba32Float && Coverage::covers(&page.coverage.exact,region));
            }
        }
        for level in 1..=key.level {
            let parents=written[level as usize-1].iter().map(|coordinate|coordinate.map(|value|value/2)).collect::<std::collections::HashSet<_>>();
            for coordinate in parents {
                if children(coordinate,level,key.extent).all(|child|written[level as usize-1].contains(&child)) {written[level as usize].entries.insert(coordinate);}
            }
        }
        for coordinate in coordinates(area,key.level) {
            if (key.level..=display_mips::MAX_LEVEL).any(|level|written[level as usize].contains(&coordinate.map(|value|value>>(level-key.level)))) {
                coverage.written.entries.insert(coordinate);
            }
        }
        coverage
    }
    pub(super) fn initialized(&self,key:&Key,view:&wgpu::TextureView,bounds:PixelRect)->Option<BTreeSet<[u32;2]>> {
        let page=self.pages.get(key).filter(|page|page.image.view==*view)?;
        Some(page_coordinates(bounds).filter(|coordinate|Coverage::covers(&page.coverage.written,
            page_rect(*coordinate).intersect(bounds))).collect())
    }
    pub(super) fn initialized_view(&self,view:&wgpu::TextureView,bounds:PixelRect)->Option<BTreeSet<[u32;2]>> {
        self.pages.iter().find(|(_,page)|page.image.view==*view).and_then(|(key,_)|self.initialized(key,view,bounds))
    }
    pub(super) fn optional_bytes(&self,level:u32)->u64 {
        self.pages.iter().filter(|(key,page)|key.level==level && page.optional).map(|(_,page)|page.image.bytes()).sum()
    }
    pub(super) fn reserve_optional(&mut self,key:&Key,limit:u64)->bool {
        let bytes=key.plan().size.map(u64::from).into_iter().product::<u64>()*u64::from(key.format.block_copy_size(None).unwrap());
        self.trim_to(limit.saturating_sub(bytes),true,None);
        self.bytes.saturating_add(bytes)<=limit
    }
    pub(super) fn set_optional(&mut self,key:&Key) {self.pages.get_mut(key).unwrap().optional=true;}
    pub(super) fn set_visible(&mut self,key:&Key) {if let Some(page)=self.pages.get_mut(key) {page.optional=false;}}
    pub fn bytes(&self) -> u64 { self.bytes }
    pub(in crate::scene) fn metadata_bytes(&self)->u64 {
        self.index.storage_bytes()+self.pages.capacity() as u64*(std::mem::size_of::<(Key,Page)>()+1) as u64
            +self.pages.len() as u64*(std::mem::size_of::<Image>()+2*std::mem::size_of::<usize>()) as u64
            +self.pages.values().map(|page|page.versions.capacity() as u64*(std::mem::size_of::<([u32;2],Recorded)>()+1) as u64).sum::<u64>()
    }
    pub fn pinned_bytes(&self)->u64 {
        self.pages.values().filter(|page|!page.inputs.is_empty() || Arc::strong_count(&page.image)>1 || page.submission.as_ref().is_some_and(submission::Submission::pending)
            || page.last_use.as_ref().is_some_and(submission::Submission::pending)).map(|page|page.image.bytes()).sum()
    }
    pub(super) fn read(&mut self,key:&Key,image:&Arc<Image>,submission:submission::Submission) {
        if let Some(page)=self.pages.get_mut(key).filter(|page|Arc::ptr_eq(&page.image,image)) {page.last_use=Some(submission);}
    }
    pub(super) fn allocated(&mut self,key:&Key)->Option<Arc<Image>> {
        let page=self.pages.get_mut(key)?;
        self.clock+=1;page.used=self.clock;Some(page.image.clone())
    }
    pub(super) fn claim(&mut self, r: &WgpuRasterizer, key: Key, limit: u64) -> Result<Arc<Image>, GpuRasterError> {
        if let Some(image)=self.allocated(&key) {return Ok(image);}
        self.clock+=1;
        let plan=key.plan();
        let bytes=plan.size.map(u64::from).into_iter().product::<u64>()*u64::from(key.format.block_copy_size(None).unwrap());
        self.trim(limit.saturating_sub(bytes));
        if bytes>limit || self.bytes().saturating_add(bytes)>limit {return Err(GpuRasterError::NotReady);}
        let image=Arc::new(Image::with_format(r,plan,key.format));
        self.bytes+=bytes;
        let coverage=WrittenCoverage::new(if key.source.is_some() {0}else {key.level});
        self.index.insert(&key);
        self.pages.insert(key,Page {image:image.clone(),optional:false,coverage,versions:Default::default(),used:self.clock,written_at:0,submission:None,last_use:None,inputs:Vec::new()});
        Ok(image)
    }
    pub(super) fn get(&mut self,key:&Key,area:PixelRect,accuracy:Accuracy,required_input_level:u32)->Option<Arc<Image>> {
        let exact=Key {format:wgpu::TextureFormat::Rgba32Float,..key.clone()};
        let key=if accuracy==Accuracy::Exact {&exact}else{key};
        let page=self.pages.get_mut(key)?;
        if page.matches(self.expected.as_deref(),area,required_input_level)!=Some(true)
            || accuracy==Accuracy::Exact && !Coverage::covers(&page.coverage.exact,area) {return None;}
        self.clock+=1;page.used=self.clock;Some(page.image.clone())
    }
    fn presented(&mut self,key:&Key,area:PixelRect)->Option<Arc<Image>> {
        let page=self.pages.get_mut(key)?;
        if page.submission.as_ref().is_some_and(|submission|submission.discarded()) || !Coverage::covers(&page.coverage.written,area) {return None;}
        self.clock+=1;page.used=self.clock;Some(page.image.clone())
    }
    fn regions(&self,key:&Key,area:PixelRect)->Vec<PixelRect> {
        self.pages.get(key).filter(|page|page.submission.as_ref().is_none_or(|submission|!submission.discarded()))
            .map_or_else(Vec::new,|page|page.coverage.written.regions(area.intersect(key.plan().bounds)))
    }
    fn project_candidate(&mut self,key:&Key,area:PixelRect,missing:&mut Vec<PixelRect>,selected:&mut Vec<Projection>) {
        for region in self.regions(key,area) {
            let pieces=missing.iter().map(|part|part.intersect(region)).filter(|part|!part.is_empty()).collect::<Vec<_>>();
            for part in pieces {
                if let Some(image)=self.presented(key,part) {
                    let versions=self.pages[key].versions.values().filter_map(|recorded|recorded.clipped(part)).collect();
                    selected.push(Projection {region:part,image,key:key.clone(),versions});
                    *missing=missing.iter().flat_map(|area|area.subtract(part)).filter(|area|!area.is_empty()).collect();
                }
            }
        }
    }
    fn projection(&mut self,key:&Key,area:PixelRect,required_input_level:u32)->(Vec<Projection>,bool) {
        let full=Key {format:wgpu::TextureFormat::Rgba32Float,..key.clone()};
        let exact=self.pages.get(&full).map_or_else(Vec::new,|page| {
            page.coverage.exact.regions(area).into_iter().filter(|region|page.matches(self.expected.as_deref(),*region,0)==Some(true)).collect()
        });
        let mut missing=vec![area];let mut selected=Vec::new();
        for region in exact {
            if let Some(image)=self.get(&full,region,Accuracy::Exact,0) {
                let versions=self.pages[&full].versions.values().filter_map(|recorded|recorded.clipped(region)).collect();
                selected.push(Projection {region,image,key:full.clone(),versions});
                missing=missing.into_iter().flat_map(|area|area.subtract(region)).filter(|area|!area.is_empty()).collect();
            }
        }
        let authored=self.expected.as_ref().and_then(|frame|frame.authored_revision(&key.expression).ok());
        let mut candidates=self.index.overlapping(key,area).filter_map(|candidate| {
            let page=&self.pages[candidate];
            (!page.optional || candidate.expression==key.expression).then_some((candidate,page))
        }).filter(|(_,page)|!page.coverage.written.is_empty())
            .map(|(candidate,page)| {
                let region=candidate.plan().bounds.intersect(area);
                let matching=page.matches(self.expected.as_deref(),region,required_input_level)==Some(true);
                let versions=page.versions.values().filter_map(|recorded|recorded.clipped(region)).collect::<Vec<_>>();
                let same=authored.zip(self.expected.as_deref()).is_some_and(|(authored,frame)|authored_matches(&versions,frame,authored,region));
                ((!matching,!same,candidate.level<key.level,candidate.level.abs_diff(key.level),
                    candidate.expression!=key.expression || candidate.color!=key.color || candidate.blend_space!=key.blend_space,
                    std::cmp::Reverse(page.written_at)),candidate.clone())
            }).collect::<Vec<_>>();
        candidates.sort_by_key(|(order,_)|*order);
        for (_,candidate) in candidates {
            if missing.is_empty() {break;}
            self.project_candidate(&candidate,area,&mut missing,&mut selected);
        }
        (selected,missing.is_empty())
    }
    pub(super) fn hold_inputs(&mut self,key:&Key,region:PixelRect,_evaluation:Evaluation,sources:crate::scene::sources::AcceptedInputs)->Result<(),GpuRasterError> {
        if sources.is_empty() {return Ok(());}
        let page=self.pages.get_mut(key).ok_or(GpuRasterError::Invariant("prepared hold inputs state is inconsistent"))?;
        let held=page.inputs.iter_mut().find(|held|held.region==region).ok_or(GpuRasterError::Invariant("accepted unit inputs are missing"))?;
        held.sources=sources;Ok(())
    }
    pub(super) fn take_inputs(&mut self,key:&Key,region:PixelRect)->crate::scene::sources::AcceptedInputs {
        let Some(page)=self.pages.get_mut(key) else {return Default::default();};
        let Some(index)=page.inputs.iter().position(|inputs|inputs.region==region) else {return Default::default();};
        if page.inputs[index].pages.is_empty() {page.inputs.swap_remove(index).sources}
        else {std::mem::take(&mut page.inputs[index].sources)}
    }
    #[cfg(test)]
    pub(super) fn accepted(&self,key:&Key,region:PixelRect)->bool {
        self.pages.get(key).is_some_and(|page|page.inputs.iter().any(|input|input.region==region && !input.pages.is_empty()))
    }
    pub(super) fn accepted_coordinates<'a>(&'a self,key:&'a Key)->impl Iterator<Item=[u32;2]>+'a {
        self.pages.iter().filter_map(|(candidate,page)|
            (*candidate==Key {coordinate:candidate.coordinate,..key.clone()} && page.inputs.iter().any(|input|!input.pages.is_empty())).then_some(candidate.coordinate))
    }
    pub(super) fn reserve_inputs(&mut self,keys:&[Key],limit:u64)->Option<(Vec<(Key,Arc<Image>)>,Vec<Key>,u64)> {
        let mut held=Vec::new();let mut missing=Vec::new();let mut bytes=0;
        for key in keys {
            if let Some(page)=self.pages.get(key) {held.push((key.clone(),page.image.clone()));}
            else {
                bytes+=key.plan().size.map(u64::from).into_iter().product::<u64>()*u64::from(key.format.block_copy_size(None).unwrap());
                missing.push(key.clone());
            }
        }
        self.trim(limit.saturating_sub(bytes));
        (self.bytes.saturating_add(bytes)<=limit).then_some((held,missing,bytes))
    }
    pub(super) fn accept(&mut self,interest:&Interest,images:Vec<(Key,PixelRect,Arc<Image>)>,accepted:Accepted)->Result<(),GpuRasterError> {
        let Interest {key,region,evaluation,input_level}=interest;let (region,evaluation)=(*region,*evaluation);
        debug_assert!(accepted.recorded.key==*key && accepted.recorded.region==region && accepted.recorded.input_level==*input_level);
        if self.expected.as_ref().is_none_or(|frame|accepted.recorded.matches(frame,accepted.recorded.input_level)==Some(false)) {return Err(GpuRasterError::Invariant("signature frame is missing or inconsistent"));}
        let page=self.pages.get_mut(key).unwrap();
        if let Some(held)=page.inputs.iter_mut().find(|held|held.region==region) {held.pages=images;debug_assert_eq!(held.accepted.recorded.version.identity(),accepted.recorded.version.identity());}
        else {page.inputs.push(Inputs {region,evaluation,pages:images,sources:Default::default(),accepted});}
        Ok(())
    }
    pub(super) fn finish_inputs(&mut self,key:&Key,region:PixelRect) {
        if let Some(page)=self.pages.get_mut(key) {page.inputs.retain(|input|input.region!=region);}
    }
    pub(super) fn retain_work(&mut self,windows:&[Interest]) {
        let expected=self.expected.clone();
        for page in self.pages.values_mut() {
            if page.inputs.is_empty() {continue;}
            page.inputs.retain(|input| {
                let matches=expected.as_ref().is_some_and(|frame|input.accepted.recorded.matches(frame,input.accepted.recorded.input_level)!=Some(false));
                let interest=input.interest();
                let retained=matches && windows.iter().any(|wanted|wanted.contains(&interest));
                debug_assert!(retained || !matches || expected.as_ref().is_none_or(|frame|!Arc::ptr_eq(frame,&input.accepted.frame)),
                    "same-frame preparation pruned a still-matching accepted producer");
                retained
            });
        }
        self.retain_referenced_inputs();
        let unwanted=self.pages.iter().filter(|(_,page)|page.coverage.written.is_empty() && page.inputs.is_empty() && Arc::strong_count(&page.image)==1)
            .map(|(key,_)|key.clone()).collect::<Vec<_>>();
        for key in unwanted {self.remove(&key).image.texture.destroy();}
    }
    fn retain_referenced_inputs(&mut self) {
        let mut accepted=std::collections::HashMap::<Key,Vec<PixelRect>>::new();
        let mut pending=self.pages.iter().filter(|(key,_)|key.source.is_none()).flat_map(|(_,page)|&page.inputs)
            .flat_map(|input|&input.pages).filter(|(key,_,_)|key.source.is_some()).map(|(key,area,_)|(key.clone(),*area)).collect::<Vec<_>>();
        while let Some((key,area))=pending.pop() {
            let regions=accepted.entry(key.clone()).or_default();
            if regions.iter().any(|region|region.intersect(area)==area) {continue;}
            regions.push(area);
            if let Some(page)=self.pages.get(&key) {
                pending.extend(page.inputs.iter().filter(|input|!input.region.intersect(area).is_empty()).flat_map(|input|&input.pages)
                    .filter(|(child,_,_)|child.source.is_some()).map(|(child,area,_)|(child.clone(),*area)));
            }
        }
        for (key,page) in &mut self.pages {if key.source.is_some() {
            page.inputs.retain(|input|accepted.get(key).is_some_and(|areas|areas.iter().any(|area|!area.intersect(input.region).is_empty())));
        }}
    }
    pub(super) fn commit(&mut self,write:Write,submission:submission::Submission) {
        let Write {key,region:area,accuracy,accepted}=write;
        assert!(if key.source.is_some() {page_coordinates(area).count()==1 && area==page_rect([area.min_x()/PAGE_SIZE,area.min_y()/PAGE_SIZE]).intersect(PixelRect::full(key.extent))}
            else {area==key.plan().bounds},"a committed unit must be completely written");
        let page=self.pages.get_mut(&key).unwrap();
        self.clock+=1;page.written_at=self.clock;
        page.coverage.commit(area,accuracy);page.submission=Some(submission);
        let coordinate=coordinates(area,page.coverage.written.level).next().expect("nonempty committed unit");
        page.versions.remove(&coordinate);
        if let Some(Accepted {frame,recorded})=accepted {
            assert!(recorded.key==key && recorded.region==area);
            debug_assert!(recorded.version.matches(frame.expected_key(&key,area,recorded.input_level).expect("accepted input version").version())==Some(true));
            page.versions.insert(coordinate,recorded);
        }
        page.inputs.retain(|input|input.region.intersect(area)!=input.region);
    }
    pub(super) fn trim(&mut self,limit:u64) {self.trim_to(limit,false,None);}
    fn remove(&mut self,key:&Key)->Page {
        let page=self.pages.remove(key).unwrap();
        self.index.remove(key);self.bytes-=page.image.bytes();page
    }
    pub(super) fn trim_optional(&mut self,level:u32,limit:u64)->u64 {
        let before=self.bytes();self.trim_to(limit,true,Some(level));before-self.bytes()
    }
    fn trim_to(&mut self,limit:u64,optional_only:bool,level:Option<u32>) {
        let mut bytes=level.map_or_else(||self.bytes(),|level|self.optional_bytes(level));
        while bytes>limit {
            let Some(key)=self.pages.iter().filter(|(key,page)|level.is_none_or(|level|key.level==level) && (!optional_only || page.optional) && page.inputs.is_empty() && Arc::strong_count(&page.image)==1
                    && page.submission.as_ref().is_none_or(|submission|!submission.pending()) && page.last_use.as_ref().is_none_or(|submission|!submission.pending()))
                .min_by_key(|(_,page)|(!page.optional,page.used)).map(|(key,_)|key.clone()) else {break;};
            let removed=self.remove(&key).image;
            bytes-=removed.bytes();removed.texture.destroy();
        }
    }
    pub(super) fn refinement_frontier(&mut self,key:&Key,limit:usize,pages:&mut Vec<Key>,required_input_level:u32) {
        if pages.len()>=limit || self.get(key,key.plan().bounds,Accuracy::Exact,0).is_some()
            || self.pages.get(key).is_some_and(|page|page.matches(self.expected.as_deref(),key.plan().bounds,required_input_level).is_none()) {return;}
        if key.level==0 || matches!(self.reduction(key,required_input_level),Some((_,Accuracy::Exact,Freshness::Current))) {
            if !pages.contains(key) {pages.push(key.clone());}return;
        }
        for child in key.finer() {self.refinement_frontier(&child,limit,pages,required_input_level);if pages.len()>=limit {break;}}
    }
    pub(super) fn reduction(&mut self,key:&Key,required_input_level:u32)->Option<(Vec<(Key,Arc<Image>)>,Accuracy,Freshness)> {
        if key.level==0 {return None;}
        let mut accuracy=Accuracy::Exact;
        let mut freshness=Freshness::Current;
        let mut images=Vec::new();
        for child in key.finer() {
            let area=child.plan().bounds;
            let full=Key {format:wgpu::TextureFormat::Rgba32Float,..child.clone()};
            let (image,selected)=if let Some(image)=self.get(&full,area,Accuracy::Exact,0) {(image,full)}
                else if let Some(image)=self.presented(&child,area) {(image,child)}
                else {(self.presented(&full,area)?,full)};
            let coverage=&self.pages[&selected].coverage;
            if !Coverage::covers(&coverage.exact,area) {accuracy=Accuracy::Display;}
            if self.pages[&selected].matches(self.expected.as_deref(),area,required_input_level)!=Some(true) {freshness=Freshness::Stale;}
            images.push((selected,image));
        }
        if freshness==Freshness::Stale && self.pages.get(key).is_some_and(|page|!page.coverage.written.is_empty()) {return None;}
        (!images.is_empty()).then_some((images,accuracy,freshness))
    }
}

#[cfg(test)]
fn fixture_store(packet:FramePacket<'_>)->Store {
    let mut store=Store::default();
    store.set_expected(Arc::new(SignatureFrame::from_scene(packet,Declared::default()).unwrap()));store
}
#[cfg(test)]
pub(super) fn accept_fixture_inputs(store:&mut Store,key:&Key,region:PixelRect,evaluation:Evaluation,images:Vec<(Key,PixelRect,Arc<Image>)>,input_level:u32) {
    let interest=Interest::new(key.clone(),evaluation,region,input_level);
    let accepted=store.accept_version(&interest).unwrap().unwrap();
    store.accept(&interest,images,accepted).unwrap();
}
#[cfg(test)]
pub(super) fn hold_fixture_inputs(store:&mut Store,key:&Key,region:PixelRect,evaluation:Evaluation,sources:crate::scene::sources::AcceptedInputs,input_level:u32) {
    if !store.pages.get(key).is_some_and(|page|page.inputs.iter().any(|held|held.region==region)) {
        let interest=Interest::new(key.clone(),evaluation,region,input_level);
        let accepted=store.accept_version(&interest).unwrap().unwrap();
        store.accept(&interest,Vec::new(),accepted).unwrap();
    }
    store.hold_inputs(key,region,evaluation,sources).unwrap();
}
#[cfg(test)]
pub(super) fn evict_composite_pages(cache:&Cache,areas:&[PixelRect]) {
    let mut store=cache.pages.lock().unwrap();
    let keys=store.retained_pages().into_iter().filter(|page|page.key.source.is_none()
        && areas.iter().any(|area|!page.key.plan().bounds.intersect(*area).is_empty())).map(|page|page.key).collect::<Vec<_>>();
    for key in keys {store.remove(&key);}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn indexed_key(coordinate:[u32;2],level:u32)->Key {
        Key {source:None,expression:Arc::new(graph::Expression::Color([1f32.to_bits();4])),extent:[65536;2],
            color:Default::default(),blend_space:layer_core::BlendSpace::Linear,level,coordinate,format:wgpu::TextureFormat::Rgba16Float}
    }
    #[test]
    fn page_index_queries_visit_only_the_requested_cells_at_each_kept_level() {
        let mut index=Index::default();
        for number in 0..50_000 {index.insert(&indexed_key([number%256,number/256],0));}
        let key=indexed_key([17,42],0);let area=key.plan().bounds;
        let coarse=indexed_key(key.coordinate.map(|value|value/8),3);index.insert(&coarse);
        let before=crate::test_support::PassCounts::snapshot();
        let found=index.overlapping(&key,area).cloned().collect::<std::collections::HashSet<_>>();
        assert!(found==[key.clone(),coarse.clone()].into());
        assert_eq!(crate::test_support::PassCounts::snapshot().delta(before).store_page_visits,2);
        for kept in [&key,&coarse] {index.remove(kept);}
        assert!(index.overlapping(&key,area).next().is_none());
    }
    #[test]
    fn seeded_page_index_changes_match_a_full_scan_after_each_operation() {
        let mut index=Index::default();let mut retained=std::collections::HashSet::new();let mut seed=0x139au64;
        for _ in 0..1024 {
            seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let level=(seed>>48) as u32%4;
            let coordinate=[(seed>>8) as u32%32,(seed>>24) as u32%32];let key=indexed_key(coordinate,level);
            if retained.remove(&key) {index.remove(&key);}else {retained.insert(key.clone());index.insert(&key);}
            let area=PixelRect::new(coordinate[0]*PAGE_SIZE,coordinate[1]*PAGE_SIZE,(coordinate[0]+3)*PAGE_SIZE,(coordinate[1]+2)*PAGE_SIZE);
            let expected=retained.iter().filter(|key|!key.plan().bounds.intersect(area).is_empty()).cloned().collect::<std::collections::HashSet<_>>();
            let actual=index.overlapping(&key,area).cloned().collect::<std::collections::HashSet<_>>();
            assert!(actual==expected);
        }
        for key in retained {index.remove(&key);}
        assert!(index.owners.is_empty());assert_eq!(index.allocated,0);
    }
    #[test]
    fn optional_subexpressions_cannot_fill_other_roots_and_yield_before_visible_pages() {
        let r=WgpuRasterizer::new_native_headless(Default::default()).unwrap();
        let extent=[2*PAGE_SIZE,PAGE_SIZE];
        let root=Key {source:None,expression:Arc::new(graph::Expression::Color([1f32.to_bits();4])),extent,
            color:Default::default(),blend_space:layer_core::BlendSpace::Linear,level:0,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
        let branch=Key {expression:Arc::new(graph::Expression::Color([0,1f32.to_bits(),0,1f32.to_bits()])),..root.clone()};
        let visible=Key {coordinate:[1,0],..root.clone()};
        let document=super::super::tests::document_at(extent);
        let bytes=u64::from(PAGE_SIZE).pow(2)*16;let mut store=fixture_store(crate::test_support::packet(document.scene(),extent));
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        for key in [&visible,&branch] {
            let image=store.claim(&r,key.clone(),2*bytes).unwrap();
            let write=store.prepare_write(key.clone(),image.plan.bounds,Accuracy::Display,Freshness::Current,key.level).unwrap();
            drop(encoder.color_pass("committed Store class",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
            store.commit(write,encoder.submission());
        }
        store.set_optional(&branch);encoder.submit(&r.queue);crate::test_support::complete(&r);
        assert!(store.coverage(&root,root.plan().bounds,0).written.is_empty());
        let (sources,complete)=store.projection(&root,root.plan().bounds,0);assert!(sources.is_empty());assert!(!complete);
        let needed=store.claim(&r,root.clone(),2*bytes).unwrap();
        assert!(store.pages.contains_key(&visible));assert!(!store.pages.contains_key(&branch));
        assert_eq!(store.bytes(),2*bytes);assert_eq!(needed.plan,root.plan());
    }
    #[test]
    fn a_native_frontier_fits_one_exact_working_window() {
        let extent=[9504,6336];
        let key=Key {source:None,expression:Arc::new(graph::Expression::Color([0;4])),extent,color:Default::default(),
            blend_space:layer_core::BlendSpace::Linear,level:2,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
        let mut missing=Vec::new();
        Store::default().refinement_frontier(&key,SOURCE_SLOTS,&mut missing,0);
        assert_eq!(missing.len(),SOURCE_SLOTS);
        let area=missing.into_iter().map(|key|page_rect(key.coordinate)).fold(PixelRect::EMPTY,PixelRect::union);
        let strip=exact_strip(extent);
        assert!(area.width()<=strip[0] && area.height()<=strip[1],"the admitted frontier {area:?} must fit its {strip:?} working window");
        assert!(u64::from(strip[0])*u64::from(strip[1])<=SOURCE_SLOTS as u64*u64::from(PAGE_SIZE).pow(2));
    }

    #[test]
    fn display_pages_and_their_reductions_require_exact_replacement() {
        let bounds=PixelRect::full([2*PAGE_SIZE,PAGE_SIZE]);
        fn written(coverage:&mut Coverage,area:PixelRect,accuracy:Accuracy) {
            for coordinate in coordinates(area,coverage.written.level) {
                coverage.written.entries.insert(coordinate);coverage.include(coordinate,Some(true),accuracy==Accuracy::Exact);
            }
        }
        let mut fine=Coverage::new(0);
        written(&mut fine,bounds,Accuracy::Display);
        assert!(fine.ready(bounds,Accuracy::Display));
        assert!(!fine.ready(bounds,Accuracy::Exact));
        let mut coarse=Coverage::new(0);
        written(&mut coarse,bounds,if fine.ready(bounds,Accuracy::Exact) {Accuracy::Exact}else{Accuracy::Display});
        assert!(!coarse.ready(bounds,Accuracy::Exact));
        let half=PixelRect::full([PAGE_SIZE;2]);
        written(&mut fine,half,Accuracy::Exact);
        assert!(fine.ready(half,Accuracy::Exact));
        assert!(!fine.ready(bounds,Accuracy::Exact));
        written(&mut fine,PixelRect::new(PAGE_SIZE,0,2*PAGE_SIZE,PAGE_SIZE),Accuracy::Exact);
        assert!(fine.ready(bounds,Accuracy::Exact));
        assert!(fine.matching.entries.remove(&[0,0]));
        assert!(Coverage::covers(&fine.written,bounds));
        assert!(!fine.ready(bounds,Accuracy::Display));
        assert!(!fine.ready(bounds,Accuracy::Exact));
    }
}

struct Projection {
    region: PixelRect,
    image: Arc<Image>,
    key: Key,
    versions:Vec<Recorded>,
}
pub(super) struct Write {
    pub key: Key,
    pub region: PixelRect,
    pub accuracy: Accuracy,
    accepted:Option<Accepted>,
}
impl RegionRecords {
    fn projection_ready(&self,r:&WgpuRasterizer,format:wgpu::TextureFormat)->bool {
        let pipeline=&r.scene_pipelines.scale.output(format).project;
        r.compilation.require([pipeline],startup::DOCUMENT)
    }
    fn project(&mut self,r:&mut WgpuRasterizer,encoder:&mut submission::CommandEncoder,image:&Image,output:&wgpu::TextureView,
        plan:display_mips::Plan,region:PixelRect,key:&Key,color:layer_core::color::DocumentColor,blend:layer_core::BlendSpace,
    )->Result<(wgpu::BindGroup,wgpu::BindGroup,u32,[u32;2],PixelRect),GpuRasterError> {
        let target_side=1<<plan.level;let source_side=(1<<image.plan.level) as f32;
        let [x,y,width,height]=paint_transform::texel_rect(region.window_local(plan.bounds),target_side);
        let matrix=key.color.space.linear_transform(color.space);
        let origin=[(region.min_x()-image.plan.bounds.min_x()) as f32/source_side,(region.min_y()-image.plan.bounds.min_y()) as f32/source_side];
        let mut bytes=[0;96];
        for (destination,value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip([x,y,width,height,origin[0].to_bits(),origin[1].to_bits(),(target_side as f32/source_side).to_bits(),working_color::space_id(key.color.space) as u32]
            .into_iter().chain(matrix.into_iter().flat_map(|row|row.map(|value|(value as f32).to_bits()).into_iter().chain([0])))
            .chain([u32::from(key.blend_space==layer_core::BlendSpace::Perceptual),u32::from(blend==layer_core::BlendSpace::Perceptual),0,0])) {
            *destination=value.to_le_bytes();
        }
        let offset=self.write(r,encoder,&bytes)?;
        let binding=Self::binding(r,&image.view,&r.empty_view,output);
        Ok((self.record_binding.clone(),binding,offset,[width,height],PixelRect::new(x,y,x+width,y+height)))
    }
    fn project_batch(&mut self,r:&mut WgpuRasterizer,encoder:&mut submission::CommandEncoder,copies:&[(&Projection,&Image)],
        color:layer_core::color::DocumentColor,blend:layer_core::BlendSpace,
    )->Result<(),GpuRasterError> {
        let mut records=Vec::with_capacity(copies.len());
        for (copy,output) in copies {
            records.push(if copy.image.plan.level==output.plan.level && copy.image.texture.format()==output.texture.format()
                && copy.key.color==color && copy.key.blend_space==blend {None}
                else {Some(self.project(r,encoder,&copy.image,&output.view,output.plan,copy.region,&copy.key,color,blend)?)});
        }
        let mut pass:Option<wgpu::ComputePass<'static>>=None;
        let mut written=Vec::<(wgpu::Texture,PixelRect)>::new();
        for ((copy,output),record) in copies.iter().zip(records) {
            if let Some((records,binding,offset,[width,height],area))=record {
                if written.iter().any(|(texture,previous)|*texture==output.texture && !previous.intersect(area).is_empty()) {drop(pass.take());written.clear();}
                let pass=pass.get_or_insert_with(||encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label:Some("project retained composite pages"),timestamp_writes:None}).forget_lifetime());
                pass.set_pipeline(&r.scene_pipelines.scale.output(output.texture.format()).project);
                pass.set_bind_group(0,&records,&[offset]);pass.set_bind_group(1,&binding,&[]);
                pass.dispatch_workgroups(width.div_ceil(8),height.div_ceil(8),1);written.push((output.texture.clone(),area));
            } else {
                drop(pass.take());written.clear();
                let side=1<<output.plan.level;
                let [sx,sy,width,height]=paint_transform::texel_rect(copy.region.window_local(copy.image.plan.bounds),side);
                let [x,y,_,_]=paint_transform::texel_rect(copy.region.window_local(output.plan.bounds),side);
                encoder.copy_texture_to_texture(wgpu::TexelCopyTextureInfo {origin:wgpu::Origin3d {x:sx,y:sy,z:0},..copy.image.texture.as_image_copy()},
                    wgpu::TexelCopyTextureInfo {origin:wgpu::Origin3d {x,y,z:0},..output.texture.as_image_copy()},wgpu::Extent3d {width,height,depth_or_array_layers:1});
            }
        }
        Ok(())
    }

}
impl Cache {
    pub(super) fn required_input_level(&self)->u32 {
        if self.evaluation==Evaluation::Native {return 0;}
        let frame=self.pages.lock().unwrap().expected().cloned();
        frame.map_or(self.plan.level,|frame|input_plan(self.plan,frame.snapshot().scene.view(),Some(self.graph.objects())).level)
    }
    pub(crate) fn waiting_for_name(&self)->bool {
        !self.coverage_snapshot().pending.is_empty() || self.presented_matches_expected().is_none()
            || self.overview.as_ref().is_some_and(|overview|overview.waiting_for_name())
    }
    pub(super) fn presented_matches_expected(&self)->Option<bool> {
        let store=self.pages.lock().unwrap();let Some(frame)=store.expected() else {return Some(false);};
        let Some(expression)=self.graph.content.as_ref().or(self.graph.root.as_ref()) else {return Some(false);};
        let snapshot=frame.snapshot();
        let input_level=if self.evaluation==Evaluation::Native {0}else {input_plan(self.plan,snapshot.scene.view(),Some(self.graph.objects())).level};
        let key=self.page_key(expression.clone(),self.plan.bounds);
        let held=self.pixels.committed_root().and_then(|root|root.authored.map(|authored|(authored,root.versions.as_slice())));
        publication_matches(frame,&key,self.plan.bounds,input_level,held)
    }
    pub(super) fn page_key(&self, expression: graph::Node, region: PixelRect) -> Key {
        let span=PAGE_SIZE<<self.plan.level;
        let expression=if self.graph.root.as_ref()==Some(&expression) {self.graph.content.clone().unwrap_or(expression)}else {expression};
        Key {source:None,expression,extent:self.plan.extent,color:self.color,blend_space:self.graph.blend_space(),level:self.plan.level,
            coordinate:[region.min_x()/span,region.min_y()/span],format:self.precision.format()}
    }
    fn fill_pages(&mut self,scene:&Scene,r:&mut WgpuRasterizer,encoder:&mut submission::CommandEncoder,commands:&mut RegionRecords,expression:&graph::Node,retained:[u64;2])->Result<(),GpuRasterError> {
        let required_input_level=self.required_input_level();
        let mut fills=Vec::new();
        for coordinate in coordinates(self.plan.bounds,self.plan.level) {
            let region=bounds(coordinate,self.plan.level,self.plan.extent);
            let key=self.page_key(expression.clone(),region);
            let mut store=self.pages.lock().unwrap();
            if store.presented(&key,region).is_some() {continue;}
            let (sources,complete)=store.projection(&key,region,required_input_level);
            if !complete {continue;}
            drop(store);
            let Some(image)=self.claim_page(scene,r,key.clone(),[retained[0]+commands.storage_bytes(),retained[1]],false)? else {continue;};
            let write=self.pages.lock().unwrap().prepare_write(key.clone(),image.plan.bounds,Accuracy::Display,Freshness::Stale,self.plan.level)?;
            fills.push((write,image,sources));
        }
        if fills.is_empty() {return Ok(());}
        if !commands.projection_ready(r,self.precision.format()) {return Err(GpuRasterError::NotReady);}
        commands.project_batch(r,encoder,&fills.iter().flat_map(|(_,output,sources)|sources.iter().map(move|source|(source,output.as_ref()))).collect::<Vec<_>>(),
            self.color,self.graph.blend_space())?;
        let mut store=self.pages.lock().unwrap();
        for (write,_image,sources) in fills {
            for source in sources {store.read(&source.key,&source.image,encoder.submission());}
            store.commit(write,encoder.submission());
        }
        Ok(())
    }
    pub(super) fn project_above(&mut self,scene:&Scene,r:&mut WgpuRasterizer,packet:FramePacket<'_>,encoder:&mut submission::CommandEncoder,
        commands:&mut RegionRecords,retained:[u64;2],
    )->Result<(),GpuRasterError> {
        let required_input_level=self.required_input_level();
        let Some(above)=self.graph.motion.as_ref().map(|halves|halves[0].clone()) else {return Ok(());};
        if graph::Expression::is_clear(&above) {return Ok(());}
        let Some(region)=r.vector_motion_region(packet.document_extent).map(|region|region.intersect(self.plan.bounds)).filter(|region|!region.is_empty()) else {return Ok(());};
        let plan=display_mips::Plan::window(self.plan.extent,self.plan.level,region);
        let mut copies=Vec::new();let mut revision=0;
        {
            let mut store=self.pages.lock().unwrap();
            for coordinate in coordinates(region,self.plan.level) {
                let mut key=self.page_key(above.clone(),bounds(coordinate,self.plan.level,self.plan.extent));
                let area=key.plan().bounds.intersect(region);
                let image=if let Some(image)=store.get(&key,area,Accuracy::Display,required_input_level) {image}else {
                    key.format=wgpu::TextureFormat::Rgba32Float;
                    let Some(image)=store.get(&key,area,Accuracy::Exact,0) else {return Ok(());};image
                };
                revision=revision.max(store.pages[&key].written_at);
                let versions=store.pages[&key].versions.values().filter_map(|recorded|recorded.clipped(area)).collect();
                copies.push(Projection {key,image,region:area,versions});
            }
        }
        if self.above_revision.as_ref()==Some(&(above.clone(),revision))
            && self.above.committed_root().is_some_and(|root|root.image.plan==plan) {return Ok(());}
        if copies.iter().any(|copy|copy.image.texture.format()!=self.precision.format()) && !commands.projection_ready(r,self.precision.format()) {return Ok(());}
        if self.above.root().is_none_or(|image|image.plan!=plan || image.texture.format()!=self.precision.format()) {
            let bytes=plan.size.map(u64::from).into_iter().product::<u64>()*u64::from(self.precision.format().block_copy_size(None).unwrap());
            let Some(reservation)=self.reserve_window(r,scene,retained,bytes,self.above.storage_bytes()) else {return Ok(());};
            let old=self.above.take();let bytes=old.iter().map(Image::bytes).sum();
            r.retired_display_bytes.fetch_add(bytes,std::sync::atomic::Ordering::AcqRel);
            let charge=crate::DisplayRetirement {bytes,charge:r.retired_display_bytes.clone(),
                #[cfg(not(target_arch="wasm32"))] _scratch:(Vec::new(),old)};
            #[cfg(target_arch="wasm32")] drop(old);
            encoder.on_submitted_work_done(move||drop(charge));
            self.above.ensure_root(r,plan,self.precision.format(),"moving paths foreground projection");
            self.commit_bytes(r,scene,retained,reservation)?;
        }
        let output=self.above.root().unwrap();
        commands.project_batch(r,encoder,&copies.iter().map(|copy|(copy,output)).collect::<Vec<_>>(),self.color,self.graph.blend_space())?;
        let mut store=self.pages.lock().unwrap();
        for copy in &copies {store.read(&copy.key,&copy.image,encoder.submission());}
        let versions=copies.iter().flat_map(|copy|copy.versions.iter().cloned()).collect();
        let authored=store.expected().map(|frame|frame.authored_revision(&above)).transpose()?;
        self.above.commit_root(encoder.submission(),copies.into_iter().map(|copy|copy.image).collect(),versions,authored);
        self.above_revision=Some((above,revision));
        Ok(())
    }
    pub(super) fn project_pages(&mut self,scene:&Scene,r:&mut WgpuRasterizer,encoder:&mut submission::CommandEncoder,commands:&mut RegionRecords,retained:[u64;2],served:PixelRect)->Result<PixelRect,GpuRasterError> {
        let Some(expression)=self.graph.root.clone() else {return Ok(PixelRect::EMPTY);};
        self.fill_pages(scene,r,encoder,commands,&expression,retained)?;
        let required_input_level=self.required_input_level();
        let frame=self.pages.lock().unwrap().expected().cloned().ok_or(GpuRasterError::Invariant("signature frame is missing or inconsistent"))?;
        let authored=frame.authored_revision(&self.page_key(expression.clone(),self.plan.bounds).expression)?;
        let mut copies=Vec::new();let mut written=vec![0u32;(self.geometry.size() as usize-32)/4];
        for (index,coordinate) in coordinates(self.plan.bounds,self.plan.level).enumerate() {
            let region=bounds(coordinate,self.plan.level,self.plan.extent).intersect(self.plan.bounds);
            let key=self.page_key(expression.clone(),region);
            let (selected,_complete)=self.pages.lock().unwrap().projection(&key,region,required_input_level);
            let selected=selected.into_iter().filter(|copy|authored_matches(&copy.versions,&frame,authored,copy.region)).collect::<Vec<_>>();
            let missing=selected.iter().fold(vec![region],|missing,copy|missing.into_iter().flat_map(|part|part.subtract(copy.region)).filter(|part|!part.is_empty()).collect());
            if missing.is_empty() {written[index/32]|=1<<(index%32);}
            copies.extend(selected);
        }
        if copies.is_empty() {return Ok(PixelRect::EMPTY);}
        let versions=copies.iter().flat_map(|copy|copy.versions.iter().cloned()).collect::<Vec<_>>();
        let complete=visible_projection_complete(self.plan,served,&written);
        let same_authored=copies.iter().all(|copy|authored_matches(&copy.versions,&frame,authored,copy.region));
        if self.pixels.committed_root().is_some_and(|held|held.authored!=Some(authored)) && !(complete && same_authored) {return Ok(PixelRect::EMPTY);}
        let conversion=copies.iter().any(|copy|copy.image.texture.format()!=self.precision.format()
            || copy.key.level!=self.plan.level || copy.key.color!=self.color || copy.key.blend_space!=self.graph.blend_space());
        if conversion && !commands.projection_ready(r,self.precision.format()) {return Err(GpuRasterError::NotReady);}
        let allocation=(self.pixels.root().is_none_or(|image|image.texture.format()!=self.precision.format())).then(|| {
            let bytes=self.plan.size.map(u64::from).into_iter().product::<u64>()*u64::from(self.precision.format().block_copy_size(None).unwrap());
            self.reserve_window(r,scene,[retained[0]+commands.storage_bytes(),retained[1]],bytes,self.pixels.storage_bytes()).ok_or(GpuRasterError::NotReady)
        }).transpose()?;
        if self.pixels.root().is_some_and(|image|image.texture.format()!=self.precision.format()) {
            let retired=self.pixels.take();
            let bytes=retired.iter().map(Image::bytes).sum();
            r.retired_display_bytes.fetch_add(bytes,std::sync::atomic::Ordering::AcqRel);
            let charge=crate::DisplayRetirement {bytes,charge:r.retired_display_bytes.clone(),
                #[cfg(not(target_arch="wasm32"))] _scratch:(Vec::new(),retired)};
            encoder.on_submitted_work_done(move||drop(charge));
        }
        self.pixels.ensure_root(r,self.plan,self.precision.format(),"composition window");
        if let Some(reservation)=allocation {self.commit_bytes(r,scene,[retained[0]+commands.storage_bytes(),retained[1]],reservation)?;}
        let leases=copies.iter().map(|copy|copy.image.clone()).collect();
        commands.project_batch(r,encoder,&copies.iter().map(|copy|(copy,self.pixels.root().unwrap())).collect::<Vec<_>>(),self.color,self.graph.blend_space())?;
        let mut changed=PixelRect::EMPTY;
        for Projection {region,image,key:source_key,..} in copies {
            self.pages.lock().unwrap().read(&source_key,&image,encoder.submission());
            changed=changed.union(region);
        }
        if written.iter().any(|word|*word!=0) {
            let count=written.iter().map(|word|word.count_ones()).sum::<u32>();
            let level=self.plan.level;let window=self.plan.bounds;
            let data=[count,window.min_x()>>level,window.min_y()>>level,window.max_x().div_ceil(1<<level),window.max_y().div_ceil(1<<level)];
            #[cfg(test)]
            let observed_written=written.clone();
            r.uploads.write_at(encoder,&self.geometry,12,&data.into_iter().chain(written).flat_map(u32::to_le_bytes).collect::<Vec<_>>())?;
            self.pixels.commit_root(encoder.submission(),leases,versions,same_authored.then_some(authored));
            #[cfg(test)]
            {
                let root=self.pixels.committed_root().unwrap();
                crate::test_support::record_published_coverage(crate::test_support::coverage_hash(&self.geometry),crate::test_support::PublishedCoverage {
                    plan:root.image.plan,receipt:root.submission.observation_id(),pages:root.pages.iter().map(|page|Arc::as_ptr(page) as usize).collect(),written:observed_written,
                });
            }
            self.write_projection_scales(r,encoder)?;
        }
        Ok(changed)
    }
}

impl Cache {
    pub(in crate::scene) fn retained_ready(&self)->bool {self.coverage_snapshot().ready(self.plan.bounds,Accuracy::Display)}
}

#[cfg(test)]
pub(super) mod query_tests {
    use super::*;
    use super::super::tests::paint_mut;
    use crate::test_support::packet;

    #[test]
    fn producer_interests_keep_owner_level_quality_and_region_together() {
        let key=Key {source:None,expression:Arc::new(graph::Expression::Color([0;4])),extent:[2048;2],color:Default::default(),
            blend_space:layer_core::BlendSpace::Linear,level:1,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
        let region=key.plan().bounds;
        let accepted=Interest::new(key.clone(),Evaluation::Display,region,0);
        let wanted=Interest::new(key.clone(),Evaluation::Display,PixelRect::full(key.extent),0);
        assert!(wanted.contains(&accepted));
        assert!(!Interest::new(key.clone(),Evaluation::Display,PixelRect::full(key.extent),1).contains(&accepted));
        assert!(!Interest::new(key.clone(),Evaluation::Native,PixelRect::full(key.extent),0).contains(&accepted));
        assert!(!Interest::new(Key {level:2,..key.clone()},Evaluation::Display,PixelRect::full(key.extent),0).contains(&accepted));
        assert!(!Interest::new(Key {format:wgpu::TextureFormat::Rgba16Float,..key.clone()},Evaluation::Display,PixelRect::full(key.extent),0).contains(&accepted));
        assert!(!Interest::new(key.clone(),Evaluation::Display,PixelRect::new(512,0,1024,512),0).contains(&accepted));
        let source=Key {source:Some(SourceTarget::Paint(layer_core::PaintHandle::from_index(0))),..key};
        let accepted=Interest::new(source.clone(),Evaluation::Display,region,1);
        assert!(Interest::new(source.clone(),Evaluation::Native,PixelRect::full(source.extent),1).contains(&accepted));
        assert!(!Interest::new(source,Evaluation::Display,PixelRect::new(0,0,256,256),1).contains(&accepted));
    }

    pub(crate) fn assert_display_grant_decrease_reconciles_retention(r:&mut WgpuRasterizer,document:&layer_core::Document) {
        use crate::submission::ColorPass;
        let extent=document.composition().size;
        let mut frame=display_frame(document,0);frame.view.width_px=256;frame.view.height_px=256;
        r.memory_demand(frame);r.prepare_moving_projection(frame).unwrap();
        let mut scene=Scene::new(&r).unwrap();scene.scale_sources.prepare(&r,frame,&[]);
        let mut cache=Cache::new(&r,Request {plan:display_mips::Plan::at(extent,0),evaluation:Evaluation::Display},document.scene().order().len());
        crate::scene::scale::tests::prepare_fixture_frame(&r,frame,&scene);
        cache.prepare_graph(&r,frame,&scene).unwrap();
        let root=cache.graph.root.clone().unwrap();
        let near=cache.page_key(root.clone(),page_rect([0,0]));
        let far=cache.page_key(root,page_rect([31,31]));
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        for key in [&near,&far] {
            let image=cache.pages.lock().unwrap().claim(&r,key.clone(),u64::MAX).unwrap();
            let write=cache.pages.lock().unwrap().prepare_write(key.clone(),key.plan().bounds,Accuracy::Display,Freshness::Current,key.level).unwrap();
            {let _pass=encoder.color_pass("accepted Display grant fixture",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE));}
            cache.pages.lock().unwrap().commit(write,encoder.submission());
            accept_fixture_inputs(&mut cache.pages.lock().unwrap(),key,key.plan().bounds,Evaluation::Display,vec![(key.clone(),key.plan().bounds,image)],key.level);
        }
        encoder.submit(&r.queue);crate::test_support::complete(&r);
        let far_pin=cache.pages.lock().unwrap().pages[&far].image.clone();
        let store_bytes=cache.pages.lock().unwrap().bytes();let mip_bytes=r.moving_images.storage_bytes();
        assert!(mip_bytes>=128<<20,"real optional mip retention exceeds the working floor");
        let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,[0;2]));
        r.memory.sync_report(layer_core::MemoryOwnerKind::Scene,report);
        let budget=r.factory.memory_budget().clone();budget.flush_reports();budget.set_ram_override(Some(384<<20));budget.flush_reports();
        let pressure=budget.admit_import(budget.import_headroom_bytes()).unwrap();
        assert!(r.memory.display_owner().grant().gpu_bytes>=report.pinned_gpu_bytes);
        assert!(r.memory.display_owner().grant().gpu_bytes<report.gpu_bytes,"the actual Core grant must fall below allocated retention");
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        r.memory.invalidate_limits();r.apply_memory_grants(Some(&mut scene),Some(&mut cache),[0;2],&mut encoder);
        encoder.submit(&r.queue);crate::test_support::complete(&r);
        let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,[0;2]));
        assert!(r.moving_images.storage_bytes()<mip_bytes,"the existing optional mip victim path must reconcile at the owner boundary");
        assert!(report.gpu_bytes<=r.memory.display_owner().grant().gpu_bytes.max(report.pinned_gpu_bytes),"Display physical must reconcile within the frame completion");
        assert!(r.memory.display_owner().grant().gpu_bytes>=report.pinned_gpu_bytes);
        let pages=cache.pages.lock().unwrap();assert_eq!(pages.bytes(),store_bytes,"accepted Store pages are guards, not optional victims");
        assert!(pages.accepted(&near,near.plan().bounds));
        let held=pages.accepted_coordinates(&near).collect::<Vec<_>>();
        let visible=display_mips::view_bounds(frame.view,extent,0).unwrap();
        assert_eq!(nearest_pages(visible,frame.view,1,held,|_|true),vec![[0,0]]);
        assert!(Arc::strong_count(&far_pin)>1);
        drop(pages);drop(far_pin);drop(pressure);
    }

    fn display_frame(document:&layer_core::Document,level:u32)->FramePacket<'_> {
        let extent=document.composition().size;let side=1u32<<level;
        let mut frame=packet(document.scene(),extent);
        frame.blend_space=document.composition().blend;frame.composite_all=false;
        frame.view.width_px=extent[0].div_ceil(side);frame.view.height_px=extent[1].div_ceil(side);
        frame.view.document_to_surface=[1./side as f32,0.,0.,1./side as f32,0.,0.];frame
    }

    fn display(document:&layer_core::Document,source:&Arc<layer_core::color::source::SourceImage>,level:u32)->WgpuRasterizer {
        let extent=document.composition().size;
        let mut renderer=crate::memory::memory_tests::legacy_scene_renderer(document.composition().color);
        renderer.ensure_document_metadata(extent,document.scene()).unwrap();
        let frame=display_frame(document,level);
        crate::memory::memory_tests::legacy_scene_frame(&mut renderer,frame);
        let [available,uploads]=renderer.source_tiles.borrow().admitted_bytes();
        let non_slot=renderer.source_tiles.borrow().non_slot_bytes();
        let mips=renderer.source_tiles.borrow().mip_budget();
        let inputs=source.tiles.len() as u64*u64::from(PAGE_SIZE).pow(2)*16;
        renderer.source_tiles.get_mut().admit_limits(available.max(inputs+SOURCE_WORKING_BYTES)+non_slot,uploads,mips);
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        for coordinate in page_coordinates(PixelRect::full(extent)) {
            let pixels=crate::prepare_image_tile(source,coordinate,renderer.device.working_space(),&renderer.device.source_samples,None).unwrap();
            Scene::decode_prepared_for(&mut renderer,&mut encoder,source,coordinate,pixels,
                crate::scene::sources::SourceUse::Exact).unwrap();
        }
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);crate::test_support::complete(&renderer);
        let mut scene=renderer.scene.take().unwrap_or_else(||Scene::new(&renderer).unwrap());
        let mut cache=Cache::new(&renderer,Request {plan:display_mips::Plan::at(extent,level),evaluation:Evaluation::Display},document.scene().order().len());
        let mut commands=RegionRecords::new(&renderer);
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        scene.begin_frame();commands.begin();scene.scale_sources.prepare(&renderer,frame,&[]);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        cache.render_graph(&mut scene,&mut renderer,frame,PixelRect::full(extent),
            &mut Encoding {encoder:&mut encoder,commands:&mut commands, retained:[0;2]},Destination::View,None,0).unwrap();
        scene.encode_jobs(&mut renderer,&mut encoder).unwrap();
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);renderer.wait_idle().unwrap();
        renderer.scene=Some(scene);renderer.scale_display=Some(cache);renderer
    }

    fn refine(renderer:&mut WgpuRasterizer,frame:FramePacket<'_>) {
        let mut scene=renderer.scene.take().unwrap();
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        scene.refine_display(renderer,frame,&mut encoder).unwrap();
        renderer.scene=Some(scene);renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);renderer.wait_idle().unwrap();
    }

    #[test]
    fn preparing_one_root_refinement_preserves_other_level_ownership() {
        use layer_core::authored::*;
        let extent=[1024,512];
        let mut artwork=Artwork::new(extent).unwrap();
        let source=layer_core::color::source::rgba8_source([512;2],|_,_|[31,71,113,255]);
        let object=artwork.objects.insert(layer_core::PortableId::random(),ImageObject::new(Image::new(source))).unwrap();
        let owner=artwork.occurrences.insert(layer_core::PortableId::random(),Occurrence::new(OccurrenceContent::Objects(object),"Image")).unwrap();
        let stack=artwork.compositions.get(artwork.root).unwrap().result;
        artwork.stacks.get_mut(stack).unwrap().entries=vec![owner];
        let document=layer_core::Document::from_artwork(artwork).unwrap();
        let mut renderer=crate::memory::memory_tests::legacy_scene_renderer(document.composition().color);
        renderer.ensure_document_metadata(extent,document.scene()).unwrap();
        let frame=display_frame(&document,0);
        crate::memory::memory_tests::legacy_scene_frame(&mut renderer,frame);
        let mut scene=Scene::new(&renderer).unwrap();scene.scale_sources.prepare(&renderer,frame,&[]);
        let mut cache=Cache::new(&renderer,Request {plan:display_mips::Plan::at(extent,1),evaluation:Evaluation::Display},1);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.pages=scene.scale_pages.clone();
        cache.prepare_root(&renderer,frame,&scene.scale_sources).unwrap();
        let key=cache.page_key(cache.graph.root.clone().unwrap(),bounds([0;2],1,extent));
        let image=cache.pages.lock().unwrap().claim(&renderer,key.clone(),cache_bytes(&renderer)).unwrap();
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        let write=cache.pages.lock().unwrap().prepare_write(key.clone(),image.plan.bounds,Accuracy::Display,Freshness::Current,key.level).unwrap();
        drop(encoder.color_pass("committed other-level page",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
        cache.pages.lock().unwrap().commit(write,encoder.submission());
        let mut pending=key.clone();pending.level=2;
        drop(cache.pages.lock().unwrap().claim(&renderer,pending.clone(),cache_bytes(&renderer)).unwrap());
        let input=Arc::new(());
        hold_fixture_inputs(&mut cache.pages.lock().unwrap(),&pending,pending.plan().bounds,Evaluation::Display,vec![input.clone().into()].into(),pending.level);
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);crate::test_support::complete(&renderer);
        assert!(cache.pages.lock().unwrap().get(&key,key.plan().bounds,Accuracy::Display,key.level).is_some());
        scene.scale_sources.object_refinements.insert((0,Some(owner)),PixelRect::new(0,0,256,256).into());
        let mut fine=Cache::new(&renderer,Request {plan:display_mips::Plan::at(extent,0),evaluation:Evaluation::Display},1);
        fine.pages=cache.pages.clone();
        fine.prepare_root(&renderer,frame,&scene.scale_sources).unwrap();
        assert!(cache.pages.lock().unwrap().get(&key,key.plan().bounds,Accuracy::Display,key.level).is_some(),"a refinement at level zero cannot stale another level's committed pixels");
        assert_eq!(Arc::strong_count(&input),2,"a refinement cannot cancel another level's accepted inputs");
        assert!(crate::test_support::float_pixels(&renderer,&image.texture).iter().all(|pixel|*pixel==[1.;4]));
    }

    #[test]
    fn an_unwritten_overview_uses_the_committed_window() {
        let extent=[4096,2048];
        let mut document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let source=layer_core::color::source::rgba8_source(extent,|_,_|[40,90,170,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=display(&document,&source,0);
        let mut cache=renderer.scale_display.take().unwrap();
        let overview=cache.overview.as_mut().unwrap();
        overview.pixels.ensure(&renderer,overview.plan,overview.precision.format());
        let key=overview.page_key(overview.graph.root.clone().unwrap(),overview.plan.bounds);
        let claimed=overview.pages.lock().unwrap().claim(&renderer,key.clone(),u64::MAX).unwrap();
        assert!(overview.pages.lock().unwrap().initialized(&key,&claimed.view,key.plan().bounds).unwrap().is_empty());
        assert!(claimed.texture.width()>0);
        assert!(overview.pixels.root().is_some() && overview.pixels.next().is_some());
        assert!(overview.presentation().is_none(),"allocation and validity bookkeeping cannot mint a committed page handle");
        renderer.scale_display=Some(cache);
        let cache=renderer.scale_display.as_ref().unwrap();
        assert!(cache.overview.as_ref().is_some_and(|overview|overview.presentable().is_none()));
        assert!(cache.presentable().is_some());
        let views=renderer.presentation_views().unwrap();
        assert_eq!(views[0],cache.view());
        assert_eq!(views[1],cache.presentation().unwrap().views()[2]);
        assert!(views.iter().all(|view|view.texture().width()>0 && view.texture().height()>0));
    }

    #[test]
    fn overview_inputs_survive_camera_level_and_evaluation_changes() {
        let extent=[8192,4096];
        let document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let renderer=crate::memory::memory_tests::legacy_scene_renderer(document.composition().color);
        let mut scene=Scene::new(&renderer).unwrap();
        let frame=display_frame(&document,0);
        scene.scale_sources.prepare(&renderer,frame,&[]);
        let mut cache=Cache::new(&renderer,Request {plan:display_mips::Plan::at(extent,0),evaluation:Evaluation::Display},document.scene().order().len());
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let overview=cache.overview.as_ref().unwrap();
        let key=overview.page_key(overview.graph.root.clone().unwrap(),overview.plan.bounds);
        drop(overview.pages.lock().unwrap().claim(&renderer,key.clone(),cache_bytes(&renderer)).unwrap());
        let owner=Arc::new(());
        hold_fixture_inputs(&mut overview.pages.lock().unwrap(),&key,key.plan().bounds,overview.evaluation,vec![owner.clone().into()].into(),key.level);
        for (level,evaluation) in [(1,Evaluation::Native),(3,Evaluation::Tiled(2)),(0,Evaluation::Display),(2,Evaluation::Native)] {
            let frame=FramePacket {composite_all:false,..display_frame(&document,level)};
            scene.scale_sources.prepare(&renderer,frame,&[]);
            (cache,_)=Cache::select(Some(cache),&renderer,frame,Request {plan:display_mips::Plan::at(extent,level),evaluation},true);
            crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
            cache.prepare_graph(&renderer,frame,&scene).unwrap();
            let global=if cache.plan==overview_plan(cache.plan) {&cache}else {cache.overview.as_deref().unwrap()};
            assert!(global.page_key(global.graph.root.clone().unwrap(),global.plan.bounds)==key);
            assert_eq!(Arc::strong_count(&owner),2,"camera/evaluation changes cannot cancel accepted overview inputs");
        }
    }

    pub(crate) fn assert_regional_object_overview(renderer:&mut WgpuRasterizer,document:&layer_core::Document,
        owner:OccurrenceHandle,id:layer_core::PortableId) {
        let extent=document.composition().size;
        let mut frame=display_frame(document,4);frame.view.width_px=256;frame.view.height_px=256;
        let mut scene=Scene::new(&renderer).unwrap();scene.scale_sources.prepare(&renderer,frame,&[]);
        let plan=overview_plan(display_mips::Plan::at(extent,0));assert_eq!(plan.level,4);
        let mut cache=Cache::new(&renderer,Request {plan,evaluation:Evaluation::Display},document.scene().order().len());
        let mut commands=RegionRecords::new(&renderer);scene.begin_frame();
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let content=scene.object_spatial.content(frame.scene,owner).unwrap();
        let leaf=scene.collection_job(&renderer,frame,owner,content,
            crate::scene::objects::ObjectWindow {origin:[0.;2],side:16.,size:[256;2]});
        assert_eq!(leaf.child(0,leaf.window,renderer.empty_view.clone()).unwrap().key.image,id);
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        cache.render_graph(&mut scene,renderer,frame,plan.bounds,&mut Encoding {encoder:&mut encoder,commands:&mut commands, retained:[0;2]},Destination::Preview,None,0).unwrap();
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);renderer.wait_idle().unwrap();
        assert!(cache.coverage_snapshot().ready(bounds([0;2],plan.level,extent),Accuracy::Display),
            "the overview unit must commit on the frame its regional mip inputs and halo are ready");
        assert!(!cache.coverage_snapshot().ready(plan.bounds,Accuracy::Display));
        let content=scene.object_spatial.content(frame.scene,owner).unwrap();
        let canonical=scene.collection_job(&renderer,frame,owner,content,
            crate::scene::objects::ObjectWindow {origin:[0.;2],side:16.,size:[256;2]});
        assert!(scene.object_results.cover(&canonical,None).0.is_empty(),"no canonical result supplies the committed preview");
    }

    #[test]
    fn an_overview_page_commits_while_other_source_chunks_wait() {
        let extent=[4096;2];
        let mut document=super::super::tests::document_at(extent);
        let source=layer_core::color::source::rgba8_source(extent,|x,y|[(x/PAGE_SIZE) as u8,(y/PAGE_SIZE) as u8,119,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=crate::memory::memory_tests::legacy_scene_renderer(document.composition().color);
        renderer.ensure_document_metadata(extent,document.scene()).unwrap();
        let mut frame=display_frame(&document,1);frame.view.width_px=256;frame.view.height_px=256;
        crate::memory::memory_tests::legacy_scene_frame(&mut renderer,frame);
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        for coordinate in [[0,0],[1,0],[0,1],[1,1]] {
            let pixels=crate::prepare_image_tile(&source,coordinate,renderer.device.working_space(),&renderer.device.source_samples,None).unwrap();
            Scene::decode_prepared_for(&mut renderer,&mut encoder,&source,coordinate,pixels,crate::scene::sources::SourceUse::Exact).unwrap();
        }
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);renderer.wait_idle().unwrap();
        let _store=crate::test_support::GatedStore::new(&layer_core::raster_storage::RetainedTiles::from_sources([source.as_ref()]),false);
        let mut scene=Scene::new(&renderer).unwrap();scene.scale_sources.prepare(&renderer,frame,&[]);
        let plan=overview_plan(display_mips::Plan::at(extent,0));
        let page_count=coordinates(plan.bounds,plan.level).count();
        assert!((16..=64).contains(&page_count));
        let mut cache=Cache::new(&renderer,Request {plan,evaluation:Evaluation::Display},document.scene().order().len());
        let mut commands=RegionRecords::new(&renderer);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        scene.begin_frame();cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        cache.render_graph(&mut scene,&mut renderer,frame,plan.bounds,&mut Encoding {encoder:&mut encoder,commands:&mut commands, retained:[0;2]},Destination::View,None,0).unwrap();
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);renderer.wait_idle().unwrap();
        let coverage=cache.coverage_snapshot();
        assert!(coverage.ready(bounds([0;2],plan.level,extent),Accuracy::Display),"one complete page must publish without the rest of its level");
        assert!(!coverage.ready(plan.bounds,Accuracy::Display),"the other source chunks are still gated");
        renderer.scene=Some(scene);renderer.scale_display=Some(cache);
        frame.view.width_px=512;frame.view.height_px=512;
        let output=create_color_target(&renderer.device,[512;2],"partial overview");
        let mut presenter=crate::test_support::float_presenter(&renderer);
        presenter.present(&mut renderer,&output.1,frame.view,[0.18,0.18,0.18,1.]).unwrap();
        let surround=crate::test_support::float_pixels(&renderer,&output.0);
        renderer.presentation.showing=true;
        let validation=renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
        presenter.present(&mut renderer,&output.1,frame.view,[0.18,0.18,0.18,1.]).unwrap();
        let picture=crate::test_support::float_pixels(&renderer,&output.0);
        assert_ne!(picture[128*512+128],surround[128*512+128],"the ready page contains real source pixels");
        assert_eq!(picture[384*512+384],surround[384*512+384],"unwritten pages stay surround, without sampling their allocation");
        assert!(pollster::block_on(validation.pop()).is_none());
    }

    #[test]
    fn cropped_content_presents_while_a_global_fallback_prepares() {
        use crate::submission::ColorPass;
        let extent=[4096,2048];
        let document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let mut renderer=crate::memory::memory_tests::legacy_scene_renderer(document.composition().color);
        let mut scene=Scene::new(&renderer).unwrap();
        let frame=display_frame(&document,0);
        scene.scale_sources.prepare(&renderer,frame,&[]);
        let mut cache=Cache::new(&renderer,Request {plan:display_mips::Plan::window(extent,0,PixelRect::full([PAGE_SIZE;2])),
            evaluation:Evaluation::Display},document.scene().order().len());
        let mut commands=RegionRecords::new(&renderer);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let key=cache.page_key(cache.graph.root.clone().unwrap(),cache.plan.bounds);
        let image=cache.pages.lock().unwrap().claim(&renderer,key.clone(),cache_bytes(&renderer)).unwrap();
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        let write=cache.pages.lock().unwrap().prepare_write(key.clone(),image.plan.bounds,Accuracy::Display,Freshness::Current,key.level).unwrap();
        drop(encoder.color_pass("first cropped page",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
        cache.pages.lock().unwrap().commit(write,encoder.submission());
        cache.project_pages(&scene,&mut renderer,&mut encoder,&mut commands,[0;2],cache.plan.bounds).unwrap();
        assert!(cache.presentation().is_some());
        assert!(cache.presentable().is_some(),"committed content can present before the rest of the overview");
        let overview=cache.overview.as_mut().unwrap();
        for coordinate in coordinates(overview.plan.bounds,overview.plan.level) {
            let region=bounds(coordinate,overview.plan.level,extent);
            let key=overview.page_key(overview.graph.root.clone().unwrap(),region);
            let image=overview.pages.lock().unwrap().claim(&renderer,key.clone(),cache_bytes(&renderer)).unwrap();
            let write=overview.pages.lock().unwrap().prepare_write(key.clone(),image.plan.bounds,Accuracy::Display,Freshness::Current,key.level).unwrap();
            drop(encoder.color_pass("global fallback page",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
            overview.pages.lock().unwrap().commit(write,encoder.submission());
        }
        overview.project_pages(&scene,&mut renderer,&mut encoder,&mut commands,[0;2],overview.plan.bounds).unwrap();
        assert!(cache.presentable().is_some());
        let plan=display_mips::Plan::window(extent,0,PixelRect::new(768,256,1024,512));
        let (mut next,_)=Cache::select(Some(cache),&renderer,frame,Request {plan,evaluation:Evaluation::Display},true);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        next.prepare_graph(&renderer,frame,&scene).unwrap();
        next.project_pages(&scene,&mut renderer,&mut encoder,&mut commands,[0;2],next.plan.bounds).unwrap();
        assert!(next.presentable().is_some());
        assert!(!next.retained_ready() && !next.output_complete());
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);crate::test_support::complete(&renderer);
        assert!(crate::test_support::float_pixels(&renderer,next.texture()).iter().all(|pixel|*pixel==[1.;4]));
    }

    #[test]
    fn exact_children_rebuild_an_evicted_parent_without_source_work() {
        let extent=[4096,512];
        let mut document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let source=layer_core::color::source::rgba8_source(extent,|_,_|[40,90,170,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=display(&document,&source,1);
        let frame=display_frame(&document,1);
        for _ in 0..64 {
            if renderer.scale_display.as_ref().unwrap().output_complete() {break;}
            refine(&mut renderer,frame);
        }
        assert!(renderer.scale_display.as_ref().unwrap().output_complete());
        let mut scene=renderer.scene.take().unwrap();
        let frame=FramePacket {composite_all:false,..display_frame(&document,2)};
        scene.scale_sources.prepare(&renderer,frame,&[]);
        let (mut cache,_)=Cache::select(renderer.scale_display.take(),&renderer,frame,
            Request {plan:display_mips::Plan::at(extent,2),evaluation:Evaluation::Display},true);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let mut key=cache.page_key(cache.graph.root.clone().unwrap(),cache.plan.bounds);
        key.format=wgpu::TextureFormat::Rgba32Float;
        {
            let mut store=cache.pages.lock().unwrap();
            if store.pages.contains_key(&key) {let page=store.remove(&key);assert_eq!(Arc::strong_count(&page.image),1);page.image.texture.destroy();}
            assert!(key.finer().all(|child|store.get(&child,child.plan().bounds,Accuracy::Exact,0).is_some()));
        }
        let before=[renderer.metrics.source_upload_submissions,renderer.source_tiles.borrow().misses,renderer.metrics.composited_pixels];
        renderer.scale_display=Some(cache);renderer.scene=Some(scene);
        refine(&mut renderer,FramePacket {composite_all:false,..display_frame(&document,2)});
        assert!(renderer.scale_display.as_ref().unwrap().pages.lock().unwrap().get(&key,key.plan().bounds,Accuracy::Exact,0).is_some(),
            "ready exact children must rebuild a missing ancestor even when no native page is missing");
        assert_eq!([renderer.metrics.source_upload_submissions,renderer.source_tiles.borrow().misses,renderer.metrics.composited_pixels],before);
    }

    #[test]
    fn kept_levels_fill_window_pages_without_satisfying_evaluation() {
        let extent=[1024,512];
        let mut document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let source=layer_core::color::source::rgba8_source(extent,|_,_|[40,90,170,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=display(&document,&source,2);
        let uploads=renderer.metrics.source_upload_submissions;
        let decoded=renderer.source_tiles.borrow().misses;
        let composed=renderer.metrics.composited_pixels;
        let frame=display_frame(&document,0);
        let scene=renderer.scene.take().unwrap();
        let (mut cache,_)=Cache::select(renderer.scale_display.take(),&renderer,frame,
            Request {plan:display_mips::Plan::at(extent,0),evaluation:Evaluation::Display},true);
        let mut commands=RegionRecords::new(&renderer);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        cache.project_pages(&scene,&mut renderer,&mut encoder,&mut commands,[0;2],cache.plan.bounds).unwrap();
        for coordinate in coordinates(cache.plan.bounds,cache.plan.level) {
            let region=bounds(coordinate,cache.plan.level,extent);
            let key=cache.page_key(cache.graph.root.clone().unwrap(),region);
            let store=cache.pages.lock().unwrap();
            let page=store.pages.get(&key).expect("each window page must have written fallback pixels");
            assert!(Coverage::covers(&page.coverage.written,region));
            assert!(page.versions.is_empty() && page.coverage.exact.is_empty());
        }
        assert!(cache.presentation().is_some());assert!(!cache.retained_ready() && !cache.output_complete());
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);crate::test_support::complete(&renderer);
        let expected=[40.,90.,170.].map(|value|layer_core::color::RgbSpace::Srgb.decode(value/255.) as f32);
        assert!(crate::test_support::float_pixels(&renderer,cache.texture()).iter().all(|pixel|
            pixel[..3].iter().zip(expected).all(|(actual,expected)|(*actual-expected).abs()<0.001) && pixel[3]==1.));
        assert_eq!(renderer.metrics.source_upload_submissions,uploads);
        assert_eq!(renderer.source_tiles.borrow().misses,decoded);
        assert_eq!(renderer.metrics.composited_pixels,composed);
    }

    #[test]
    fn a_new_level_projects_committed_finer_pages_without_source_work() {
        let extent=[1024,512];
        let mut document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let source=layer_core::color::source::rgba8_source(extent,|_,_|[40,90,170,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=display(&document,&source,0);
        let before=renderer.scale_display.as_ref().unwrap().coverage_snapshot().written.regions(PixelRect::full(extent))
            .iter().map(|region|region.area()).sum::<u64>();
        let expected=crate::test_support::float_pixels(&renderer,renderer.scale_display.as_ref().unwrap().texture())[0];
        let uploads=renderer.metrics.source_upload_submissions;
        let decoded=renderer.source_tiles.borrow().misses;
        let composed=renderer.metrics.composited_pixels;
        let frame=display_frame(&document,1);
        let mut scene=renderer.scene.take().unwrap();
        scene.begin_frame();scene.scale_sources.prepare(&renderer,frame,&[]);
        let request=Request {plan:display_mips::Plan::at(extent,1),evaluation:Evaluation::Display};
        let (mut cache,_)=Cache::select(renderer.scale_display.take(),&renderer,frame,request,true);
        let mut commands=RegionRecords::new(&renderer);
        crate::scene::scale::tests::prepare_fixture_frame(&renderer,frame,&scene);
        cache.prepare_graph(&renderer,frame,&scene).unwrap();
        let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
        assert!(!cache.project_pages(&scene,&mut renderer,&mut encoder,&mut commands,[0;2],cache.plan.bounds).unwrap().is_empty());
        assert_eq!(encoder.pass_count(),1,"eight retained inputs must share one projection pass");
        let coverage=cache.coverage_snapshot();
        assert!(Coverage::covers(&coverage.written,cache.plan.bounds));
        assert!(coverage.exact.is_empty(),"finer display fallbacks cannot grant exact permission");
        assert_eq!(coverage.written.regions(cache.plan.bounds).iter().map(|region|region.area()).sum::<u64>(),before);
        assert!(cache.presentation().is_some());
        renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);crate::test_support::complete(&renderer);
        assert!(crate::test_support::float_pixels(&renderer,cache.texture()).iter().all(|pixel|
            pixel.iter().zip(expected).all(|(actual,expected)|(*actual-expected).abs()<3e-5)));
        assert_eq!(renderer.metrics.source_upload_submissions,uploads);
        assert_eq!(renderer.source_tiles.borrow().misses,decoded);
        assert_eq!(renderer.metrics.composited_pixels,composed);
    }

    #[test]
    fn accepted_direct_pages_share_storage_and_keep_their_frame_candidate() {
        let r=WgpuRasterizer::new_native_headless(Default::default()).unwrap();
        let parent=Key {source:None,expression:Arc::new(graph::Expression::Color([0;4])),extent:[2048;2],color:r.document_color(),
            blend_space:layer_core::BlendSpace::Linear,level:2,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
        let child=parent.finer().next().unwrap();let bytes=256*256*16;
        let direct=std::collections::HashSet::from([parent.clone(),child.clone(),child.clone()]).into_iter().collect::<Vec<_>>();
        let document=super::super::tests::document_at(parent.extent);
        let mut store=fixture_store(crate::test_support::packet(document.scene(),parent.extent));
        let (held,missing,extra)=store.reserve_inputs(&direct,2*bytes).unwrap();
        assert!(held.is_empty());assert_eq!(missing.len(),2);assert_eq!(extra,2*bytes);
        let descendants=parent.finer().flat_map(|child|std::iter::once(child.clone()).chain(child.finer().collect::<Vec<_>>())).count();
        assert!((descendants as u64+1)*bytes>2*bytes);
        for key in missing {drop(store.claim(&r,key,2*bytes).unwrap());}
        let other=Key {coordinate:[1,0],..parent.clone()};drop(store.claim(&r,other.clone(),3*bytes).unwrap());
        for key in [&parent,&other] {
            let shared=store.pages[&child].image.clone();
            accept_fixture_inputs(&mut store,key,key.plan().bounds,Evaluation::Display,vec![(child.clone(),child.plan().bounds,shared)],key.level);
        }
        assert_eq!(store.bytes(),3*bytes);assert_eq!(store.pinned_bytes(),3*bytes);
        let (held,missing,extra)=store.reserve_inputs(&direct,3*bytes).unwrap();
        assert!(missing.is_empty());assert_eq!(extra,0);drop(held);
        store.finish_inputs(&parent,parent.plan().bounds);
        let held=store.accepted_coordinates(&parent).collect::<Vec<_>>();assert_eq!(held,vec![[1,0]]);
        let view=layer_render::ViewState {width_px:256,height_px:256,document_to_surface:[1.,0.,0.,1.,0.,0.]};
        assert_eq!(nearest_pages(PixelRect::new(0,0,512,256),view,1,held,|_|true),vec![[0,0]],
            "an accepted farther page cannot precede the current focus");
        store.finish_inputs(&other,other.plan().bounds);store.trim(0);
        assert_eq!(store.bytes(),0);assert_eq!(store.pinned_bytes(),0);
    }

    #[test]
    fn panning_releases_off_view_accepted_inputs_before_the_next_view() {
        let mut r=WgpuRasterizer::new_native_headless(Default::default()).unwrap();
        let old=Key {source:None,expression:Arc::new(graph::Expression::Color([0;4])),extent:[512,256],color:r.document_color(),
            blend_space:layer_core::BlendSpace::Linear,level:0,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
        let next=Key {coordinate:[1,0],..old.clone()};
        let interests=[(0..32).map(|_|Arc::new(())).collect::<Vec<_>>(),(0..32).map(|_|Arc::new(())).collect::<Vec<_>>()];
        let document=super::super::tests::document_at(old.extent);
        let mut store=fixture_store(crate::test_support::packet(document.scene(),old.extent));
        for (key,inputs) in [(&old,&interests[0]),(&next,&interests[1])] {
            drop(store.claim(&r,key.clone(),u64::MAX).unwrap());
            hold_fixture_inputs(&mut store,key,key.plan().bounds,Evaluation::Display,inputs.iter().cloned().map(Into::into).collect::<Vec<_>>().into(),key.level);
        }
        assert!(interests.iter().flatten().all(|interest|Arc::strong_count(interest)==2));
        store.set_expected(Arc::new(SignatureFrame::from_snapshot(store.expected().unwrap().snapshot())));
        store.retain_work(&[Interest::new(next.clone(),Evaluation::Display,next.plan().bounds,next.level)]);
        assert!(interests[0].iter().all(|interest|Arc::strong_count(interest)==1),"off-view accepted result interest ends at the wanted-set change");
        assert!(interests[1].iter().all(|interest|Arc::strong_count(interest)==2));
        assert_eq!(store.pinned_bytes(),store.pages[&next].image.bytes());
        assert!(!store.pages.contains_key(&old));
        let inputs=store.take_inputs(&next,next.plan().bounds);
        assert_eq!(inputs.len(),32,"the new view retains its admitted inputs and can run");
        let image=store.allocated(&next).unwrap();
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        let write=store.prepare_write(next.clone(),next.plan().bounds,Accuracy::Display,Freshness::Current,next.level).unwrap();
        drop(encoder.color_pass("next accepted view",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
        crate::scene::sources::Lease::track(inputs.into_iter().collect(),&mut encoder);
        store.commit(write,encoder.submission());
        r.uploads.finish(&encoder);r.last_submission=Some(encoder.submit(&r.queue));r.wait_idle().unwrap();
        assert!(store.get(&next,next.plan().bounds,Accuracy::Display,next.level).is_some());
        assert!(crate::test_support::float_pixels(&r,&image.texture).iter().all(|pixel|*pixel==[1.;4]));
        drop(image);store.trim(0);
        assert!(interests.iter().flatten().all(|interest|Arc::strong_count(interest)==1));
        assert_eq!(store.pinned_bytes(),0);assert_eq!(store.bytes(),0);
    }

    #[test]
    fn cancelled_parents_release_only_their_raw_chunk_interest() {
        let document=super::super::tests::document_at([1024;2]);
        let id=document.scene().order().iter().find_map(|owner|document.scene().source_target(*owner).filter(|id|matches!(id,SourceTarget::Paint(_)))).unwrap();
        let r=WgpuRasterizer::new_native_headless(Default::default()).unwrap();
        for self_destination in [false,true] {for finish in [false,true] {
            let parent=Key {source:None,expression:Arc::new(graph::Expression::Color([0;4])),extent:[1024;2],color:r.document_color(),
                blend_space:layer_core::BlendSpace::Linear,level:0,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
            let other=Key {coordinate:[1,0],..parent.clone()};
            let source=Key {source:Some(id),level:2,..parent.clone()};
            let chunks=[page_rect([0,0]),page_rect([1,0])];let interests=[Arc::new(()),Arc::new(())];
            let mut store=fixture_store(crate::test_support::packet(document.scene(),parent.extent));
            for key in [&parent,&other,&source] {drop(store.claim(&r,key.clone(),u64::MAX).unwrap());}
            for (index,key) in [&parent,&other].into_iter().enumerate() {
                let shared=store.pages[&source].image.clone();
                accept_fixture_inputs(&mut store,key,key.plan().bounds,Evaluation::Display,vec![(source.clone(),chunks[index],shared.clone())],key.level);
                if self_destination {accept_fixture_inputs(&mut store,&source,chunks[index],Evaluation::Display,vec![(source.clone(),chunks[index],shared.clone())],source.level);}
                hold_fixture_inputs(&mut store,&source,chunks[index],Evaluation::Display,vec![interests[index].clone().into()].into(),source.level);
            }
            assert_eq!(Arc::strong_count(&interests[0]),2);assert_eq!(Arc::strong_count(&interests[1]),2);
            if finish {store.finish_inputs(&parent,parent.plan().bounds);}
            store.set_expected(Arc::new(SignatureFrame::from_snapshot(store.expected().unwrap().snapshot())));
            store.retain_work(&[Interest::new(other.clone(),Evaluation::Display,other.plan().bounds,other.level),Interest::new(source.clone(),Evaluation::Display,source.plan().bounds,source.level)]);
            assert_eq!(Arc::strong_count(&interests[0]),1,"another parent using the same source key must not keep the cancelled chunk: self_destination={self_destination} finish={finish}");
            assert_eq!(Arc::strong_count(&interests[1]),2);assert!(!store.pages.contains_key(&parent));
            assert!(store.accepted(&other,other.plan().bounds));
            store.finish_inputs(&other,other.plan().bounds);
            store.set_expected(Arc::new(SignatureFrame::from_snapshot(store.expected().unwrap().snapshot())));
            store.retain_work(&[Interest::new(source.clone(),Evaluation::Display,source.plan().bounds,source.level)]);
            assert_eq!(Arc::strong_count(&interests[1]),1);store.trim(0);assert_eq!(store.bytes(),0);
        }}
    }

    #[test]
    fn keyed_pages_stay_physical_until_submission_and_read_leases_finish() {
        use crate::submission::ColorPass;
        let r=WgpuRasterizer::new_native_headless(Default::default()).unwrap();
        let key=Key {source:None,expression:Arc::new(graph::Expression::Color([0;4])),extent:[PAGE_SIZE;2],color:r.document_color(),
            blend_space:layer_core::BlendSpace::Linear,level:0,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float};
        let document=super::super::tests::document_at(key.extent);
        let mut store=fixture_store(crate::test_support::packet(document.scene(),key.extent));let bounds=key.plan().bounds;
        let image=store.claim(&r,key.clone(),u64::MAX).unwrap();
        let bytes=image.bytes();
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        let write=store.prepare_write(key.clone(),bounds,Accuracy::Exact,Freshness::Current,0).unwrap();
        drop(encoder.color_pass("committed page lifetime",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
        store.commit(write,encoder.submission());
        drop(image);
        store.trim(0);
        assert_eq!(store.bytes(),bytes);
        assert_eq!(store.pinned_bytes(),bytes,"encoding storage cannot disappear before submission");
        encoder.submit(&r.queue);crate::test_support::complete(&r);
        assert_eq!(store.pinned_bytes(),0);
        let lease=store.get(&key,bounds,Accuracy::Exact,0).unwrap();
        store.trim(0);
        assert_eq!(store.bytes(),bytes);assert_eq!(store.pinned_bytes(),bytes);
        let mut discarded=submission::CommandEncoder::new(&r.device,&Default::default());
        store.read(&key,&lease,discarded.submission());
        drop(lease);store.trim(0);
        assert_eq!(store.bytes(),bytes,"an encoded reader retains its physical input");
        drop(discarded);
        assert!(store.get(&key,bounds,Accuracy::Exact,0).is_some(),"discarding a reader cannot invalidate its completed producer");
        store.trim(0);
        assert_eq!(store.bytes(),0);assert_eq!(store.pinned_bytes(),0);
    }

    #[test]
    fn complete_native_pages_advance_before_the_coarse_parent_is_exact() {
        let extent=[4352,512];
        let mut artwork=layer_core::authored::Artwork::new(extent).unwrap();
        crate::test_support::add_paint(&mut artwork,"Pixels",extent);
        let composition=artwork.compositions.get_mut(artwork.root).unwrap();
        composition.color.depth=layer_core::color::SampleDepth::U8;composition.blend=layer_core::BlendSpace::Linear;
        let mut document=layer_core::Document::from_artwork(artwork).unwrap();
        let source=layer_core::color::source::rgba8_source(extent,|x,y|[x as u8,y as u8,91,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=display(&document,&source,1);
        let frame=display_frame(&document,1);
        let cache=renderer.scale_display.as_ref().unwrap();
        assert_eq!(cache.precision,crate::display_precision::DisplayPrecision::Half);
        assert!(cache.coverage_snapshot().exact.is_empty());
        let mut key=cache.page_key(cache.graph.root.clone().unwrap(),page_rect([0;2]));
        let store=cache.pages.clone();
        assert!(store.lock().unwrap().get(&key,key.plan().bounds,Accuracy::Display,key.level).is_some());
        key.format=wgpu::TextureFormat::Rgba32Float;
        refine(&mut renderer,frame);
        let cache=renderer.scale_display.as_ref().unwrap();
        let strip=cache.exact_tile.as_ref().unwrap();
        assert!(u64::from(strip.texture.width())*u64::from(strip.texture.height())<=SOURCE_SLOTS as u64*u64::from(PAGE_SIZE).pow(2));
        let native=Key {level:0,coordinate:[0,0],..key.clone()};
        let native_count=|store:&mut Store|page_coordinates(PixelRect::full(extent)).filter(|coordinate| {
            let native=Key {coordinate:*coordinate,..native.clone()};
            store.get(&native,native.plan().bounds,Accuracy::Exact,0).is_some()
        }).count();
        for coordinate in [[0,0],[1,0]] {
            let native=Key {coordinate,..native.clone()};
            assert!(store.lock().unwrap().get(&native,native.plan().bounds,Accuracy::Exact,0).is_some());
        }
        assert!(store.lock().unwrap().get(&key,key.plan().bounds,Accuracy::Exact,0).is_none(),"a coarse parent owes all four complete native children before exact reduction");
        assert!(cache.coverage_snapshot().exact.is_empty(),"complete native top pages cannot grant a partial coarse exact permission");
        let mut completed=native_count(&mut store.lock().unwrap());
        assert_eq!(completed,2,"the first strip commits two whole native pages");
        let count=page_coordinates(PixelRect::full(extent)).count();
        for _ in 1..count {
            if !renderer.scale_display.as_ref().unwrap().has_pending_work(&renderer) {break;}
            refine(&mut renderer,frame);
            let cache=renderer.scale_display.as_ref().unwrap();
            let next=native_count(&mut store.lock().unwrap());
            assert!(next>completed,"ready native pages must advance without recapturing the same page");
            assert!(cache.presentable_pages()>0);completed=next;
        }
        let cache=renderer.scale_display.as_ref().unwrap();
        assert_eq!(completed,count);assert!(!cache.has_pending_work(&renderer));
        for column in 0..extent[0].div_ceil(2*PAGE_SIZE) {
            let key=Key {coordinate:[column,0],..key.clone()};
            assert!(store.lock().unwrap().get(&key,key.plan().bounds,Accuracy::Exact,0).is_some());
        }
    }

    #[test]
    fn full_edge_store_fill_and_exact_reduction_stay_inside_the_display_grant() {
        use crate::submission::ColorPass;
        use layer_core::{DeviceMemoryFacts,MemoryBudget,MemoryFacts};
        let mut failures=Vec::new();
        for exact in [false,true] {
            let extent=[8192;2];
            let mut document=layer_core::Document::new(layer_core::PortableId::random(),extent[0],extent[1],
                layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
            document.artwork.compositions.get_mut(document.artwork.root).unwrap().color.depth=layer_core::color::SampleDepth::F32;
            let mut r=crate::memory::memory_tests::renderer_in_budget(document.composition().color,
                DeviceMemoryFacts {budget_bytes:Some(768<<20),unified:false,..Default::default()},
                MemoryBudget::with_facts(MemoryFacts {host_default_bytes:Some(2<<30),override_bytes:Some(2<<30),..Default::default()})).unwrap();
            r.ensure_document_metadata(extent,document.scene()).unwrap();
            let mut frame=display_frame(&document,1);frame.view.width_px=2048;frame.view.height_px=2048;
            r.memory_demand(frame);r.prepare_moving_projection(frame).unwrap();r.publish_memory(true);
            r.factory.memory_budget().flush_reports();r.refresh_memory_limits();
            let mut scene=Scene::new(&r).unwrap();scene.scale_sources.prepare(&r,frame,&[]);
            let mut commands=RegionRecords::with_capacity(&r,1<<20);
            let window=if r.memory.scene_limit().saturating_sub(scene.working_bytes()+commands.storage_bytes())>=80<<20 {4096}else {2048};
            frame.view.width_px=window/2;frame.view.height_px=window/2;
            r.memory_demand(frame);r.prepare_moving_projection(frame).unwrap();r.publish_memory(true);
            r.factory.memory_budget().flush_reports();r.refresh_memory_limits();
            let mut cache=Cache::new(&r,Request {plan:display_mips::Plan::window(extent,1,PixelRect::full([window;2])),evaluation:Evaluation::Display},document.scene().order().len());
            crate::scene::scale::tests::prepare_fixture_frame(&r,frame,&scene);
            cache.prepare_graph(&r,frame,&scene).unwrap();cache.overview=None;
            let retained=[commands.storage_bytes(),0];
            let root=cache.graph.root.clone().unwrap();
            let mut key=cache.page_key(root,page_rect([0;2]));key.format=wgpu::TextureFormat::Rgba32Float;
            let page_bytes=key.plan().size.map(u64::from).into_iter().product::<u64>()*16;
            let window_bytes=cache.plan.size.map(u64::from).into_iter().product::<u64>()*16
                +cache.plan.level_size(cache.plan.level+1).map(u64::from).into_iter().product::<u64>()*16;
            let initial_scene=r.memory.scene_limit();let initial_display=r.memory.display_owner().grant().gpu_bytes;
            let reservation=cache.reserve_bytes(&r,&scene,retained,window_bytes,0).unwrap_or_else(||panic!("projection setup: Scene={initial_scene} Display={initial_display} window_bytes={window_bytes}"));
            let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
            cache.pixels.ensure(&r,cache.plan,wgpu::TextureFormat::Rgba32Float);
            for image in [cache.pixels.root().unwrap(),cache.pixels.next().unwrap()] {
                drop(encoder.color_pass("initialized retained projection",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
            }
            cache.pixels.commit_root(encoder.submission(),Vec::new(),Vec::new(),None);cache.pixels.commit_next(encoder.submission());
            cache.commit_bytes(&r,&scene,retained,reservation).unwrap();
            encoder.submit(&r.queue);crate::test_support::complete(&r);
            let mut held=Vec::new();
            let mut group=0usize;
            let parents=coordinates(cache.plan.bounds,cache.plan.level).collect::<Vec<_>>();
            if exact {
                loop {
                    let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,retained));
                    let grant=r.memory.display_owner().grant().gpu_bytes;
                    if report.gpu_bytes+4*page_bytes>grant {break;}
                    let Some(&parent)=parents.get(group) else {break;};
                    let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
                    for coordinate in [[parent[0]*2,parent[1]*2],[parent[0]*2+1,parent[1]*2],
                        [parent[0]*2,parent[1]*2+1],[parent[0]*2+1,parent[1]*2+1]] {
                        let child=Key {level:0,coordinate,..key.clone()};
                        let image=cache.pages.lock().unwrap().claim(&r,child.clone(),u64::MAX).unwrap();
                        let write=cache.pages.lock().unwrap().prepare_write(child.clone(),image.plan.bounds,Accuracy::Exact,Freshness::Current,0).unwrap();
                        drop(encoder.color_pass("ready exact child",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
                        cache.pages.lock().unwrap().commit(write,encoder.submission());held.push(image);
                    }
                    encoder.submit(&r.queue);crate::test_support::complete(&r);
                    let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,retained));
                    assert!(report.gpu_bytes<=r.memory.display_owner().grant().gpu_bytes,"exact-child setup must fit the real Core grant");
                    group+=1;
                }
                assert!(group>0);
            } else {
                let coarse=Key {level:5,..key.clone()};
                let image=cache.pages.lock().unwrap().claim(&r,coarse.clone(),u64::MAX).unwrap();
                let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
                let write=cache.pages.lock().unwrap().prepare_write(coarse.clone(),image.plan.bounds,Accuracy::Display,Freshness::Current,coarse.level).unwrap();
                drop(encoder.color_pass("committed coarse fallback",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
                cache.pages.lock().unwrap().commit(write,encoder.submission());held.push(image);
                encoder.submit(&r.queue);crate::test_support::complete(&r);
            }
            let id=document.active_target().unwrap();let mut coordinate=0u32;
            loop {
                let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,retained));
                let grant=r.memory.display_owner().grant().gpu_bytes;
                assert!(report.gpu_bytes<=grant,"retention setup must stay inside the actual Display grant");
                if report.gpu_bytes+page_bytes>grant {break;}
                let raw=Key {source:Some(id),expression:Arc::new(graph::Expression::Color([0;4])),level:0,
                    coordinate:[coordinate%32,coordinate/32],..key.clone()};
                assert!(raw.coordinate[1]<32);
                let image=cache.pages.lock().unwrap().claim(&r,raw.clone(),u64::MAX).unwrap();
                let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
                let write=cache.pages.lock().unwrap().prepare_write(raw.clone(),image.plan.bounds,Accuracy::Exact,Freshness::Current,0).unwrap();
                drop(encoder.color_pass("retained raw read lease",&image.view,wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)));
                cache.pages.lock().unwrap().commit(write,encoder.submission());held.push(image);
                encoder.submit(&r.queue);crate::test_support::complete(&r);coordinate+=1;
            }
            let before=r.display_memory_report(Some(&scene),cache.frame_memory(&r,retained));
            assert!(r.memory.display_owner().grant().gpu_bytes-before.gpu_bytes<page_bytes,"a new page needs a genuine working admission");
            let store_bytes=cache.pages.lock().unwrap().bytes();
            assert!(store_bytes<cache_bytes(&r),"plain Store capacity: Store={store_bytes} capacity={} initial_Scene={initial_scene} initial_Display={initial_display} window_bytes={window_bytes}",cache_bytes(&r));
            for frame_index in 0..3 {
                scene.begin_frame();commands.begin();
                let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
                let result=if exact {cache.refine(&mut scene,&mut r,frame,&mut encoder,&mut commands)}
                    else {cache.project_pages(&scene,&mut r,&mut encoder,&mut commands,retained,cache.plan.bounds)};
                r.uploads.finish(&encoder);encoder.submit(&r.queue);crate::test_support::complete(&r);
                let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,[commands.storage_bytes(),0]));
                let grant=r.memory.display_owner().grant().gpu_bytes;
                if report.gpu_bytes>grant {
                    failures.push(format!("exact={exact} frame={frame_index}: physical={} grant={grant} pinned={} Store={} projection={} initial_Scene={initial_scene} initial_Display={initial_display} window_bytes={window_bytes} result={result:?}",
                        report.gpu_bytes,report.pinned_gpu_bytes,cache.pages.lock().unwrap().bytes(),cache.window_bytes()));
                    break;
                }
                assert!(result.is_ok() || matches!(result,Err(GpuRasterError::NotReady)),"{result:?}");
            }
            drop(held);
        }
        assert!(failures.is_empty(),"{}",failures.join("\n"));
    }

    #[test]
    fn refinement_replaces_display_pages_in_the_same_keyed_store() {
        let mut document=layer_core::Document::new(layer_core::PortableId::random(),512,512,
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        let root=document.artwork.root;
        document.artwork.compositions.get_mut(root).unwrap().blend=layer_core::BlendSpace::Linear;
        let source=layer_core::color::source::rgba8_source([512;2],|x,y|[x as u8,y as u8,91,255]);
        paint_mut(&mut document,0).base=Some(layer_core::authored::PaintBase::new(source.clone().into()));
        let mut renderer=display(&document,&source,0);
        let frame=display_frame(&document,0);
        let cache=renderer.scale_display.as_ref().unwrap();
        let key=cache.page_key(cache.graph.root.clone().unwrap(),page_rect([0;2]));
        let store=cache.pages.clone();
        assert!(store.lock().unwrap().get(&key,page_rect([0;2]),Accuracy::Display,key.level).is_some());
        assert!(store.lock().unwrap().get(&key,page_rect([0;2]),Accuracy::Exact,0).is_none(),
            "Display composition is not an exact capture");
        let coarse=Key {level:1,coordinate:[0;2],..key.clone()};
        let (_,accuracy,freshness)=store.lock().unwrap().reduction(&coarse,0).unwrap();
        assert_eq!(accuracy,Accuracy::Display);assert_eq!(freshness,Freshness::Current);
        let deadline=std::time::Instant::now()+READBACK_TIMEOUT;
        loop {
            let mut scene=renderer.scene.take().unwrap();
            let mut encoder=submission::CommandEncoder::new(&renderer.device,&Default::default());
            let result=scene.refine_display(&mut renderer,frame,&mut encoder);
            renderer.scene=Some(scene);
            assert!(result.is_ok() || matches!(result,Err(GpuRasterError::NotReady)),"{result:?}");
            renderer.uploads.finish(&encoder);encoder.submit(&renderer.queue);renderer.wait_idle().unwrap();
            if store.lock().unwrap().get(&key,page_rect([0;2]),Accuracy::Exact,0).is_some() {break;}
            assert!(std::time::Instant::now()<deadline,"approximate pages must remain owed exact refinement");
        }
        assert!(Arc::ptr_eq(&store,&renderer.scale_display.as_ref().unwrap().pages));
    }
}

#[cfg(test)]
pub(super) struct RetainedPage {
    pub key: Key,
    pub texture: wgpu::Texture,
    pub written:PageSet,
    pub current:PageSet,
    pub exact:PageSet,
}
#[cfg(test)]
impl Store {
    pub(super) fn remove_composites(&mut self) {
        let keys=self.pages.keys().filter(|key|key.source.is_none()).cloned().collect::<Vec<_>>();
        for key in keys {self.remove(&key);}
    }
    pub(super) fn remove_source_level(&mut self,id:SourceTarget,level:u32) {
        let keys=self.pages.keys().filter(|key|key.source==Some(id) && key.level==level).cloned().collect::<Vec<_>>();
        for key in keys {self.remove(&key);}
    }
    pub(super) fn discard_source_chunks(&mut self,id:SourceTarget,level:u32,area:PixelRect) {
        for (key,page) in &mut self.pages {
            if key.source!=Some(id) || key.level!=level {continue;}
            for coordinate in coordinates(area.intersect(key.plan().bounds),page.coverage.written.level) {
                page.versions.remove(&coordinate);page.coverage.written.entries.remove(&coordinate);page.coverage.exact.entries.remove(&coordinate);
            }
            page.inputs.retain(|input|area.intersect(input.region).is_empty());
        }
    }
    pub(super) fn retained_pages(&self) -> Vec<RetainedPage> {
        self.pages.iter().filter(|(_,page)|page.submission.as_ref().is_none_or(|submission|!submission.discarded()))
            .map(|(key,page)|RetainedPage {key:key.clone(),texture:page.image.texture.clone(),
                written:page.coverage.written.clone(),current:PageSet {level:page.coverage.written.level,entries:page.coverage.written.iter().copied().filter(|coordinate|
                    page.matches(self.expected.as_deref(),bounds(*coordinate,page.coverage.written.level,key.extent),key.level)==Some(true)).collect()},exact:page.coverage.exact.clone()}).collect()
    }
}

#[cfg(test)]
mod required_window_publication_tests {
    use super::*;

    #[test]
    fn current_exact_store_publishes_required_window_before_idle() {
        let mut document=layer_core::Document::new(layer_core::PortableId::random(),4096,4096,
            layer_core::DocumentNames {paint:"Pixels".into(),paper:"Paper".into()});
        document.artwork.compositions.get_mut(document.artwork.root).unwrap().color.depth=layer_core::color::SampleDepth::F32;
        let budget=layer_core::MemoryBudget::with_facts(layer_core::MemoryFacts {
            host_default_bytes:Some(2<<30),override_bytes:Some(2<<30),..Default::default()
        });
        let mut r=crate::memory::memory_tests::renderer_in_budget(document.composition().color,
            layer_core::DeviceMemoryFacts {budget_bytes:Some(768<<20),unified:false,..Default::default()},budget).unwrap();
        let mut frame=crate::test_support::packet(document.scene(),[4096,4096]);
        frame.view=layer_render::ViewState {width_px:1024,height_px:1024,document_to_surface:[1.,0.,0.,1.,0.,0.]};
        r.ensure_document_metadata([4096,4096],document.scene()).unwrap();
        crate::memory::memory_tests::cache_frame_with_budget(&mut r,frame,0,320<<20);
        let mut scene=Scene::new(&r).unwrap();scene.scale_sources.prepare(&r,frame,&[]);
        let mut cache=Cache::new(&r,Request {plan:display_mips::Plan::window([4096,4096],0,PixelRect::new(0,0,1024,1024)),evaluation:Evaluation::Display},document.scene().order().len());
        crate::scene::scale::tests::prepare_fixture_frame(&r,frame,&scene);
        cache.prepare_graph(&r,frame,&scene).unwrap();
        assert_ne!(cache.plan.level,overview_plan(cache.plan).level);
        let mut required=Vec::new();
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        for current in std::iter::once(&cache).chain(cache.overview.as_deref()) {
            for coordinate in coordinates(current.plan.bounds,current.plan.level) {
                let region=bounds(coordinate,current.plan.level,current.plan.extent);
                let mut key=current.page_key(current.graph.root.clone().unwrap(),region);
                key.format=wgpu::TextureFormat::Rgba32Float;
                let image=current.claim_page(&scene,&r,key.clone(),[0;2],false).unwrap().unwrap();
                let write=current.pages.lock().unwrap().prepare_write(key.clone(),region,Accuracy::Exact,Freshness::Current,0).unwrap();
                drop(encoder.color_pass("current exact required page",&image.view,wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
                current.pages.lock().unwrap().commit(write,encoder.submission());
                current.pages.lock().unwrap().set_visible(&key);
                required.push((key,image));
            }
        }
        encoder.submit(&r.queue);crate::test_support::complete(&r);
        let mut commands=RegionRecords::new(&r);
        let mut overview=cache.overview.take().unwrap();
        let retained=cache.window_memory();
        let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
        overview.project_level(&scene,&mut r,&mut encoder,&mut commands,retained,overview.plan.bounds).unwrap();
        r.uploads.finish(&encoder);encoder.submit(&r.queue);crate::test_support::complete(&r);
        assert!(overview.pixels.committed_root().is_some());cache.overview=Some(overview);
        let branch=Arc::new(graph::Expression::Color([0,1f32.to_bits(),0,1f32.to_bits()]));
        let page_bytes=u64::from(PAGE_SIZE).pow(2)*16;
        let mut index=0u32;
        while cache.pages.lock().unwrap().bytes().saturating_add(page_bytes)<=cache.store_limit(&scene,&r,[commands.storage_bytes(),0],0) {
            let region=page_rect([index%16,index/16]);index+=1;
            assert!(index<=256,"bounded optional Store fixture");
            let key=cache.page_key(branch.clone(),region);
            let image=cache.claim_page(&scene,&r,key.clone(),[commands.storage_bytes(),0],true).unwrap().unwrap();
            let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
            let write=cache.pages.lock().unwrap().prepare_write(key.clone(),region,Accuracy::Display,Freshness::Current,key.level).unwrap();
            drop(encoder.color_pass("optional subexpression retention",&image.view,wgpu::LoadOp::Clear(wgpu::Color::GREEN)));
            cache.pages.lock().unwrap().commit(write,encoder.submission());
            cache.pages.lock().unwrap().set_optional(&key);
            encoder.submit(&r.queue);crate::test_support::complete(&r);
        }
        let before=cache.pages.lock().unwrap().bytes();
        let optional_before=cache.pages.lock().unwrap().optional_bytes(0);
        let window_bytes=cache.plan.size.map(u64::from).into_iter().product::<u64>()*16;
        assert!(optional_before>=window_bytes,"optional victims can pay for the whole required window");
        assert!(cache.output_complete() && cache.retained_ready());
        assert!(cache.pixels.root().is_none());
        assert!(required.iter().all(|(_,image)|Arc::strong_count(image)>1));
        let required_bytes=required.iter().map(|(_,image)|image.bytes()).sum::<u64>();
        let scene_limit_before=r.memory.scene_limit();
        assert!(required_bytes+cache.window_bytes()+commands.storage_bytes()+scene.working_bytes()+window_bytes<=scene_limit_before,"required inputs and window fit after optional victims leave");
        assert!(before+cache.window_bytes()+commands.storage_bytes()+scene.working_bytes()+window_bytes>scene_limit_before,"the full Store must require room for publication");
        let pending_before=cache.has_pending_work(&r);
        let mut attempts=Vec::new();
        for _ in 0..2 {
            let mut encoder=submission::CommandEncoder::new(&r.device,&Default::default());
            attempts.push(cache.project_level(&scene,&mut r,&mut encoder,&mut commands,[0;2],cache.plan.bounds));
            r.uploads.finish(&encoder);encoder.submit(&r.queue);crate::test_support::complete(&r);
            let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,[0;2]));
            assert!(report.gpu_bytes<=r.memory.display_owner().grant().gpu_bytes,"every publication attempt remains admitted");
            if cache.pixels.committed_root().is_some() {break;}
        }
        let committed=cache.pixels.committed_root().is_some();
        let optional_after=cache.pages.lock().unwrap().optional_bytes(0);
        let report=r.display_memory_report(Some(&scene),cache.frame_memory(&r,[0;2]));
        let grant=r.memory.display_owner().grant().gpu_bytes;
        for(key,image)in &required {assert!(cache.pages.lock().unwrap().pages.get(key).is_some_and(|page|Arc::ptr_eq(&page.image,image)),"projection reads must survive optional eviction");}
        assert!(pending_before && attempts.iter().any(|result|result.is_ok()) && committed && optional_after<optional_before && report.gpu_bytes<=grant,
            "required publication cannot sleep behind optional retention: pending={pending_before}, attempts={attempts:?}, committed={committed}, store_before={before}, optional={optional_before}->{optional_after}, window={window_bytes}, physical={}, grant={grant}, scene_limit={scene_limit_before}->{}",report.gpu_bytes,r.memory.scene_limit());
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;
    use layer_core::{Document, authored::{Artwork,SourceTarget}, raster::{RasterData,RasterPlane,RasterRevision,RasterTile,TileKey}};

    fn fixture()->(Document,SourceTarget) {
        let mut artwork=Artwork::new([1024,512]).unwrap();
        let (_,target)=crate::test_support::add_paint(&mut artwork,"Paint",[1024,512]);
        (Document::from_artwork(artwork).unwrap(),target)
    }
    fn install(document:&mut Document,target:SourceTarget,tile:RasterTile) {
        let SourceTarget::Paint(handle)=target else {unreachable!()};
        document.artwork.paint.get_mut(handle).unwrap().raster=RasterRevision::backed(RasterData {
            tiles:[(TileKey {plane:RasterPlane::Color,coordinate:[0;2]},tile)].into(),..Default::default()});
    }
    fn frame(document:&Document)->Arc<SignatureFrame> {
        Arc::new(SignatureFrame::from_scene(crate::test_support::packet(document.scene(),document.composition().size),
            Declared {name_wake:Some(std::task::Waker::from(Arc::new(PackingNotification::default()))),..Default::default()}).unwrap())
    }
    #[derive(Default)]
    struct PackingNotification(std::sync::atomic::AtomicUsize);
    impl std::task::Wake for PackingNotification {
        fn wake(self:Arc<Self>) {self.wake_by_ref();}
        fn wake_by_ref(self:&Arc<Self>) {self.0.fetch_add(1,std::sync::atomic::Ordering::Relaxed);}
    }
    fn key(target:SourceTarget)->Key {
        Key {source:None,expression:Arc::new(graph::Expression::Source {id:target,frame:SourceFrame::at([0;2],[0;2]),extent:[1024,512],outside:0}),
            extent:[1024,512],color:Default::default(),blend_space:layer_core::BlendSpace::Linear,level:0,coordinate:[0;2],format:wgpu::TextureFormat::Rgba32Float}
    }
    fn accepted(store:&Store,key:&Key)->Recorded {
        store.prepare_write(key.clone(),key.plan().bounds,Accuracy::Exact,Freshness::Current,0).unwrap().accepted.unwrap().recorded
    }
    fn coverage(recorded:&Recorded,frame:&SignatureFrame)->Coverage {
        let mut coverage=Coverage::new(0);coverage.written.entries.insert([0;2]);
        coverage.include([0;2],recorded.matches(frame,0),true);coverage
    }
    #[test]
    fn stale_composite_spatial_recipes_do_not_acquire_a_new_authored_witness() {
        use layer_core::authored::{PortableId,Occurrence,OccurrenceContent,ImageObject,Image};
        use layer_core::authored::vector::*;
        let mut failures=Vec::new();
        for kind in ["paint","vector","objects"] {
            let (mut document,target)=fixture();
            install(&mut document,target,RasterTile::pending(RasterPlane::Color.descriptor(Default::default())));
            let mut object=None;
            let owner=match kind {
                "paint"=>document.scene().source_owner(target).unwrap(),
                "vector"=> {
                    let geometry=PathGeometry::new(vec![Subpath::new(&[[0.,0.],[256.,128.]].map(Anchor::corner),false)]).unwrap();
                    let path=PathItem::new(ItemId(1),Arc::new(geometry),Arc::new(Style::default()));
                    let vector=document.artwork.vectors.insert(PortableId::random(),VectorObject::from_records(vec![Record::Path(path)])).unwrap();
                    document.artwork.occurrences.insert(PortableId::random(),Occurrence::new(OccurrenceContent::Vector(vector),"Vector")).unwrap()
                },
                _=> {
                    let image=Image::new(layer_core::color::source::rgba8_source([1;2],|_,_|[255;4]));
                    let handle=document.artwork.objects.insert(PortableId::random(),ImageObject::new(image)).unwrap();object=Some(handle);
                    document.artwork.occurrences.insert(PortableId::random(),Occurrence::new(OccurrenceContent::Objects(handle),"Object")).unwrap()
                },
            };
            if kind!="paint" {document.artwork.stacks.get_mut(document.composition().result).unwrap().entries.push(owner);document=Document::from_artwork(document.artwork.clone()).unwrap();}
            let mut index=object_spatial::SpatialIndex::default();
            let expression=match kind {
                "paint"=>key(target).expression,
                "vector"=>Arc::new(graph::Expression::Vector {owner,offset:[0;2],moving:None}),
                _=>Arc::new(graph::Expression::Objects {content:index.content(document.scene(),owner).unwrap(),preview:false}),
            };
            let key=Key {expression,..key(target)};let before=frame(&document);
            let mut store=Store::default();store.set_expected(before.clone());let written=accepted(&store,&key);
            let raw=if kind=="paint" {Some(accepted(&store,&Key {source:Some(target),..key.clone()}))}else {None};
            if let Some(object)=object {document.artwork.objects.get_mut(object).unwrap().affine.0[4]=128.;document.artwork.objects.get_mut(object).unwrap().affine.0[5]=64.;}
            else {document.artwork.occurrences.get_mut(owner).unwrap().offset=[128,64];}
            let after=frame(&document);let authored=after.authored_revision(&key.expression).unwrap();
            assert_ne!(written.authored,authored,"{kind}: the selected model actually moved");
            let expected=after.expected_key(&key,key.plan().bounds,0).unwrap();
            if written.version.matches(expected.version())!=Some(false) {failures.push(format!("{kind}: old spatial pixel recipe still matches after selected move"));}
            if written.matches(&after,0)!=Some(false) {failures.push(format!("{kind}: recorded page is incorrectly current"));}
            if authored_matches(std::slice::from_ref(&written),&after,authored,key.plan().bounds) {failures.push(format!("{kind}: old pixels acquired the new authored witness"));}
            if let Some(raw)=raw {assert_eq!(raw.matches(&after,0),Some(true),"unchanged local source pixels remain reusable");}
        }
        assert!(failures.is_empty(),"{}",failures.join("\n"));
    }
    #[test]
    fn accepted_page_versions_are_not_replaced_by_a_later_frame_and_undo_reuses_the_witness() {
        let (mut document,target)=fixture();let descriptor=RasterPlane::Color.descriptor(Default::default());
        install(&mut document,target,RasterTile::pending(descriptor));let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before.clone());let written=accepted(&store,&key);
        install(&mut document,target,RasterTile::pending(descriptor));let after=frame(&document);store.set_expected(after.clone());
        assert_eq!(written.matches(&after,0),Some(false));
        assert!(coverage(&written,&after).needs(key.plan().bounds,Accuracy::Display));
        store.set_expected(before.clone());assert_eq!(written.matches(store.expected().unwrap(),0),Some(true));
        assert!(coverage(&written,&before).ready(key.plan().bounds,Accuracy::Exact));
    }
    #[test]
    fn undeclared_expected_inputs_need_preparation_instead_of_waiting_for_packing() {
        let (mut document,target)=fixture();let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before);let written=accepted(&store,&key);
        let SourceTarget::Paint(handle)=target else {unreachable!()};
        document.artwork.paint.get_mut(handle).unwrap().raster=RasterRevision::pending();
        let expected=frame(&document);
        assert!(matches!(expected.expected_key(&key,key.plan().bounds,0),Err(GpuRasterError::NotReady)));
        let coverage=coverage(&written,&expected);
        assert!(coverage.pending.is_empty(),"missing expected inputs are not an in-flight packing comparison");
        assert!(coverage.needs(key.plan().bounds,Accuracy::Display));
    }
    #[test]
    fn unresolved_packing_neither_requests_recomposition_nor_relabels_written_pixels() {
        let (mut document,target)=fixture();let tile=RasterTile::pending(RasterPlane::Color.descriptor(Default::default()));
        install(&mut document,target,tile.clone());let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before);let written=accepted(&store,&key);let identity=written.version.identity();
        let (canonical,comparison)=RasterTile::pending_from_live(tile.descriptor(),tile.content_version());
        install(&mut document,target,canonical);let packing=frame(&document);
        assert_eq!(written.matches(&packing,0),None);let pending=coverage(&written,&packing);
        assert!(Coverage::covers(&pending.written,key.plan().bounds));
        assert!(!pending.ready(key.plan().bounds,Accuracy::Display));assert!(!pending.needs(key.plan().bounds,Accuracy::Exact));
        comparison.resolve(false).unwrap();assert_eq!(written.matches(&packing,0),Some(true));
        assert!(coverage(&written,&packing).ready(key.plan().bounds,Accuracy::Exact));assert_eq!(written.version.identity(),identity);
    }
    #[test]
    fn packing_is_pending_only_after_subscribing_to_its_comparison() {
        let (mut document,target)=fixture();let tile=RasterTile::pending(RasterPlane::Color.descriptor(Default::default()));
        install(&mut document,target,tile.clone());let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before);let written=accepted(&store,&key);
        let (canonical,comparison)=RasterTile::pending_from_live(tile.descriptor(),tile.content_version());
        install(&mut document,target,canonical);
        let packet=crate::test_support::packet(document.scene(),document.composition().size);
        let unregistered=SignatureFrame::from_scene(packet,Declared::default()).unwrap();
        assert_eq!(written.matches(&unregistered,0),Some(false));
        assert!(coverage(&written,&unregistered).needs(key.plan().bounds,Accuracy::Display));
        let notification=Arc::new(PackingNotification::default());
        let registered=SignatureFrame::from_scene(packet,Declared {
            name_wake:Some(std::task::Waker::from(notification.clone())),..Default::default()
        }).unwrap();
        assert_eq!(written.matches(&registered,0),None);
        assert!(!coverage(&written,&registered).needs(key.plan().bounds,Accuracy::Display));
        comparison.resolve(false).unwrap();
        assert_eq!(notification.0.load(std::sync::atomic::Ordering::Relaxed),1);
        assert_eq!(written.matches(&registered,0),Some(true));
    }
    #[test]
    fn quantized_packing_is_realization_work_and_keeps_the_authored_origin() {
        let (mut document,target)=fixture();let tile=RasterTile::pending(RasterPlane::Color.descriptor(Default::default()));
        install(&mut document,target,tile.clone());let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before);let written=accepted(&store,&key);
        let (canonical,comparison)=RasterTile::pending_from_live(tile.descriptor(),tile.content_version());
        install(&mut document,target,canonical);let packing=frame(&document);comparison.resolve(true).unwrap();
        assert_eq!(written.matches(&packing,0),Some(false));assert!(coverage(&written,&packing).needs(key.plan().bounds,Accuracy::Display));
        assert_eq!(written.version.authored(),packing.expected_key(&key,key.plan().bounds,0).unwrap().version().authored());
    }
    #[test]
    fn a_resampled_publication_waits_for_its_target_level_producer() {
        let (document,target)=fixture();let frame=frame(&document);let source=key(target);
        let mut store=Store::default();store.set_expected(frame.clone());let written=accepted(&store,&source);
        let output=Key {level:1,..source.clone()};let area=source.plan().bounds;
        let authored=frame.authored_revision(&output.expression).unwrap();
        assert_eq!(written.matches(&frame,2),Some(true));
        assert_eq!(publication_matches(&frame,&output,area,2,Some((authored,std::slice::from_ref(&written)))),Some(false),
            "a finer copied image is presentable fallback, not the target-level sampling recipe");
        let same_level=accepted(&store,&output);
        assert_eq!(same_level.input_level,0);
        assert_eq!(publication_matches(&frame,&output,area,2,Some((authored,&[same_level]))),Some(true),
            "a target-level producer may retain finer exact inputs");
    }

    #[test]
    fn a_publication_cannot_match_a_new_root_only_because_its_old_expression_still_matches() {
        let (document,target)=fixture();let frame=frame(&document);let mut key=key(target);
        key.expression=Arc::new(graph::Expression::Opacity {input:key.expression.clone(),opacity:0.5f32.to_bits()});
        let mut store=Store::default();store.set_expected(frame.clone());let written=accepted(&store,&key);
        let authored=frame.authored_revision(&key.expression).unwrap();
        let next=Key {expression:Arc::new(graph::Expression::Opacity {input:key.expression.clone(),opacity:0.25f32.to_bits()}),..key.clone()};
        assert_eq!(written.matches(&frame,0),Some(true));
        assert_eq!(publication_matches(&frame,&key,key.plan().bounds,0,Some((authored,std::slice::from_ref(&written)))),Some(true));
        assert_eq!(publication_matches(&frame,&next,next.plan().bounds,0,Some((authored,&[written]))),Some(false));
    }
    #[test]
    fn clipped_copies_keep_the_input_footprint_without_claiming_unwritten_window_pixels() {
        let (document,target)=fixture();let frame=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(frame.clone());let written=accepted(&store,&key);
        let clipped=written.clipped(PixelRect::new(0,0,128,256)).unwrap();assert_eq!(clipped.region,written.region);
        assert_eq!(recorded_matches(std::slice::from_ref(&clipped),&frame,clipped.written,0),Some(true));
        assert_eq!(recorded_matches(&[clipped],&frame,key.plan().bounds,0),Some(false));
    }
    #[test]
    fn a_new_authored_input_is_needed_even_while_its_canonical_comparison_is_unresolved() {
        let (mut document,target)=fixture();let descriptor=RasterPlane::Color.descriptor(Default::default());
        install(&mut document,target,RasterTile::pending(descriptor));let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before.clone());let old=accepted(&store,&key);
        let next=RasterTile::pending(descriptor);let (canonical,_comparison)=RasterTile::pending_from_live(descriptor,next.content_version());
        install(&mut document,target,canonical);let pending=frame(&document);
        assert!(matches!(pending.expected_key(&key,key.plan().bounds,0).unwrap(),signatures::ExpectedKey::Unresolved(_)));
        assert_eq!(old.matches(&pending,0),Some(false));assert!(coverage(&old,&pending).needs(key.plan().bounds,Accuracy::Display));
        let authored=before.authored_revision(&key.expression).unwrap();
        assert_eq!(publication_matches(&pending,&key,key.plan().bounds,0,Some((authored,&[old]))),Some(false));
    }
    #[test]
    fn current_required_quality_is_not_inferred_from_a_historical_coarse_witness() {
        let (document,target)=fixture();let frame=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(frame.clone());
        let written=store.prepare_write(key.clone(),key.plan().bounds,Accuracy::Display,Freshness::Current,2).unwrap().accepted.unwrap().recorded;
        let authored=frame.authored_revision(&key.expression).unwrap();
        assert_eq!(written.matches(&frame,2),Some(true));assert_eq!(written.matches(&frame,0),Some(false));
        assert_eq!(publication_matches(&frame,&key,key.plan().bounds,0,Some((authored,std::slice::from_ref(&written)))),Some(false));
        assert_eq!(publication_matches(&frame,&key,key.plan().bounds,2,Some((authored,&[written]))),Some(true));
    }

    #[test]
    fn authored_projection_allows_camera_crops_but_rejects_a_mixed_edit() {
        let (mut document,target)=fixture();let descriptor=RasterPlane::Color.descriptor(Default::default());
        install(&mut document,target,RasterTile::pending(descriptor));let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before.clone());let old=accepted(&store,&key);
        let left=old.clipped(PixelRect::new(0,0,128,256)).unwrap();
        let right=old.clipped(PixelRect::new(128,0,256,256)).unwrap();
        assert!(authored_matches(&[left.clone(),right],&before,old.authored,key.plan().bounds));
        assert!(authored_matches(std::slice::from_ref(&left),&before,old.authored,left.written));
        install(&mut document,target,RasterTile::pending(descriptor));let after=frame(&document);store.set_expected(after.clone());
        let next=accepted(&store,&key);let right=next.clipped(PixelRect::new(128,0,256,256)).unwrap();
        assert!(!authored_matches(&[left,right],&after,next.authored,key.plan().bounds));
    }
    #[test]
    fn authored_projection_keeps_live_pixels_during_same_authored_packing() {
        let (mut document,target)=fixture();let tile=RasterTile::pending(RasterPlane::Color.descriptor(Default::default()));
        install(&mut document,target,tile.clone());let before=frame(&document);let key=key(target);
        let mut store=Store::default();store.set_expected(before);let written=accepted(&store,&key);
        let (canonical,comparison)=RasterTile::pending_from_live(tile.descriptor(),tile.content_version());
        install(&mut document,target,canonical);let packing=frame(&document);let authored=packing.authored_revision(&key.expression).unwrap();
        assert_eq!(written.matches(&packing,0),None);
        assert!(authored_matches(std::slice::from_ref(&written),&packing,authored,key.plan().bounds));
        comparison.resolve(true).unwrap();
        assert_eq!(written.matches(&packing,0),Some(false));
        assert!(authored_matches(&[written],&packing,authored,key.plan().bounds));
    }

    #[test]
    fn authored_projection_requires_every_visible_page_without_waiting_for_margin_pages() {
        let plan=display_mips::Plan::window([1024,512],0,PixelRect::full([1024,512]));
        let view=layer_render::ViewState {width_px:256,height_px:256,document_to_surface:[1.,0.,0.,1.,-256.,0.]};
        let coordinates=coordinates(plan.bounds,plan.level).collect::<Vec<_>>();
        let mut written=vec![0u32;coordinates.len().div_ceil(32)];
        let index=coordinates.iter().position(|coordinate|*coordinate==[1,0]).unwrap();written[index/32]|=1<<(index%32);
        assert!(visible_projection_complete(plan,display_mips::view_bounds(view,plan.extent,0).unwrap(),&written));
        let wider=layer_render::ViewState {width_px:512,..view};
        assert!(!visible_projection_complete(plan,display_mips::view_bounds(wider,plan.extent,0).unwrap(),&written));
        let index=coordinates.iter().position(|coordinate|*coordinate==[2,0]).unwrap();written[index/32]|=1<<(index%32);
        assert!(visible_projection_complete(plan,display_mips::view_bounds(wider,plan.extent,0).unwrap(),&written));
    }

}
