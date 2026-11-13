use crate::*;
use crate::snapshots::{RetainedCopy, SnapshotEnvironment, SnapshotInfo, VersionMarker};
use layer_ui::{Localizer, Platform, UiLanguage};
use std::{cell::{Cell, RefCell}, collections::BTreeMap, path::Path, rc::Rc, sync::Arc};

const WEB_NOW: u64 = 1_900_000_000_000;
const SKETCH: &str = "builtin:workspace:painter";
const PAINT: &str = "builtin:workspace:illustrator";
const PHOTO: &str = "builtin:workspace:photographer";
const RELEASE_1_0_12: &str = include_str!("../tests/fixtures/web-store-1.0.12.json");
const RELEASE_1_0_15: &str = include_str!("../tests/fixtures/web-store-1.0.15.json");

#[derive(Default)]
struct Records {
    records: RefCell<BTreeMap<String, String>>,
    marker: RefCell<Option<VersionMarker>>,
    checked: Cell<bool>,
}
impl Records {
    fn new(database: &str, copies: &[(&str, &str)]) -> Rc<Self> {
        let records = Rc::new(Self::default());
        records.records.borrow_mut().insert("database".into(), database.into());
        if !copies.is_empty() {
            *records.marker.borrow_mut() = Some(VersionMarker {
                version: snapshots::VERSION.into(),
                unreadable_identity: None,
                copies: copies.iter().map(|(id, text)| {
                    records.records.borrow_mut().insert((*id).into(), (*text).into());
                    SnapshotInfo { id: (*id).into(), bytes: text.len() as u64 }
                }).collect(),
            });
        }
        records
    }
    fn json(&self, id: &str) -> serde_json::Value {
        serde_json::from_str(&self.records.borrow()[id]).unwrap()
    }
}
struct WebStore(Rc<Records>);
impl WorkspaceStore for WebStore {
    async fn execute(&self, request: StoreRequest) -> Result<StoreResponse, StoreError> {
        let records = &self.0;
        let protect = !records.checked.get() || request.needs_snapshots()
            || matches!(request, StoreRequest::Reopen | StoreRequest::Adopted);
        let snapshot = records.records.borrow().get("database").cloned();
        let protection = protect.then(|| {
            let copies = records.records.borrow().iter().filter(|(id, _)| id.starts_with("snapshot:"))
                .map(|(id, snapshot)| RetainedCopy { id: id.clone(), snapshot: snapshot.clone() }).collect();
            let unreadable = records.records.borrow().get("unreadable").cloned();
            snapshots::protect(snapshot.as_deref(), SnapshotEnvironment { marker: records.marker.borrow().clone(), unreadable, copies }, &request)
        }).transpose()?;
        let (plan, restored, response) = protection.map_or((None, None, None), |protection| (Some(protection.plan), protection.restored, protection.response));
        let mut database = match restored {
            Some(database) => database,
            None => snapshot.as_deref().map(BrowserDatabase::decode).transpose()?.unwrap_or_default(),
        };
        let response = match response {
            Some(response) => response,
            None => {
                let (next, response) = database.execute_owned(request, WEB_NOW)?;
                database = next;
                response
            }
        };
        if let Some(plan) = plan {
            let mut stored = records.records.borrow_mut();
            for copy in plan.copies { stored.insert(copy.info.id, copy.snapshot); }
            for id in plan.remove { stored.remove(&id); }
            *records.marker.borrow_mut() = Some(plan.marker);
        }
        records.records.borrow_mut().insert("database".into(), database.encoded()?);
        records.checked.set(true);
        Ok(response)
    }
}

fn copy_id(version: &str) -> String {
    format!("snapshot:{}", snapshots::name(version))
}
fn start_web(records: &Rc<Records>) -> (WorkspaceController<WebStore>, layer_host::NativeHost) {
    let mut host = layer_host::NativeHost::new(Platform::Web).unwrap();
    let mut controller = WorkspaceController::new_localized(WebStore(records.clone()), Platform::Web, WEB_NOW, Localizer::shared(UiLanguage::English), Arc::new(|| {}));
    settle(&mut controller, &mut host, WEB_NOW, |c| c.view.ready && !c.view.busy);
    (controller, host)
}
fn settle<S: WorkspaceStore + 'static>(controller: &mut WorkspaceController<S>, host: &mut layer_host::NativeHost, now: u64, done: impl Fn(&WorkspaceController<S>) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done(controller) && std::time::Instant::now() < deadline {
        controller.tick(&mut host.session, now);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    for _ in 0..5 { controller.tick(&mut host.session, now); }
    assert!(done(controller), "{:?}", controller.view.error);
}
fn send<S: WorkspaceStore + 'static>(controller: &mut WorkspaceController<S>, host: &mut layer_host::NativeHost, now: u64, input: WorkspaceInput) {
    controller.input(&mut host.session, input, now).unwrap();
    settle(controller, host, now, |c| !c.view.busy && !c.view.loading);
}
fn opens<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>, id: &str) -> bool {
    pollster::block_on(controller.manager.load(id)).is_ok_and(|stored| !stored.fallback)
}
fn starting_layout(id: &str, platform: Platform) -> layer_ui::DockLayout {
    Entity::included_workspace(id, platform, false, 0).unwrap().capture().unwrap().history.layout().clone()
}
fn opened_layout(name: &str, layout: layer_ui::DockLayout) -> layer_ui::DockLayout {
    let mut entity = Entity::workspace(name, layer_ui::WorkspaceCapture::from_template(&layout).unwrap(), layout, 0);
    entity.resolve_phone().unwrap();
    entity.capture().unwrap().history.layout().clone()
}
fn stored_layout<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>, id: &str) -> layer_ui::DockLayout {
    pollster::block_on(controller.manager.load(id)).unwrap().entity.capture().unwrap().history.layout().clone()
}
fn switch<S: WorkspaceStore + 'static>(controller: &mut WorkspaceController<S>, host: &mut layer_host::NativeHost, now: u64, id: &str) {
    send(controller, host, now, WorkspaceInput::Switch { id: id.into() });
    assert_eq!(controller.manager.active_id().as_deref(), Some(id), "{:?}", controller.view.error);
    assert!(controller.view.error.is_none() && controller.view.status.is_none() && controller.view.prompt.is_none(), "{id}: {:?}", controller.view.status);
}
fn edit_layout<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>, now: u64) {
    let mut capture = controller.manager.current().unwrap().capture().unwrap();
    let mut layout = capture.history.layout().clone();
    layout.canvas_bar = !layout.canvas_bar;
    capture.history.append(&layout, layer_ui::LayoutChange::CanvasBar);
    controller.manager.observe(capture, now);
    pollster::block_on(controller.manager.flush()).unwrap();
}
fn web_content(records: &Records, id: &str) -> serde_json::Value {
    records.json("database")["items"][id]["entity"]["content"].clone()
}

#[test]
fn unreadable_workspaces_open_with_their_starting_layout_and_nothing_is_written() {
    let mut database: serde_json::Value = serde_json::from_str(RELEASE_1_0_15).unwrap();
    database["items"][SKETCH]["entity"]["content"] = "{not json".into();
    database["items"][PAINT]["entity"]["content"]["history"]["current"] = "missing".into();
    let records = Records::new(&database.to_string(), &[]);
    let (mut controller, mut host) = start_web(&records);
    for id in [SKETCH, PAINT, PHOTO] {
        switch(&mut controller, &mut host, WEB_NOW, id);
    }
    for id in [SKETCH, PAINT] {
        assert_eq!(stored_layout(&controller, id), starting_layout(id, Platform::Web), "{id}");
        assert_eq!(web_content(&records, id), database["items"][id]["entity"]["content"], "{id}");
    }
    drop(controller);
    records.checked.set(false);
    let (mut controller, mut host) = start_web(&records);
    for id in [SKETCH, PAINT] {
        assert_eq!(web_content(&records, id), database["items"][id]["entity"]["content"], "restart and maintenance keep {id}");
    }
    switch(&mut controller, &mut host, WEB_NOW, SKETCH);
    edit_layout(&controller, WEB_NOW);
    let saved: ItemContent = serde_json::from_value(web_content(&records, SKETCH)).unwrap();
    saved.validate().unwrap();
    assert_eq!(web_content(&records, PAINT), database["items"][PAINT]["entity"]["content"]);
}

fn released_content(id: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(RELEASE_1_0_12).unwrap()["items"][id]["entity"]["content"].clone()
}
fn released_layout(id: &str) -> layer_ui::DockLayout {
    fn removed(tile: &serde_json::Value) -> bool {
        matches!(tile["control"]["kind"].as_str(), Some("opacity" | "brush_opacity_slider")) || tile["control"]["command"] == "decoration"
    }
    fn retain(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Array(items) => {
                items.retain(|item| !removed(item));
                items.iter_mut().for_each(retain);
            }
            serde_json::Value::Object(fields) => fields.values_mut().for_each(retain),
            _ => {}
        }
    }
    let mut content = released_content(id);
    retain(&mut content);
    let mut entity = Entity { id: id.into(), metadata: Metadata::new(ItemKind::Workspace, "Released", 0), content: serde_json::from_value(content).unwrap(), working: Some(Default::default()) };
    entity.resolve_phone().unwrap();
    entity.capture().unwrap().history.layout().clone()
}

#[test]
fn a_released_1_0_12_store_opens_every_workspace_without_asking() {
    let records = Records::new(RELEASE_1_0_12, &[]);
    let original = records.json("database");
    let (mut controller, mut host) = start_web(&records);
    assert!(notice(&host).is_none());
    for id in [SKETCH, PAINT, PHOTO] {
        switch(&mut controller, &mut host, WEB_NOW, id);
        assert_eq!(stored_layout(&controller, id), released_layout(id), "{id} keeps every released value this build reads");
        assert_eq!(web_content(&records, id), original["items"][id]["entity"]["content"], "{id}");
    }
    assert_ne!(released_layout(SKETCH), starting_layout(SKETCH, Platform::Web));
}

#[test]
fn a_native_store_with_released_1_0_12_workspaces_keeps_their_layouts() {
    let (directory, _) = native_store(false);
    let sql = rusqlite::Connection::open(directory.join("workspaces.sqlite3")).unwrap();
    for id in [SKETCH, PAINT] {
        sql.execute("UPDATE items SET content=?2 WHERE id=?1", (id, released_content(id).to_string())).unwrap();
    }
    drop(sql);
    let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
    for id in [SKETCH, PAINT] {
        switch(&mut controller, &mut host, 1000, id);
        assert_eq!(stored_layout(&controller, id), released_layout(id), "{id}");
    }
    crate::controller_tests::close_native(controller, &mut host);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn an_unreadable_custom_workspace_opens_with_its_baseline_or_the_initial_layout() {
    let mut database = BrowserDatabase::decode(RELEASE_1_0_15).unwrap();
    let baseline = layer_ui::WorkspacePreset::Photographer.layout(Platform::Web);
    let custom = Entity::workspace("Mine", layer_ui::WorkspaceCapture::from_template(&baseline).unwrap(), baseline.clone(), WEB_NOW);
    let id = custom.id.clone();
    let batch = CommitBatch::prepare(Owner::fresh(), vec![Mutation::Create { entity: custom, claim: false, name_policy: NamePolicy::Exact }]).unwrap();
    database.execute(StoreRequest::Commit { batch }, WEB_NOW).unwrap();
    let mut database: serde_json::Value = serde_json::from_str(&database.encoded().unwrap()).unwrap();
    database["items"][&id]["entity"]["content"]["history"] = "{not json".into();
    let initial = DEFAULT_WORKSPACES.iter().find(|(_, preset)| *preset == layer_ui::WorkspacePreset::initial(Platform::Web, false)).unwrap().0;
    for (broken, expected) in [(false, opened_layout("Mine", baseline)), (true, starting_layout(initial, Platform::Web))] {
        if broken { database["items"][&id]["entity"]["content"]["baseline"] = "newer".into(); }
        let records = Records::new(&database.to_string(), &[]);
        let (mut controller, mut host) = start_web(&records);
        switch(&mut controller, &mut host, WEB_NOW, &id);
        assert_eq!(controller.manager.active_name().as_deref(), Some("Mine"));
        assert_eq!(stored_layout(&controller, &id), expected, "baseline broken: {broken}");
        assert_eq!(web_content(&records, &id), database["items"][&id]["entity"]["content"]);
    }
}

#[test]
fn native_stores_open_unreadable_workspaces_with_their_starting_layout() {
    let (directory, _) = native_store(false);
    let path = directory.join("workspaces.sqlite3");
    let content = |id: &str| rusqlite::Connection::open(&path).unwrap().query_row("SELECT content FROM items WHERE id=?1", [id], |row| row.get::<_, String>(0)).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute("UPDATE items SET content='{not json' WHERE id=?1", [SKETCH]).unwrap();
    for launch in 0..2 {
        let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
        assert!(controller.view.ready && controller.view.error.is_none(), "{:?}", controller.view.error);
        switch(&mut controller, &mut host, 1000, SKETCH);
        assert_eq!(stored_layout(&controller, SKETCH), starting_layout(SKETCH, Platform::Gtk));
        switch(&mut controller, &mut host, 1000, PAINT);
        assert_eq!(content(SKETCH), "{not json", "launch {launch}");
        crate::controller_tests::close_native(controller, &mut host);
    }
    let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
    switch(&mut controller, &mut host, 1000, SKETCH);
    edit_layout(&controller, 1000);
    assert_ne!(content(SKETCH), "{not json");
    assert!(opens(&controller, SKETCH));
    crate::controller_tests::close_native(controller, &mut host);
    std::fs::remove_dir_all(directory).unwrap();
}

fn native_store(retained: bool) -> (std::path::PathBuf, String) {
    let directory = crate::test_support::temp_dir("workspace-recovery");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("workspaces.sqlite3");
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/compat/1.0.15/workspaces.sqlite3"), &path).unwrap();
    let pristine = format!("{}.sqlite3", snapshots::name(snapshots::VERSION));
    let mut marker = VersionMarker { version: snapshots::VERSION.into(), ..Default::default() };
    if retained {
        std::fs::copy(&path, directory.join(&pristine)).unwrap();
        marker.copies.push(SnapshotInfo { id: pristine.clone(), bytes: std::fs::metadata(directory.join(&pristine)).unwrap().len() });
    }
    std::fs::write(directory.join("last-version"), serde_json::to_vec(&marker).unwrap()).unwrap();
    (directory, pristine)
}
fn notice(host: &layer_host::NativeHost) -> Option<String> {
    host.session.state().notice.as_ref().map(|notice| notice.text.clone())
}
const DAMAGED_NOTICE: &str = "Your saved workspaces were damaged. Capy Canvas started new ones.";
const RESTORED_NOTICE: &str = "Your saved workspaces were damaged. Capy Canvas restored an earlier copy.";
fn quiet_start<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>) {
    assert!(controller.view.ready && controller.view.error.is_none() && controller.view.prompt.is_none() && controller.view.status.is_none(), "{:?} {:?}", controller.view.error, controller.view.prompt);
}

#[test]
fn a_damaged_web_store_installs_its_newest_readable_copy_once() {
    let truncated = &RELEASE_1_0_15[..RELEASE_1_0_15.len() / 2];
    let earlier = copy_id("1.0.14");
    let newest = copy_id(snapshots::VERSION);
    for retained in [false, true] {
        let copies = if retained { vec![(earlier.as_str(), RELEASE_1_0_15), (newest.as_str(), truncated)] } else { Vec::new() };
        let records = Records::new(truncated, &copies);
        let (controller, host) = start_web(&records);
        quiet_start(&controller);
        assert!(!controller.manager.in_memory());
        assert_eq!(notice(&host).as_deref(), Some(if retained { RESTORED_NOTICE } else { DAMAGED_NOTICE }));
        let damaged: Vec<_> = records.records.borrow().iter().filter(|(id, _)| id.starts_with("damaged:")).map(|(_, text)| text.clone()).collect();
        assert_eq!(damaged, vec![truncated.to_string()]);
        assert!(records.records.borrow().get(&earlier).is_none(), "the installed copy is consumed");
        assert!(records.marker.borrow().as_ref().unwrap().copies.iter().all(|copy| copy.id != earlier));
        if retained {
            assert!(records.records.borrow().contains_key(&newest), "a copy that can't be read stays retained");
            for (id, _) in DEFAULT_WORKSPACES {
                assert!(opens(&controller, id), "{id}");
            }
            let editing = serde_json::from_str::<serde_json::Value>(RELEASE_1_0_15).unwrap()["editing"].as_str().unwrap().to_owned();
            assert_eq!(*controller.manager.editing().unwrap(), layer_ui::EditingState::restore(&editing, &Localizer::shared(UiLanguage::English)));
        }
        drop(controller);
        records.checked.set(false);
        let (controller, host) = start_web(&records);
        quiet_start(&controller);
        assert!(!controller.manager.in_memory());
        assert_eq!(notice(&host), None);
        assert_eq!(records.records.borrow().keys().filter(|id| id.starts_with("damaged:")).count(), 1);
    }
}

#[test]
fn a_damaged_native_store_installs_its_newest_readable_copy_once() {
    for (retained, older) in [(false, false), (true, false), (true, true)] {
        let (directory, pristine) = native_store(retained);
        let database = directory.join("workspaces.sqlite3");
        if older {
            rusqlite::Connection::open(&database).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION - 1).unwrap();
        } else {
            std::fs::write(&database, b"damaged SQLite store").unwrap();
        }
        let damaged_bytes = std::fs::read(&database).unwrap();
        let expected: Option<String> = retained.then(|| rusqlite::Connection::open(directory.join(&pristine)).unwrap().query_row("SELECT json FROM editing", [], |row| row.get(0)).unwrap());
        let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
        settle(&mut controller, &mut host, 1000, |c| c.view.ready && !c.view.busy);
        quiet_start(&controller);
        assert!(!controller.manager.in_memory());
        assert_eq!(notice(&host).as_deref(), Some(if retained { RESTORED_NOTICE } else { DAMAGED_NOTICE }));
        let damaged: Vec<_> = std::fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("workspaces.damaged.") && name.ends_with(".sqlite3")).collect();
        assert_eq!(damaged.len(), 1);
        assert_eq!(std::fs::read(directory.join(&damaged[0])).unwrap(), damaged_bytes);
        assert!(!directory.join(&pristine).exists(), "the installed copy is consumed");
        if retained {
            for (id, _) in DEFAULT_WORKSPACES {
                assert!(opens(&controller, id), "{id}");
            }
            let installed: String = rusqlite::Connection::open(&database).unwrap().query_row("SELECT json FROM editing", [], |row| row.get(0)).unwrap();
            assert_eq!(Some(installed), expected);
        }
        crate::controller_tests::close_native(controller, &mut host);
        let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
        settle(&mut controller, &mut host, 1000, |c| c.view.ready && !c.view.busy);
        quiet_start(&controller);
        assert_eq!(notice(&host), None);
        crate::controller_tests::close_native(controller, &mut host);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
