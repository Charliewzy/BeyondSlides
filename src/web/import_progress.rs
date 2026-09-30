use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(super) struct ImportProgress {
    pub resource: &'static str,
    pub phase: &'static str,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub completed_items: Option<usize>,
    pub total_items: Option<usize>,
}

impl Default for ImportProgress {
    fn default() -> Self {
        Self {
            resource: "preparing",
            phase: "preparing",
            downloaded_bytes: 0,
            total_bytes: None,
            completed_items: None,
            total_items: None,
        }
    }
}

#[derive(Default)]
pub(super) struct ImportTracker {
    entries: Arc<Mutex<HashMap<String, ImportProgress>>>,
}

impl ImportTracker {
    pub fn register(&self, import_id: &str) -> Result<ImportRegistration, String> {
        if import_id.is_empty()
            || import_id.len() > 64
            || !import_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("Invalid import progress identifier".into());
        }
        let mut entries = self
            .entries
            .lock()
            .expect("import progress lock is not poisoned");
        if entries.contains_key(import_id) {
            return Err("This import is already in progress".into());
        }
        entries.insert(import_id.into(), ImportProgress::default());
        Ok(ImportRegistration {
            entries: Arc::clone(&self.entries),
            import_id: import_id.into(),
        })
    }

    pub fn get(&self, import_id: &str) -> Option<ImportProgress> {
        self.entries
            .lock()
            .expect("import progress lock is not poisoned")
            .get(import_id)
            .cloned()
    }

    pub fn update(&self, import_id: &str, change: impl FnOnce(&mut ImportProgress)) {
        if let Some(progress) = self
            .entries
            .lock()
            .expect("import progress lock is not poisoned")
            .get_mut(import_id)
        {
            change(progress);
        }
    }

    pub fn has_active_import(&self) -> bool {
        !self
            .entries
            .lock()
            .expect("import progress lock is not poisoned")
            .is_empty()
    }
}

pub(super) struct ImportRegistration {
    entries: Arc<Mutex<HashMap<String, ImportProgress>>>,
    import_id: String,
}

impl Drop for ImportRegistration {
    fn drop(&mut self) {
        self.entries
            .lock()
            .expect("import progress lock is not poisoned")
            .remove(&self.import_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_registered_imports_have_progress() {
        let tracker = ImportTracker::default();
        assert!(tracker.register("../invalid").is_err());
        let registration = tracker.register("import-1").unwrap();
        assert!(tracker.register("import-1").is_err());
        tracker.update("import-1", |progress| {
            progress.resource = "slides";
            progress.phase = "recognizing_text";
            progress.completed_items = Some(1);
            progress.total_items = Some(3);
        });
        let progress = tracker.get("import-1").unwrap();
        assert_eq!(progress.resource, "slides");
        assert_eq!(progress.completed_items, Some(1));
        assert!(tracker.has_active_import());
        drop(registration);
        assert!(tracker.get("import-1").is_none());
        assert!(!tracker.has_active_import());
    }
}
