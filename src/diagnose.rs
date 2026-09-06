//! Boot-time diagnostic pass over the parsed config.
//!
//! For every field the target accepts but has NOT wired end-to-end,
//! emit a `tracing::warn!` at boot naming the field and the intended
//! contract. Operators reading the boot log can then determine "am I
//! depending on anything the target doesn't implement?" without
//! reading source.
//!
//! Adding a new field to `AppConfig`? Ask yourself: is it wired
//! end-to-end? If yes, no entry needed here. If no, add a row to
//! `UNWIRED` below with the planned-release tag.

use crate::config::AppConfig;

/// Fields that are parsed and stored on `AppConfig` but not yet
/// consumed by any behaviour. When operators leave the value at
/// default, no warning fires (their config change was inert either
/// way); when they set a non-default value, the WARN tells them the
/// value has no effect and points at the divergence entry.
struct UnwiredField {
    name: &'static str,
    /// One-line description of what the field *would* control once
    /// wired. Included in the WARN so log-readers understand the
    /// impact without leaving the log.
    intended: &'static str,
    /// Roadmap pointer — release in which the field is expected to
    /// become active. Bare release tag; no internal issue ID.
    planned_release: &'static str,
}

// v0.1.0-alpha.3: `cors_origin` was moved from silent-drop-behind-WARN
// to a hard boot failure (see F6 in the v1 audit). If future fields
// land in the accepted-but-unwired state, add them here — the diagnose
// pass survives an empty table.
const UNWIRED: &[UnwiredField] = &[];

/// Walk the config, emit WARN for every non-default value on a field
/// that isn't wired.
pub fn diagnose(cfg: &AppConfig) {
    for f in UNWIRED {
        if is_nondefault(cfg, f.name) {
            tracing::warn!(
                field = f.name,
                intended = f.intended,
                planned_release = f.planned_release,
                "config field is accepted but has no effect in this build"
            );
        }
    }
}

/// Returns true if the operator has set the named field to something
/// other than the built-in default. Keeps the dispatch here so
/// `UNWIRED` stays a plain data table.
fn is_nondefault(_cfg: &AppConfig, field: &str) -> bool {
    // If this panics, `UNWIRED` names a field this function doesn't
    // know how to check — that's a bug in *this* module. Better to
    // fail loudly at boot than to silently skip diagnostics.
    panic!("diagnose: unwired-field dispatch missing case for {field:?}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn diagnose_default_config_emits_nothing() {
        // Should be a no-op — no warnings, no panics.
        diagnose(&AppConfig::default());
    }

    #[test]
    fn diagnose_empty_unwired_table_is_a_noop() {
        // F6 moved cors_origin out of UNWIRED into a hard boot failure.
        // The table is now empty; make sure diagnose still doesn't panic
        // when handed a fully-populated config.
        let cfg = AppConfig {
            cors_origin: "https://example.com".into(),
            ..AppConfig::default()
        };
        diagnose(&cfg);
    }
}
