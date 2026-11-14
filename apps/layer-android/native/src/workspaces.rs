use crate::app::App;
use layer_workspace::WorkspaceInput;
use serde_json::{Value, json};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl App {
    pub fn workspace(&mut self, request: Value) -> Result<Value, String> {
        let time = now();
        if request["type"] == "start" && self.workspaces.is_none() {
                let path = request["directory"]
                    .as_str()
                    .ok_or("Workspace storage directory is missing")?;
                let wake = self.workspace_wake.clone().ok_or("Workspace storage has no owner to wake")?;
                self.workspaces = Some(crate::launch::start_workspaces(&mut self.host, std::path::Path::new(path), time, wake)?);
        }
        let Some(c) = &mut self.workspaces else {
            return Ok(Value::Null);
        };
        let changes = self.host.take_service_changes();
        c.observe_regions(&mut self.host.session, changes, time);
        if !matches!(request["type"].as_str(), Some("start" | "tick" | "capture")) {
            let input: WorkspaceInput =
                serde_json::from_value(request.clone()).map_err(|e| e.to_string())?;
            let before = self.host.session.state().revision;
            match c.input(&mut self.host.session, input, time) {
                Ok(change) => self.host.apply_change(before, change),
                Err(error) => c.set_error(error),
            }
        }
        if request["type"] == "capture" {
            return serde_json::to_value(self.host.session.capture_workspace()?)
                .map_err(|e| e.to_string());
        }
        let before = self.host.session.state().revision;
        let change = c.tick(&mut self.host.session, time);
        self.host.apply_change(before, change);
        Ok(json!({"view": c.view, "wake": change.needs_present(), "refresh": change.regions != 0}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_failure_bypasses_launch_until_retry_clears_it() {
        let directory = std::env::temp_dir().join(format!("capy-android-launch-{}", layer_workspace::new_id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut app = App::new("", &["en-US"], None).unwrap();
        app.workspace_wake = Some(std::sync::Arc::new(|| {}));
        app.workspace(json!({"type":"start","directory":directory})).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !app.workspaces.as_ref().unwrap().view.ready {
            app.workspace(json!({"type":"tick"})).unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(app.host.launch_state().visible);
        app.workspaces.as_mut().unwrap().set_error(layer_workspace::StoreError::new(
            layer_workspace::ErrorKind::StorageFull, "Storage full"));
        app.workspace(json!({"type":"tick"})).unwrap();
        let failed = app.host.launch_state();
        assert!(!failed.visible);
        assert!(!failed.finished);
        assert_eq!(failed.frame, None);
        app.workspace(json!({"type":"form","action":{"type":"save_as_new"}})).unwrap();
        assert!(app.workspaces.as_ref().unwrap().view.prompt.is_some());
        assert!(!app.host.launch_state().visible);
        assert!(!app.host.launch_state().finished);
        app.workspace(json!({"type":"cancel"})).unwrap();
        assert!(app.host.launch_state().visible);
        app.workspaces.as_mut().unwrap().set_error(layer_workspace::StoreError::new(
            layer_workspace::ErrorKind::StorageFull, "Storage full"));
        app.workspace(json!({"type":"tick"})).unwrap();
        assert!(!app.host.launch_state().visible);
        app.workspace(json!({"type":"retry"})).unwrap();
        while app.workspaces.as_ref().unwrap().view.error.is_some() || app.workspaces.as_ref().unwrap().view.busy {
            app.workspace(json!({"type":"tick"})).unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(app.host.launch_state().visible);
        assert!(!app.host.launch_state().finished);
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
