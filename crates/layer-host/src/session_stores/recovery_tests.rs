use super::*;
use layer_core::package::session_store::{NativeStoreOpener, OwnerLock, SessionStore, collect_unreferenced_stores, import_session_directories, packed_sessions};
use layer_ui::{Localizer, UiLanguage, document_recovery_unavailable, session_recovery::{ClaimedManifest, SessionDrawing, SessionManifest}};
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

#[derive(Clone, Copy, Debug)]
enum Layout { Flat, Window }
#[derive(Clone, Copy, Debug)]
enum Saved { TornIntentUnused, ForeignRetired, TornIntentListed, UnreadableDrawing, BrokenDrawing, UnparsableList, InvalidList, LegacyConflict, LegacyUnparsable, StrayRootLock }
#[derive(Clone, Copy, Debug, PartialEq)]
enum Store { Retired, Unchanged }
struct Expected { notice: bool, prompt: bool, listed: &'static [&'static str], target: &'static str, store: Store }
const STARTS: [(Saved, Expected); 10] = [
    (Saved::TornIntentUnused, Expected { notice: false, prompt: false, listed: &["listed"], target: "unused", store: Store::Retired }),
    (Saved::ForeignRetired, Expected { notice: false, prompt: false, listed: &["listed"], target: "unused", store: Store::Unchanged }),
    (Saved::TornIntentListed, Expected { notice: false, prompt: false, listed: &["listed"], target: "listed", store: Store::Unchanged }),
    (Saved::UnreadableDrawing, Expected { notice: false, prompt: true, listed: &[], target: "listed", store: Store::Retired }),
    (Saved::BrokenDrawing, Expected { notice: false, prompt: true, listed: &[], target: "listed", store: Store::Unchanged }),
    (Saved::UnparsableList, Expected { notice: true, prompt: false, listed: &[], target: "listed", store: Store::Unchanged }),
    (Saved::InvalidList, Expected { notice: true, prompt: false, listed: &[], target: "listed", store: Store::Unchanged }),
    (Saved::LegacyConflict, Expected { notice: false, prompt: false, listed: &["legacy", "listed"], target: "listed", store: Store::Unchanged }),
    (Saved::LegacyUnparsable, Expected { notice: true, prompt: false, listed: &["listed"], target: "listed", store: Store::Unchanged }),
    (Saved::StrayRootLock, Expected { notice: false, prompt: false, listed: &["listed"], target: "listed", store: Store::Unchanged }),
];
const ROOT_LOCKS: [&str; 2] = [".lock", "owner.lock"];
const UNPARSABLE: &[u8] = b"{not json";
const INVALID: &[u8] = br#"{"generation":1,"drawings":[{"id":1,"key":"listed"},{"id":1,"key":"other"}],"active":1,"clean_exit":false,"restoring":[],"blocked":[]}"#;

type Files = BTreeMap<String, Option<Vec<u8>>>;
#[derive(Debug, Default, PartialEq)]
struct Start { notices: Vec<String>, prompts: Vec<String>, errors: Vec<String> }

impl Layout {
    fn names(self) -> (&'static str, &'static str) {
        match self { Self::Flat => (".json", ".lock"), Self::Window => ("session.json", "owner.lock") }
    }
    fn manifest(self, packed: &Path) -> PathBuf {
        match self { Self::Flat => packed.join("window.json"), Self::Window => packed.join("window").join("session.json") }
    }
    fn stores(self, packed: &Path) -> PathBuf {
        match self { Self::Flat => packed.to_path_buf(), Self::Window => packed.join("window") }
    }
    fn windows(self, packed: &Path) -> Vec<(PathBuf, PathBuf, PathBuf)> {
        let mut windows: Vec<_> = std::fs::read_dir(packed).unwrap().map(|entry| entry.unwrap().path()).filter_map(|path| match self {
            Self::Flat => path.extension().is_some_and(|extension| extension == "json").then(|| (path.clone(), path.with_extension("lock"), packed.to_path_buf())),
            Self::Window => path.join("session.json").is_file().then(|| (path.join("session.json"), path.join("owner.lock"), path.clone())),
        }).collect();
        windows.sort();
        windows
    }
}

fn files(root: &Path, path: &Path) -> Files {
    let mut files = Files::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).into_iter().flatten() {
            let path = entry.unwrap().path();
            let name = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if path.symlink_metadata().unwrap().is_dir() { pending.push(path); files.insert(name, None); }
            else { files.insert(name, Some(membership(std::fs::read(&path).unwrap()))); }
        }
    }
    files
}
fn membership(bytes: Vec<u8>) -> Vec<u8> {
    SessionManifest::parse(&bytes).map_or(bytes, |manifest| serde_json::to_vec(&SessionManifest { generation: 0, ..manifest }).unwrap())
}
fn checkpoint(path: &Path) {
    let mut store = pollster::block_on(SessionStore::open(path)).unwrap();
    pollster::block_on(super::tests::capture().0.checkpoint(&mut store, &AtomicBool::new(false))).unwrap();
}
fn seed(root: &Path, layout: Layout, saved: Saved) -> Option<&'static [u8]> {
    let packed = packed_sessions(root).unwrap();
    let stores = layout.stores(&packed);
    SessionManifest::default().stage(vec![SessionDrawing { id: 1, key: "listed".into() }], 1).unwrap().publish(&layout.manifest(&packed)).unwrap();
    checkpoint(&stores.join("listed"));
    match saved {
        Saved::TornIntentUnused => { checkpoint(&stores.join("unused")); std::fs::write(stores.join("unused/.retiring"), b"").unwrap(); }
        Saved::ForeignRetired => {
            checkpoint(&stores.join("unused"));
            pollster::block_on(pollster::block_on(SessionStore::open(&stores.join("unused"))).unwrap().retire()).unwrap();
            std::fs::write(stores.join("unused/.retired"), b"foreign").unwrap();
        }
        Saved::TornIntentListed => std::fs::write(stores.join("listed/.retiring"), b"").unwrap(),
        Saved::UnreadableDrawing => for name in ["head.0", "head.1"] { std::fs::write(stores.join("listed").join(name), b"invalid head").unwrap(); },
        Saved::BrokenDrawing => { std::fs::remove_dir_all(stores.join("listed/generations")).unwrap(); std::fs::write(stores.join("listed/generations"), b"not a directory").unwrap(); }
        Saved::UnparsableList => { std::fs::write(layout.manifest(&packed), UNPARSABLE).unwrap(); return Some(UNPARSABLE); }
        Saved::InvalidList => { std::fs::write(layout.manifest(&packed), INVALID).unwrap(); return Some(INVALID); }
        Saved::LegacyConflict => {
            SessionManifest::default().stage(vec![SessionDrawing { id: 1, key: "legacy".into() }], 1).unwrap().publish(&layout.manifest(root)).unwrap();
            checkpoint(&layout.stores(root).join("legacy"));
        }
        Saved::LegacyUnparsable => {
            let list = match layout { Layout::Flat => root.join("older.json"), Layout::Window => root.join("older/session.json") };
            std::fs::create_dir_all(list.parent().unwrap()).unwrap(); std::fs::write(list, UNPARSABLE).unwrap(); return Some(UNPARSABLE);
        }
        Saved::StrayRootLock => for name in ROOT_LOCKS { std::fs::write(packed.join(name), b"").unwrap(); },
    }
    None
}
fn blocked(manifest: &SessionManifest, blocked: bool) -> Vec<SessionDrawing> {
    manifest.drawings.iter().filter(|drawing| manifest.blocked.contains(&drawing.id) == blocked).cloned().collect()
}
fn reply(reply: Result<StoreReply, String>) -> Result<StoreResult, String> { reply?.try_take().ok_or("Drawing storage reply is pending")? }
fn start(root: &Path, layout: Layout) -> Start {
    let mut start = Start::default();
    let (manifest_name, lock_name) = layout.names();
    let packed = match import_session_directories(root, manifest_name, lock_name) { Ok(packed) => packed, Err(error) => { start.errors.push(error); return start; } };
    for (path, lock, directory) in layout.windows(&packed) {
        let Some(_lease) = OwnerLock::claim(&lock).unwrap() else { continue };
        let saved = match SessionManifest::read_claimed(&path) {
            Ok(ClaimedManifest::Ready(saved)) => saved,
            Ok(ClaimedManifest::SetAside { error, .. }) => { start.notices.push(document_recovery_unavailable(&Localizer::shared(UiLanguage::English), &error)); continue; }
            Ok(ClaimedManifest::Missing) => continue,
            Err(error) => { start.errors.push(error); continue; }
        };
        let stores = SessionStores::with_opener(directory.clone(), Arc::new(NativeStoreOpener), Arc::new(|work: StoreWork| work.run()), Arc::new(|| {}), native_now_ms, CompactionActivity::default());
        let mut manifest = saved.recover_interrupted().unwrap();
        manifest.publish(&path).unwrap();
        for drawing in blocked(&manifest, false) {
            manifest = manifest.begin_restore(drawing.id).unwrap(); manifest.publish(&path).unwrap();
            let attempt = *manifest.restoring.iter().find(|attempt| attempt.id == drawing.id).unwrap();
            let restored = stores.adopt(drawing.id, &drawing.key).and_then(|_| reply(stores.restore(drawing.id, ProjectLimits::for_import())));
            manifest = manifest.finish_restore(attempt, matches!(restored, Ok(StoreResult::Restored { session: Some(_), .. }))).unwrap(); manifest.publish(&path).unwrap();
        }
        for drawing in blocked(&manifest, true) {
            start.prompts.push(drawing.key.clone());
            let discarded = reply(stores.prepare_retirement(&drawing.key)).and_then(|_| {
                manifest = manifest.remove(drawing.id)?; manifest.publish(&path)?;
                reply(stores.retire_copy(&drawing.key))
            });
            if let Err(error) = discarded { start.errors.push(error); }
        }
        for _ in 0..2 {
            manifest = manifest.reconcile(manifest.drawings.clone(), manifest.active, false).unwrap(); manifest.publish(&path).unwrap();
            let lists = match layout {
                Layout::Flat => layout.windows(&packed).into_iter().map(|(path, ..)| SessionManifest::read(&path).ok().flatten()).collect(),
                Layout::Window => Some(vec![manifest.clone()]),
            };
            if let Some(lists) = lists {
                let reachable: BTreeSet<_> = lists.into_iter().flat_map(|list: SessionManifest| list.drawings).map(|drawing| drawing.key).collect();
                pollster::block_on(collect_unreferenced_stores(&directory, &reachable, &AtomicBool::new(false)));
            }
        }
    }
    start
}
fn listed(packed: &Path, layout: Layout) -> Vec<String> {
    let lists = layout.windows(packed).into_iter().filter_map(|(path, ..)| SessionManifest::read(&path).ok().flatten());
    lists.flat_map(|list| list.drawings).map(|drawing| drawing.key).collect::<BTreeSet<_>>().into_iter().collect()
}
fn store(root: &Path, path: &Path, seeded: &Files) -> Store {
    if files(root, path) == *seeded { return Store::Unchanged; }
    assert!(path.join(".retired").exists() && !path.join("head.0").exists() && !path.join("head.1").exists(), "{path:?} is neither unchanged nor retired");
    Store::Retired
}

#[test]
fn starting_three_times_shows_no_repeating_notice() {
    for (saved, expected) in STARTS {
        for layout in [Layout::Flat, Layout::Window] {
            let root = std::env::temp_dir().join(format!("capy-recovery-starts-{}", PortableId::random()));
            let damaged = seed(&root, layout, saved);
            let target = layout.stores(&root.join("packed")).join(expected.target);
            let seeded = files(&root, &target);
            let starts: Vec<_> = (0..3).map(|_| (start(&root, layout), files(&root, &root))).collect();
            let case = format!("{saved:?} {layout:?}");
            assert_eq!(starts[0].0.prompts.len(), usize::from(expected.prompt), "{case}: first start {:?}", starts[0].0);
            assert!(starts[0].0.errors.is_empty(), "{case}: first start {:?}", starts[0].0);
            let notices: Vec<_> = damaged.map(|list| document_recovery_unavailable(&Localizer::shared(UiLanguage::English), &SessionManifest::parse(list).unwrap_err())).into_iter().collect();
            assert_eq!(starts[0].0.notices, notices, "{case}: first start");
            assert_eq!(notices.len(), usize::from(expected.notice), "{case}");
            assert!(damaged.is_none_or(|list| starts[2].1.iter().any(|(name, bytes)| Path::new(name).file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("preserved-")) && bytes.as_deref() == Some(list))), "{case}: list bytes kept");
            for (later, _) in &starts[1..] { assert_eq!(*later, Start::default(), "{case}: later start"); }
            let changed: Vec<_> = starts[1].1.keys().chain(starts[2].1.keys()).filter(|name| starts[1].1.get(*name) != starts[2].1.get(*name)).collect();
            assert!(changed.is_empty(), "{case}: later starts change {changed:?}");
            assert_eq!(listed(&root.join("packed"), layout), expected.listed, "{case}: membership");
            assert!(!matches!(saved, Saved::StrayRootLock) || ROOT_LOCKS.iter().all(|name| root.join("packed").join(name).is_file()), "{case}: root locks kept");
            assert_eq!(store(&root, &target, &seeded), expected.store, "{case}: {}", expected.target);
            std::fs::remove_dir_all(&root).unwrap();
        }
    }
}
