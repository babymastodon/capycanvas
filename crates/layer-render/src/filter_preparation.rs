use layer_core::{Document, EffectProgram, authored::{OccurrenceHandle, SceneSnapshot}};
use std::{any::Any, fmt::Debug, sync::Arc};

pub trait PreparationKey: Any + Debug + Send + Sync {
    fn matches(&self, document: &Document) -> bool;
    fn equivalent(&self, other: &dyn PreparationKey) -> bool;
}
impl PartialEq for dyn PreparationKey {
    fn eq(&self, other: &Self) -> bool { self.equivalent(other) }
}

#[derive(Clone, Debug)]
pub struct FilterPreparation {
    pub program: Arc<EffectProgram>,
    pub scene: Option<(Arc<SceneSnapshot>, OccurrenceHandle)>,
    pub key: Option<Arc<dyn PreparationKey>>,
}
impl FilterPreparation {
    pub fn new(key: Option<Arc<dyn PreparationKey>>, program: Arc<EffectProgram>, scene: Option<(Arc<SceneSnapshot>, OccurrenceHandle)>) -> Self {
        Self { program, scene, key }
    }
    pub fn matches(&self, document: &Document) -> bool { self.key.as_ref().is_none_or(|key| key.matches(document)) }
}
impl PartialEq for FilterPreparation {
    fn eq(&self, other: &Self) -> bool { self.program == other.program && self.key == other.key }
}
