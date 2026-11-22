//! A model's direct upstreams, the environment it binds against and the
//! digest that keys its cached result.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use cogitator_ir::schema::{Completeness, ProjectSchemas};
use cogitator_sql_front::SchemaEnv;

use super::JobInput;
use crate::context::types::ModelDetails;
use crate::graph::{NodeKind, ProjectGraph};

pub(super) struct Upstream<'g> {
    models: Vec<&'g str>,
    sources: Vec<&'g str>,
    /// An upstream sorts at or after this model: a dependency cycle.
    pub(super) cycle: bool,
}

impl<'g> Upstream<'g> {
    pub(super) fn of(
        graph: &'g ProjectGraph,
        model: &str,
        depths: &HashMap<&str, usize>,
    ) -> Upstream<'g> {
        let own = depths.get(model).copied();
        let mut up = Upstream {
            models: Vec::new(),
            sources: Vec::new(),
            cycle: false,
        };
        for node in graph.model_neighbors(model).0 {
            match node.kind {
                NodeKind::Model => {
                    let deeper = own
                        .is_none_or(|d| depths.get(node.name.as_str()).is_none_or(|up| *up >= d));
                    up.cycle |= node.name == model || deeper;
                    up.models.push(node.name.as_str());
                }
                NodeKind::Source => up.sources.push(node.name.as_str()),
                _ => {}
            }
        }
        up
    }

    /// An upstream model with neither a schema nor a file behind it.
    pub(super) fn missing(
        &self,
        schemas: &ProjectSchemas,
        models: &HashMap<String, ModelDetails>,
    ) -> Option<String> {
        self.models
            .iter()
            .find(|m| schemas.model(m).is_none() && !models.contains_key(**m))
            .map(|m| m.to_string())
    }

    /// True when this model's columns rest on a catalog snapshot, directly
    /// or through an upstream model.
    pub(super) fn catalog_backed(
        &self,
        schemas: &ProjectSchemas,
        backed: &HashSet<Arc<str>>,
        input: &JobInput<'_>,
    ) -> bool {
        self.models.iter().any(|m| backed.contains(*m))
            || self.sources.iter().any(|source| {
                input.sources.get(*source).is_some_and(|details| {
                    details.tables.keys().any(|table| {
                        schemas
                            .source(source, table)
                            .is_some_and(|s| s.completeness == Completeness::Closed)
                    })
                })
            })
    }

    /// The upstream schemas this model binds against, and their digest.
    pub(super) fn env_for(
        &self,
        model: &str,
        schemas: &ProjectSchemas,
        input: &JobInput<'_>,
    ) -> (SchemaEnv, u64) {
        let mut hasher = DefaultHasher::new();
        input.dialect.as_str().hash(&mut hasher);
        let mut env = ProjectSchemas::default();
        for name in &self.models {
            if let Some(schema) = schemas.models.get(*name) {
                name.hash(&mut hasher);
                schema.fingerprint.hash(&mut hasher);
                env.models.insert(Arc::from(*name), schema.clone());
            }
        }
        for source in &self.sources {
            let Some(details) = input.sources.get(*source) else {
                continue;
            };
            let mut tables: Vec<&str> = details.tables.keys().map(String::as_str).collect();
            tables.sort_unstable(); // the digest must not depend on map order
            for table in tables {
                if let Some(schema) = schemas.source(source, table) {
                    (source, table).hash(&mut hasher);
                    schema.fingerprint.hash(&mut hasher);
                    env.sources
                        .insert((Arc::from(*source), Arc::from(table)), schema.clone());
                }
            }
        }
        let env = SchemaEnv {
            schemas: Arc::new(env),
            model: Some(Arc::from(model)),
        };
        (env, hasher.finish())
    }
}
