//! Project-level column knowledge the binder and rules read. The maps
//! are lookup-only: any output built from them must sort first.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use crate::types::ColType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Completeness {
    /// Every column is known; "column does not exist" may be reported.
    Closed,
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemaCol {
    pub name: Arc<str>,
    pub ty: ColType,
    /// Backed by a `unique` test or constraint.
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelSchema {
    pub columns: Arc<[SchemaCol]>,
    pub completeness: Completeness,
    pub fingerprint: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectSchemas {
    pub revision: u64,
    pub models: HashMap<Arc<str>, Arc<ModelSchema>>,
    pub sources: HashMap<(Arc<str>, Arc<str>), Arc<ModelSchema>>,
}

impl ProjectSchemas {
    pub fn model(&self, name: &str) -> Option<&Arc<ModelSchema>> {
        self.models.get(name)
    }

    pub fn source(&self, source: &str, table: &str) -> Option<&Arc<ModelSchema>> {
        // boundary: the tuple key owns its parts, so a lookup builds one
        let key: (Arc<str>, Arc<str>) = (Arc::from(source), Arc::from(table));
        self.sources.get(&key)
    }
}
