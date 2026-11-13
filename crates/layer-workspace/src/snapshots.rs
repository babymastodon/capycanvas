use crate::{BrowserDatabase, ErrorKind, StoreError, StoreRequest, StoreResponse, content_id, new_id};
use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const BYTE_BUDGET: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub bytes: u64,
}

pub fn prune(copies: Vec<SnapshotInfo>) -> Vec<String> {
    let mut bytes = 0u64;
    copies.into_iter().rev().enumerate().filter_map(|(index,copy)| {
        bytes = bytes.saturating_add(copy.bytes);
        (index >= 3 && (index >= 10 || bytes > BYTE_BUDGET)).then_some(copy.id)
    }).collect()
}

pub(crate) fn name(version: &str) -> String {
    let version = if !version.is_empty() && version.len() <= 80
        && version.bytes().all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c)) {version} else {"earlier"};
    format!("workspaces.{version}.{}",new_id())
}

pub(crate) fn managed_name(name: &str) -> bool {
    if name.starts_with("workspaces.damaged.") { return false; }
    name.strip_prefix("workspaces.").and_then(|name|name.rsplit_once('.')).is_some_and(|(_,id)|uuid::Uuid::parse_str(id).is_ok())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct VersionMarker {
    pub version: String,
    pub unreadable_identity: Option<String>,
    #[serde(default)]
    pub copies: Vec<SnapshotInfo>,
}
impl VersionMarker {
    pub(crate) fn finish(mut self, copies: Vec<SnapshotInfo>, adopted: bool) -> (Self, Vec<String>) {
        self.copies.extend(copies);
        let remove = if adopted {prune(self.copies.clone())} else {Vec::new()};
        self.copies.retain(|copy|!remove.contains(&copy.id));
        self.version = VERSION.into();
        (self, remove)
    }
}

#[derive(Default, Deserialize)]
pub struct SnapshotEnvironment {
    pub marker: Option<VersionMarker>,
    pub unreadable: Option<String>,
    #[serde(default)]
    pub copies: Vec<RetainedCopy>,
}

#[derive(Deserialize)]
pub struct RetainedCopy {
    pub id: String,
    pub snapshot: String,
}

#[derive(Serialize)]
pub struct SnapshotCopy {
    #[serde(flatten)]
    pub info: SnapshotInfo,
    pub snapshot: String,
}

#[derive(Serialize)]
pub struct SnapshotPlan {
    pub marker: VersionMarker,
    pub copies: Vec<SnapshotCopy>,
    pub remove: Vec<String>,
}

pub fn plan(snapshot: Option<&str>, environment: SnapshotEnvironment, adopted: bool) -> Result<SnapshotPlan, StoreError> {
    let mut previous = environment.marker.unwrap_or_default();
    if previous.copies.iter().any(|copy|!copy.id.strip_prefix("snapshot:").is_some_and(managed_name)) {
        return Err(StoreError::invalid("Invalid workspace snapshot identity."));
    }
    let mut copies = Vec::new();
    let mut copy = |version: &str, text: &str| copies.push(SnapshotCopy {
        info: SnapshotInfo {id:format!("snapshot:{}",name(version)),bytes:text.len() as u64},
        snapshot:text.into(),
    });
    if previous.version != VERSION && let Some(text) = snapshot
        && serde_json::from_str::<serde_json::Value>(text).map_or(true, |value| value["items"].as_object().is_some_and(|items| !items.is_empty())
            || value["editing"].is_string()
            || value["pending"].as_object().is_some_and(|pending| !pending.is_empty())) {
        copy(&previous.version,text);
    }
    let unreadable_identity = environment.unreadable.as_ref().map(|text| content_id(text.as_bytes())).or_else(||previous.unreadable_identity.clone());
    let changed = unreadable_identity != previous.unreadable_identity;
    if changed && let Some(text) = &environment.unreadable {
        copy("unreadable",text);
    }
    previous.unreadable_identity = unreadable_identity;
    let (marker, remove) = previous.finish(copies.iter().map(|copy|copy.info.clone()).collect(), adopted);
    Ok(SnapshotPlan {marker,copies,remove})
}

pub struct Protection {
    pub plan: SnapshotPlan,
    pub restored: Option<BrowserDatabase>,
    pub response: Option<StoreResponse>,
}
pub fn protect(snapshot: Option<&str>, mut environment: SnapshotEnvironment, request: &StoreRequest) -> Result<Protection, StoreError> {
    let retained = std::mem::take(&mut environment.copies);
    let mut plan = plan(snapshot, environment, matches!(request, StoreRequest::Adopted))?;
    if !matches!(request, StoreRequest::SetAside) {
        return Ok(Protection {plan, restored: None, response: None});
    }
    let damaged = snapshot.filter(|text| BrowserDatabase::decode(text).is_err_and(|error| error.kind == ErrorKind::DamagedStorage))
        .ok_or_else(|| StoreError::new(ErrorKind::Unavailable, "Only damaged storage can be set aside."))?;
    plan.copies.push(SnapshotCopy {info: SnapshotInfo {id: format!("damaged:{}", new_id()), bytes: damaged.len() as u64}, snapshot: damaged.into()});
    let installed = plan.marker.copies.iter().rev().find_map(|copy| {
        let text = plan.copies.iter().find(|new| new.info.id == copy.id).map(|new| &new.snapshot)
            .or_else(|| retained.iter().find(|old| old.id == copy.id).map(|old| &old.snapshot))?;
        BrowserDatabase::decode(text).ok().map(|database| (copy.id.clone(), database))
    });
    let restored = installed.is_some();
    let database = match installed {
        Some((id, mut database)) => {
            plan.marker.copies.retain(|copy| copy.id != id);
            plan.copies.retain(|copy| copy.info.id != id);
            plan.remove.push(id);
            database.release_unlocked(&[], u64::MAX);
            database
        }
        None => BrowserDatabase::default(),
    };
    Ok(Protection {plan, restored: Some(database), response: Some(StoreResponse::SetAside {restored})})
}

#[cfg(test)]
mod tests {
    #[test]
    fn damaged_stores_are_never_managed_version_copies() {
        let name = format!("workspaces.damaged.{}",crate::new_id());
        assert!(!super::managed_name(&name));
        assert!(super::managed_name(&super::name("1.0.15")));
    }
    use super::*;
    #[test]
    fn version_changes_and_unreadable_records_keep_original_bytes_without_repeated_copies() {
        let original = r#"{"items":{"custom":{}},"editing":"released palettes"}"#;
        let old = r#"{"items":{"legacy":{}}}"#;
        let first = plan(Some(original),SnapshotEnvironment {marker:None,unreadable:Some(old.into()),..Default::default()},false).unwrap();
        assert_eq!(first.copies.len(),2);
        assert_eq!(first.copies[0].snapshot,original);
        assert_eq!(first.copies[1].snapshot,old);
        let mut marker = first.marker.clone();
        assert!(plan(Some(original),SnapshotEnvironment {marker:Some(marker.clone()),unreadable:Some(old.into()),..Default::default()},false).unwrap().copies.is_empty());
        marker.version = "99.0.0".into();
        let down = plan(Some(original),SnapshotEnvironment {marker:Some(marker),unreadable:Some(old.into()),..Default::default()},false).unwrap();
        assert_eq!(down.copies.len(),1);
        assert_ne!(first.copies[0].info.id,down.copies[0].info.id);
    }
    const RELEASE_1_0_15: &str = include_str!("../tests/fixtures/web-store-1.0.15.json");
    #[test]
    fn damaged_current_stores_are_copied_on_version_change_and_set_aside_unmanaged() {
        let damaged = &RELEASE_1_0_15[..RELEASE_1_0_15.len()/2];
        assert_eq!(BrowserDatabase::decode(damaged).err().unwrap().kind, ErrorKind::DamagedStorage);
        let first = plan(Some(damaged),SnapshotEnvironment::default(),false).unwrap();
        assert_eq!(first.copies.len(), 1);
        assert_eq!(first.copies[0].snapshot, damaged);
        let environment = || SnapshotEnvironment {marker:Some(first.marker.clone()),..Default::default()};
        let aside = protect(Some(damaged),environment(),&StoreRequest::SetAside).unwrap();
        assert!(matches!(aside.response, Some(StoreResponse::SetAside {restored:false})));
        assert_eq!(aside.restored.unwrap().encoded().unwrap(), BrowserDatabase::default().encoded().unwrap());
        let [kept] = &aside.plan.copies[..] else {panic!()};
        assert!(kept.info.id.starts_with("damaged:"));
        assert_eq!(kept.snapshot, damaged);
        assert_eq!(aside.plan.marker.copies, first.marker.copies);
        let unknown_field = RELEASE_1_0_15.replacen('{', r#"{"future":1,"#, 1);
        BrowserDatabase::decode(&unknown_field).unwrap();
        for current in [None, Some(RELEASE_1_0_15), Some(unknown_field.as_str()), Some(r#"{"schema":99}"#)] {
            let Err(error) = protect(current,environment(),&StoreRequest::SetAside) else {panic!()};
            assert_eq!(error.kind, ErrorKind::Unavailable, "{current:?}");
        }
        for older in [r#"{"schema":8}"#, r#"{"items":{}}"#, "[]"] {
            assert_eq!(BrowserDatabase::decode(older).err().unwrap().kind, ErrorKind::DamagedStorage, "{older}");
            assert!(protect(Some(older),environment(),&StoreRequest::SetAside).is_ok(), "{older}");
        }
    }
    #[test]
    fn setting_aside_installs_and_consumes_the_newest_readable_copy() {
        let damaged = &RELEASE_1_0_15[..RELEASE_1_0_15.len()/2];
        let copies = [(format!("snapshot:{}",name("1.0.14")), RELEASE_1_0_15), (format!("snapshot:{}",name("1.0.15")), damaged)];
        let mut marker = VersionMarker {version:VERSION.into(),..Default::default()};
        marker.copies = copies.iter().map(|(id,text)| SnapshotInfo {id:id.clone(),bytes:text.len() as u64}).collect();
        let environment = |marker: &VersionMarker| SnapshotEnvironment {marker:Some(marker.clone()),unreadable:None,
            copies:copies.iter().filter(|(id,_)| marker.copies.iter().any(|copy| &copy.id == id)).map(|(id,text)| RetainedCopy {id:id.clone(),snapshot:text.to_string()}).collect()};
        let aside = protect(Some(damaged),environment(&marker),&StoreRequest::SetAside).unwrap();
        assert!(matches!(aside.response, Some(StoreResponse::SetAside {restored:true})));
        let mut expected = BrowserDatabase::decode(RELEASE_1_0_15).unwrap();
        expected.release_unlocked(&[], u64::MAX);
        assert_eq!(aside.restored.unwrap().encoded().unwrap(), expected.encoded().unwrap());
        assert_eq!(aside.plan.remove, vec![copies[0].0.clone()]);
        assert_eq!(aside.plan.marker.copies.iter().map(|copy| &copy.id).collect::<Vec<_>>(), [&copies[1].0]);
        let [kept] = &aside.plan.copies[..] else {panic!()};
        assert_eq!(kept.snapshot, damaged);
        let again = protect(Some(damaged),environment(&aside.plan.marker),&StoreRequest::SetAside).unwrap();
        assert!(matches!(again.response, Some(StoreResponse::SetAside {restored:false})), "a consumed copy is never installed again");
        assert_eq!(again.plan.marker.copies, aside.plan.marker.copies);
    }
    #[test]
    fn retention_keeps_the_newest_within_count_and_bytes_and_at_least_three() {
        for (count,size,kept) in [(12,1,10),(8,80*1024*1024,3),(8,64*1024*1024,4),(8,90*1024*1024,3),(2,300*1024*1024,2)] {
            let copies = (0..count).map(|i|SnapshotInfo {id:i.to_string(),bytes:size}).collect();
            assert_eq!(prune(copies),(0..count-kept).rev().map(|i|i.to_string()).collect::<Vec<_>>());
        }
    }
}
