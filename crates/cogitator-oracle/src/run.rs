//! The engine-neutral differential check. Both engine runs call [`run_case`],
//! so the meaning of P and N lives in one place.

use crate::claims::{check_claims, EngineReads};
use crate::corpus::{Case, Fixture, Role};

/// One relation's columns as `(name, type)`, in result order.
pub type Columns = Vec<(String, String)>;

/// The result of comparing two queries as bags of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BagReport {
    pub left_columns: Columns,
    pub right_columns: Columns,
    /// `(left EXCEPT ALL right, right EXCEPT ALL left)` row counts; `None`
    /// when the column counts differ.
    pub except_all: Option<(u64, u64)>,
}

impl BagReport {
    pub fn columns_match(&self) -> bool {
        self.left_columns == self.right_columns
    }

    /// Whether the two sides hold the same rows, duplicates and NULLs included.
    pub fn rows_match(&self) -> bool {
        self.except_all == Some((0, 0))
    }

    pub fn equal(&self) -> bool {
        self.columns_match() && self.rows_match()
    }
}

/// The two things the harness asks of an engine.
pub trait Engine {
    fn name(&self) -> &str;

    /// Compare `left` and `right` as bags after running `setup`.
    fn bag(&mut self, setup: &str, left: &str, right: &str) -> Result<BagReport, String>;

    /// The base-table columns the engine's plan for `query` reads.
    fn reads(&mut self, setup: &str, query: &str) -> Result<EngineReads, String>;
}

/// Run one case and return a message per failed expectation.
pub fn run_case<E: Engine>(engine: &mut E, case: &Case) -> Vec<String> {
    let mut failures = Vec::new();
    for fixture in &case.fixtures {
        failures.extend(check_fixture(engine, case, fixture));
    }
    failures
        .into_iter()
        .map(|f| format!("[{}] {}: {f}", engine.name(), case.name))
        .collect()
}

fn check_fixture<E: Engine>(engine: &mut E, case: &Case, fixture: &Fixture) -> Vec<String> {
    let setup = case.setup(fixture);
    let name = &fixture.name;
    let mut failures = Vec::new();
    match engine.bag(&setup, &case.model, &case.after) {
        Ok(report) => {
            if let Some(failure) = judge(fixture.role, &report) {
                failures.push(format!("fixture {name}: {failure}: {report:?}"));
            }
        }
        Err(e) => failures.push(format!("fixture {name}: engine error: {e}")),
    }
    if !case.plan.is_empty() {
        match engine.reads(&setup, &case.model) {
            Ok(reads) => failures.extend(
                check_claims(&reads, &case.plan)
                    .into_iter()
                    .map(|f| format!("fixture {name}: {f}")),
            ),
            Err(e) => failures.push(format!("fixture {name}: plan error: {e}")),
        }
    }
    failures
}

/// `None` when the report is what the role demands.
fn judge(role: Role, report: &BagReport) -> Option<&'static str> {
    match role {
        Role::Preserve if !report.equal() => Some("P fix is not bag-preserving"),
        Role::Precondition if !report.equal() => {
            Some("the precondition fixture differs, so the fix is not safe where it should be")
        }
        Role::Witness if !report.columns_match() => {
            Some("the witness differs in columns, not only in rows")
        }
        Role::Witness if report.rows_match() => {
            Some("the witness does not differ, so the N label is unproven")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(except_all: Option<(u64, u64)>, types: (&str, &str)) -> BagReport {
        BagReport {
            left_columns: vec![("a".to_owned(), types.0.to_owned())],
            right_columns: vec![("a".to_owned(), types.1.to_owned())],
            except_all,
        }
    }

    #[test]
    fn preserve_needs_empty_differences_in_both_directions() {
        let equal = report(Some((0, 0)), ("INT", "INT"));
        assert_eq!(judge(Role::Preserve, &equal), None);
        assert!(judge(Role::Preserve, &report(Some((1, 0)), ("INT", "INT"))).is_some());
        assert!(judge(Role::Preserve, &report(Some((0, 2)), ("INT", "INT"))).is_some());
        assert!(judge(Role::Preserve, &report(None, ("INT", "INT"))).is_some());
    }

    #[test]
    fn preserve_needs_matching_types() {
        let retyped = report(Some((0, 0)), ("INT", "BIGINT"));
        assert!(judge(Role::Preserve, &retyped).is_some());
    }

    #[test]
    fn a_witness_must_differ_in_rows_only() {
        assert_eq!(
            judge(Role::Witness, &report(Some((1, 0)), ("INT", "INT"))),
            None
        );
        assert!(judge(Role::Witness, &report(Some((0, 0)), ("INT", "INT"))).is_some());
        assert!(judge(Role::Witness, &report(Some((1, 0)), ("INT", "TEXT"))).is_some());
    }

    #[test]
    fn a_precondition_must_be_equal() {
        assert_eq!(
            judge(Role::Precondition, &report(Some((0, 0)), ("INT", "INT"))),
            None
        );
        assert!(judge(Role::Precondition, &report(Some((3, 0)), ("INT", "INT"))).is_some());
    }
}
