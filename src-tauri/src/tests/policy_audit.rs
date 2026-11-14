//! Shared field-coverage audit for the two session policies.
//!
//! Both pipelines snapshot a slice of `AppConfig` into a session policy
//! (`SessionPolicy` for the classic pipeline, `RecordOnlyPolicy` for
//! the record-only one). Without an audit, a new `AppConfig` field
//! silently reaches one pipeline and not the other.
//!
//! The helper enumerates `AppConfig`'s **serde keys** — a hand-maintained
//! field list would silently miss new fields — and asserts that every
//! key is either covered by the policy's snapshot fields or listed as an
//! intentional exclusion.

use crate::config::AppConfig;
use std::collections::HashSet;

/// Assert that every `AppConfig` field is accounted for by a session
/// policy.
///
/// * `covered` — fields the policy snapshots (directly or through a
///   helper such as `SaveConfig::from_app_config` / `pipeline_mode()`),
/// * `excluded` — fields the policy intentionally does not consume; each
///   one is a documented decision, not an omission.
///
/// A field in neither list fails the audit: the author of a new
/// `AppConfig` field must decide, per pipeline, whether it is snapshotted
/// or deliberately left out.
pub(crate) fn assert_policy_covers_all_appconfig_fields(
    policy_name: &str,
    covered: &[&str],
    excluded: &[&str],
) {
    let cfg = AppConfig::default();
    let json = match serde_json::to_value(&cfg) {
        Ok(v) => v,
        Err(e) => panic!("AppConfig must serialize: {e}"),
    };
    let obj = match json.as_object() {
        Some(o) => o,
        None => panic!("AppConfig must serialize to a JSON object"),
    };
    let all_fields: HashSet<&str> = obj.keys().map(|s| s.as_str()).collect();
    let covered_set: HashSet<&str> = covered.iter().copied().collect();
    let excluded_set: HashSet<&str> = excluded.iter().copied().collect();

    for field in all_fields {
        assert!(
            covered_set.contains(field) || excluded_set.contains(field),
            "AppConfig field '{field}' is not accounted for by {policy_name} — snapshot it in \
             the policy, or list it as an intentional exclusion"
        );
    }
}

/// Both policies' exclusion lists must contain every entry of the shared
/// base table `NON_PIPELINE_FIELDS` (`commands/mod.rs`).
///
/// With the `unsnapshoted_fields!` macro this is structural — the macro
/// prepends the base to every expansion — but the assertion pins the
/// contract if a list is later de-macroed into a hand-written literal:
/// dropping a base entry from either policy's list then fails here
/// instead of silently narrowing that policy's field-coverage audit.
#[test]
fn policy_exclusion_lists_cover_non_pipeline_base() {
    let classic = crate::commands::recording_session::KNOWN_UNSNAPSHOTED_FIELDS;
    let record_only = crate::commands::record_only_session::RECORD_ONLY_KNOWN_UNSNAPSHOTED_FIELDS;
    for base in crate::commands::NON_PIPELINE_FIELDS {
        assert!(
            classic.contains(base),
            "KNOWN_UNSNAPSHOTED_FIELDS (SessionPolicy) dropped base entry '{base}' from \
             NON_PIPELINE_FIELDS"
        );
        assert!(
            record_only.contains(base),
            "RECORD_ONLY_KNOWN_UNSNAPSHOTED_FIELDS (RecordOnlyPolicy) dropped base entry \
             '{base}' from NON_PIPELINE_FIELDS"
        );
    }
}
