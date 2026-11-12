use crate::*;
use crate::snapshots::{SnapshotEnvironment, SnapshotInfo, VersionMarker};
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
    fn copies(&self) -> Vec<String> {
        self.records.borrow().keys().filter(|id| id.starts_with("snapshot:")).cloned().collect()
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
            || matches!(request, StoreRequest::Reopen | StoreRequest::Adopted | StoreRequest::Snapshots);
        let snapshot = records.records.borrow().get("database").cloned();
        let protection = protect.then(|| {
            let restoring = match &request {
                StoreRequest::RestoreSnapshot { id } | StoreRequest::CheckSnapshot { id, .. } => records.records.borrow().get(id).cloned(),
                _ => None,
            };
            let unreadable = records.records.borrow().get("unreadable").cloned();
            snapshots::protect(snapshot.as_deref(), SnapshotEnvironment { marker: records.marker.borrow().clone(), unreadable, restoring }, &request)
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
fn submit() -> WorkspaceInput {
    WorkspaceInput::Submit { name: String::new(), description: None, choice: None }
}
fn recovery<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>) -> ManagerAction {
    let status = controller.view.status.clone().expect("a status for the failed workspace");
    assert!(status.error && !status.retry, "{status:?}");
    status.action.expect("a recovery action").action
}
fn opens<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>, id: &str) -> bool {
    pollster::block_on(controller.manager.load(id)).and_then(|stored| stored.entity.check_opens()).is_ok()
}
fn starting_layout(id: &str, platform: Platform) -> layer_ui::DockLayout {
    Entity::included_workspace(id, platform, false, 0).unwrap().capture().unwrap().history.layout().clone()
}
fn stored_layout<S: WorkspaceStore + 'static>(controller: &WorkspaceController<S>, id: &str) -> layer_ui::DockLayout {
    pollster::block_on(controller.manager.load(id)).unwrap().entity.capture().unwrap().history.layout().clone()
}

#[test]
fn an_earlier_copy_that_opens_recovers_workspaces_this_build_cannot_open() {
    let earlier = copy_id("earlier");
    let unreadable = copy_id("unreadable");
    let kept = copy_id(snapshots::VERSION);
    let records = Records::new(RELEASE_1_0_12, &[(&earlier, RELEASE_1_0_15), (&unreadable, RELEASE_1_0_12), (&kept, RELEASE_1_0_15)]);
    let (mut controller, mut host) = start_web(&records);
    assert!(controller.view.error.is_none());
    assert_eq!(controller.manager.active_id().as_deref(), Some(PHOTO), "startup skips the bound workspace it can't open");
    for id in [PAINT, SKETCH] {
        send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Switch { id: id.into() });
        assert_eq!(controller.manager.active_id().as_deref(), Some(PHOTO));
        assert_eq!(recovery(&controller), ManagerAction::RestoreCopy(kept.clone()), "{id}");
    }
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Open { page: ManagerPage::Workspaces });
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Select { id: Some(SKETCH.into()) });
    assert_eq!(controller.view.primary, "Restore Earlier Workspaces…");
    assert!(controller.view.enabled);
    let before = records.copies();
    let pre_recovery = records.json("database");
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Confirm);
    assert_eq!(controller.view.prompt_action, Some(ManagerAction::RestoreCopy(kept.clone())));
    let prompt = controller.view.prompt.clone().unwrap();
    assert_eq!(prompt.title, "Restore Earlier Workspaces?");
    assert_eq!(prompt.message, "Your workspaces, palettes and brush settings return to an earlier copy. The current ones are kept as a copy.");
    send(&mut controller, &mut host, WEB_NOW, submit());
    settle(&mut controller, &mut host, WEB_NOW, |c| c.view.ready && !c.view.busy && c.view.prompt.is_none());
    assert!(controller.view.error.is_none());
    assert_eq!(controller.view.page, None);
    assert_eq!(controller.manager.active_id().as_deref(), Some(SKETCH), "the restored copy opens the workspace that couldn't be opened");
    for (id, _) in DEFAULT_WORKSPACES {
        assert!(opens(&controller, id), "{id}");
    }
    let added: Vec<_> = records.copies().into_iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(added.len(), 1);
    assert_eq!(records.json(&added[0])["items"][PAINT]["entity"], pre_recovery["items"][PAINT]["entity"]);
    assert!(records.marker.borrow().as_ref().unwrap().copies.iter().any(|copy| copy.id == added[0]));
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Switch { id: PAINT.into() });
    assert_eq!(controller.manager.active_id().as_deref(), Some(PAINT));
    assert!(controller.view.status.is_none());
}

#[test]
fn an_included_workspace_without_an_openable_copy_restores_its_starting_layout() {
    let records = Records::new(RELEASE_1_0_12, &[]);
    let (mut controller, mut host) = start_web(&records);
    assert_eq!(controller.manager.active_id().as_deref(), Some(PHOTO));
    assert_eq!(records.copies().len(), 1, "the store this build found is kept");
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Switch { id: PAINT.into() });
    assert_eq!(recovery(&controller), ManagerAction::RestoreIncluded(PAINT.into()));
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Open { page: ManagerPage::Workspaces });
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Select { id: Some(PAINT.into()) });
    assert_eq!(controller.view.selected.as_deref(), Some(PAINT));
    assert_eq!(controller.view.primary, "Restore Starting Layout…");
    assert!(controller.view.enabled);
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Confirm);
    assert_eq!(controller.view.prompt_action, Some(ManagerAction::RestoreIncluded(PAINT.into())));
    let prompt = controller.view.prompt.clone().unwrap();
    assert_eq!(prompt.title, "Restore Starting Layout");
    assert_eq!(prompt.message, "Paint returns to its starting layout. A copy of your current workspaces is kept.");
    let before = records.copies();
    let pre_recovery = records.json("database");
    send(&mut controller, &mut host, WEB_NOW, submit());
    assert!(controller.view.error.is_none());
    assert_eq!(controller.manager.active_id().as_deref(), Some(PAINT));
    assert_eq!(controller.view.page, None);
    assert_eq!(stored_layout(&controller, PAINT), starting_layout(PAINT, Platform::Web));
    assert!(!opens(&controller, SKETCH), "other workspaces are left alone");
    let added: Vec<_> = records.copies().into_iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(added.len(), 1);
    assert_eq!(records.json(&added[0])["items"][PAINT], pre_recovery["items"][PAINT]);
}

#[test]
fn garbage_in_an_item_follows_the_same_recovery() {
    let mut database: serde_json::Value = serde_json::from_str(RELEASE_1_0_15).unwrap();
    database["items"][SKETCH]["entity"]["content"] = "{not json".into();
    let damaged = database.to_string();
    for retained in [false, true] {
        let pristine = copy_id(snapshots::VERSION);
        let copies = if retained { vec![(pristine.as_str(), RELEASE_1_0_15)] } else { Vec::new() };
        let records = Records::new(&damaged, &copies);
        let (mut controller, mut host) = start_web(&records);
        send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Switch { id: SKETCH.into() });
        let action = recovery(&controller);
        assert_eq!(action, if retained { ManagerAction::RestoreCopy(pristine.clone()) } else { ManagerAction::RestoreIncluded(SKETCH.into()) });
        let before = records.copies();
        send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Form { action });
        send(&mut controller, &mut host, WEB_NOW, submit());
        settle(&mut controller, &mut host, WEB_NOW, |c| c.view.ready && !c.view.busy && c.view.prompt.is_none());
        assert!(controller.view.error.is_none());
        assert!(opens(&controller, SKETCH));
        let added: Vec<_> = records.copies().into_iter().filter(|id| !before.contains(id)).collect();
        assert_eq!(added.len(), 1);
        assert_eq!(records.json(&added[0])["items"][SKETCH]["entity"]["content"], "{not json");
    }
}

#[test]
fn a_custom_workspace_this_build_cannot_open_never_offers_a_failing_action() {
    let mut database = BrowserDatabase::decode(RELEASE_1_0_15).unwrap();
    let custom = crate::test_support::workspace("Mine");
    let id = custom.id.clone();
    let batch = CommitBatch::prepare(Owner::fresh(), vec![Mutation::Create { entity: custom, claim: false, name_policy: NamePolicy::Exact }]).unwrap();
    database.execute(StoreRequest::Commit { batch }, WEB_NOW).unwrap();
    let mut database: serde_json::Value = serde_json::from_str(&database.encoded().unwrap()).unwrap();
    database["items"][&id]["entity"]["content"]["history"] = "{not json".into();
    let records = Records::new(&database.to_string(), &[]);
    let (mut controller, mut host) = start_web(&records);
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Switch { id: id.clone() });
    assert_eq!(recovery(&controller), ManagerAction::SaveAsNew);
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Open { page: ManagerPage::Workspaces });
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Select { id: Some(id.clone()) });
    assert_eq!(controller.view.selected, None);
    assert!(!controller.view.enabled);
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Dismiss);
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Switch { id: id.clone() });
    let previous = controller.manager.active_id();
    let action = recovery(&controller);
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Form { action });
    send(&mut controller, &mut host, WEB_NOW, WorkspaceInput::Submit { name: "Recovered".into(), description: None, choice: None });
    assert!(controller.view.error.is_none());
    assert_ne!(controller.manager.active_id(), previous);
    assert_eq!(controller.manager.active_name().as_deref(), Some("Recovered"));
}

fn native_copies(directory: &Path) -> Vec<String> {
    let mut copies: Vec<_> = std::fs::read_dir(directory).unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.strip_suffix(".sqlite3").is_some_and(snapshots::managed_name))
        .collect();
    copies.sort();
    copies
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

#[test]
fn native_stores_recover_included_workspaces_they_cannot_open() {
    for retained in [false, true] {
        let (directory, pristine) = native_store(retained);
        let path = directory.join("workspaces.sqlite3");
        rusqlite::Connection::open(&path).unwrap().execute("UPDATE items SET content='{not json' WHERE id=?1", [SKETCH]).unwrap();
        let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
        assert!(controller.view.ready && controller.view.error.is_none());
        send(&mut controller, &mut host, 1000, WorkspaceInput::Switch { id: SKETCH.into() });
        let action = recovery(&controller);
        assert_eq!(action, if retained { ManagerAction::RestoreCopy(pristine.clone()) } else { ManagerAction::RestoreIncluded(SKETCH.into()) });
        let before = native_copies(&directory);
        send(&mut controller, &mut host, 1000, WorkspaceInput::Form { action });
        send(&mut controller, &mut host, 1000, submit());
        settle(&mut controller, &mut host, 1000, |c| c.view.ready && !c.view.busy && c.view.prompt.is_none());
        assert!(controller.view.error.is_none(), "{:?}", controller.view.error);
        for (id, _) in DEFAULT_WORKSPACES {
            assert!(opens(&controller, id), "{id}");
        }
        assert_eq!(controller.manager.active_id().as_deref(), Some(SKETCH));
        if !retained {
            assert_eq!(stored_layout(&controller, SKETCH), starting_layout(SKETCH, Platform::Gtk));
        }
        let added: Vec<_> = native_copies(&directory).into_iter().filter(|id| !before.contains(id)).collect();
        assert_eq!(added.len(), 1);
        let kept: String = rusqlite::Connection::open(directory.join(&added[0])).unwrap()
            .query_row("SELECT content FROM items WHERE id=?1", [SKETCH], |row| row.get(0)).unwrap();
        assert_eq!(kept, "{not json");
        crate::controller_tests::close_native(controller, &mut host);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn a_damaged_web_store_is_set_aside_and_offers_an_earlier_copy_once() {
    let truncated = &RELEASE_1_0_15[..RELEASE_1_0_15.len() / 2];
    for retained in [false, true] {
        let pristine = copy_id(snapshots::VERSION);
        let copies = if retained { vec![(pristine.as_str(), RELEASE_1_0_15)] } else { Vec::new() };
        let records = Records::new(truncated, &copies);
        let (mut controller, mut host) = start_web(&records);
        assert!(!controller.manager.in_memory());
        assert_eq!(notice(&host).as_deref(), Some(DAMAGED_NOTICE));
        let damaged: Vec<_> = records.records.borrow().iter().filter(|(id, _)| id.starts_with("damaged:")).map(|(_, text)| text.clone()).collect();
        assert_eq!(damaged, vec![truncated.to_string()]);
        assert!(records.marker.borrow().as_ref().unwrap().copies.iter().all(|copy| copy.id.starts_with("snapshot:")));
        assert!(BrowserDatabase::decode(&records.records.borrow()["database"]).is_ok());
        if retained {
            assert_eq!(controller.view.prompt_action, Some(ManagerAction::RestoreCopy(pristine.clone())));
            assert_eq!(controller.view.prompt.as_ref().unwrap().title, "Restore Earlier Workspaces?");
            send(&mut controller, &mut host, WEB_NOW, submit());
            settle(&mut controller, &mut host, WEB_NOW, |c| c.view.ready && !c.view.busy && c.view.prompt.is_none());
            for (id, _) in DEFAULT_WORKSPACES {
                assert!(opens(&controller, id), "{id}");
            }
        } else {
            assert!(controller.view.prompt.is_none());
            assert!(records.copies().iter().any(|id| records.records.borrow()[id] == truncated), "the store found after an upgrade is kept");
        }
        assert!(controller.view.error.is_none());
        drop(controller);
        records.checked.set(false);
        let (controller, host) = start_web(&records);
        assert!(!controller.manager.in_memory());
        assert_eq!(notice(&host), None);
        assert!(controller.view.prompt.is_none());
        assert_eq!(records.records.borrow().keys().filter(|id| id.starts_with("damaged:")).count(), 1);
    }
}

#[test]
fn a_damaged_native_store_is_set_aside_and_offers_an_earlier_copy_once() {
    let (directory, pristine) = native_store(true);
    std::fs::write(directory.join("workspaces.sqlite3"), b"damaged SQLite store").unwrap();
    let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
    settle(&mut controller, &mut host, 1000, |c| c.view.prompt.is_some());
    assert!(!controller.manager.in_memory());
    assert_eq!(notice(&host).as_deref(), Some(DAMAGED_NOTICE));
    assert_eq!(controller.view.prompt_action, Some(ManagerAction::RestoreCopy(pristine)));
    let damaged: Vec<_> = std::fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("workspaces.damaged.")).collect();
    assert_eq!(damaged.len(), 1);
    assert_eq!(std::fs::read(directory.join(&damaged[0])).unwrap(), b"damaged SQLite store");
    send(&mut controller, &mut host, 1000, submit());
    settle(&mut controller, &mut host, 1000, |c| c.view.ready && !c.view.busy && c.view.prompt.is_none());
    assert!(controller.view.error.is_none(), "{:?}", controller.view.error);
    for (id, _) in DEFAULT_WORKSPACES {
        assert!(opens(&controller, id), "{id}");
    }
    crate::controller_tests::close_native(controller, &mut host);
    let (mut controller, mut host) = crate::controller_tests::start_native(&directory);
    settle(&mut controller, &mut host, 1000, |c| c.view.ready && !c.view.busy);
    assert_eq!(notice(&host), None);
    assert!(controller.view.prompt.is_none());
    for (id, _) in DEFAULT_WORKSPACES {
        assert!(opens(&controller, id), "{id}");
    }
    crate::controller_tests::close_native(controller, &mut host);
    std::fs::remove_dir_all(directory).unwrap();
}
