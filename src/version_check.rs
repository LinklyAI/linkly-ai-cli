//! Detect when the Desktop app on the other end of the MCP connection is
//! older than what this CLI was built for.
//!
//! Without this check, calling a brand-new tool like `find_paths` against
//! a Desktop that doesn't ship it yet surfaces a generic
//! `Tool error: tool not found`, and (worse) sending unknown parameters
//! like `--modified-after` to an old Desktop is a silent no-op — the
//! `serde(deny_unknown_fields)` guard on the Desktop side closes that
//! second hole, but the user still ends up with a confusing error
//! instead of "your Desktop is out of date".
//!
//! The check is best-effort: if the Desktop reports a non-SemVer
//! `version` string (dev build, custom fork, …) we stay silent rather
//! than risk a false alarm.

use semver::Version;

/// Lowest Desktop version that exposes every tool / parameter this CLI
/// release knows about. Bump this whenever the CLI starts depending on
/// new MCP surface area (new tool, new field, new response semantics).
///
/// We use a `-beta.0` sentinel rather than the bare version so beta
/// builds in the same release line are accepted: SemVer treats
/// `X.Y.Z-beta.N` as less than `X.Y.Z`, but greater than `X.Y.Z-beta.0`,
/// so any beta of the targeted release passes. Desktop's release script
/// never emits `-beta.0` (numbering starts at `-beta.1`), making this a
/// safe synthetic floor.
pub const MIN_DESKTOP_VERSION_FOR_FULL_FEATURES: &str = "0.10.1-beta.0";

/// Plain-English description of what the CLI started using at the
/// version above. Surfaced verbatim in the warning so the user
/// understands what they're missing.
///
/// 0.10.1 is also the release where `note_save.tags` switched from
/// full-replacement to additive (body #tokens became the source of
/// truth) — gating on it keeps this CLI's additive `--tags` from ever
/// reaching a full-replacement Desktop, where a partial tag list would
/// silently delete the tags it omits.
pub const FEATURES_REQUIRING_MIN_DESKTOP: &str =
    "list --scope folder/library (with --path, --library, --type and --modified-after/-before), \
     note-save --app-name, and note-save's additive --tags model";

/// What we found when comparing the Desktop version against the floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionGap {
    /// The Desktop version string verbatim (e.g. `"0.4.0"`).
    pub actual: String,
    /// The minimum Desktop version this CLI requires for all features.
    /// Stored as a stable, user-visible label rather than the raw
    /// `-beta.0` sentinel.
    pub required: &'static str,
    /// Human-readable list of the features that aren't available
    /// against `actual` but are available at `required`.
    pub missing_features: &'static str,
}

/// Compare a Desktop `version` string against this CLI's floor.
///
/// Returns `Ok(())` when the Desktop is new enough OR the version
/// string isn't valid SemVer (we'd rather stay silent than emit a
/// false positive against dev builds). Returns `Err(VersionGap)` only
/// when the Desktop is provably behind.
pub fn check_desktop_version(version: &str) -> Result<(), VersionGap> {
    let actual = match Version::parse(version) {
        Ok(v) => v,
        Err(_) => return Ok(()), // Non-SemVer (dev build / unknown) → silent
    };
    // The min-version constant is a hard-coded constant we control —
    // parse failure here is a programmer error, not a runtime concern.
    let required = Version::parse(MIN_DESKTOP_VERSION_FOR_FULL_FEATURES)
        .expect("MIN_DESKTOP_VERSION_FOR_FULL_FEATURES is a valid SemVer literal");

    if actual < required {
        // Strip the synthetic `-beta.0` floor so users see the public
        // version label (e.g. `0.10.0`) instead of the internal sentinel.
        let required_label = MIN_DESKTOP_VERSION_FOR_FULL_FEATURES.trim_end_matches("-beta.0");
        Err(VersionGap {
            actual: version.to_string(),
            required: required_label,
            missing_features: FEATURES_REQUIRING_MIN_DESKTOP,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The floor label, hardcoded on purpose.
    ///
    /// Deriving it from `MIN_DESKTOP_VERSION_FOR_FULL_FEATURES` would make
    /// these assertions vacuous — test and production would compute the same
    /// (possibly wrong) string from the same source and always agree. Spelling
    /// it out means bumping the constant turns these tests red, which is the
    /// reminder to also update the feature description below.
    const EXPECTED_FLOOR: &str = "0.10.1";

    #[test]
    fn floor_label_matches_the_constant() {
        // The one place the two are compared, so a malformed constant (missing
        // or misspelled `-beta.0` sentinel) is caught here rather than silently
        // weakening every other assertion.
        assert_eq!(
            MIN_DESKTOP_VERSION_FOR_FULL_FEATURES,
            format!("{}-beta.0", EXPECTED_FLOOR)
        );
    }

    #[test]
    fn old_stable_desktop_triggers_warning() {
        let gap = check_desktop_version("0.4.0").unwrap_err();
        assert_eq!(gap.actual, "0.4.0");
        assert_eq!(gap.required, EXPECTED_FLOOR);
        // Assert on the *content* of the message, not on "the field equals the
        // constant that was assigned to it" — the bug this whole check exists
        // to prevent is a bumped version paired with a stale feature list.
        assert!(gap.missing_features.contains("folder/library"));
        assert!(gap.missing_features.contains("--app-name"));
        assert!(gap.missing_features.contains("additive --tags"));
        assert!(
            !gap.missing_features.contains("find_paths"),
            "feature list still advertises find_paths, which shipped long before the current floor"
        );
        assert!(
            !gap.missing_features.contains("image-text"),
            "feature list still advertises read --image-text, which shipped at 0.10.0, before the current floor"
        );
    }

    #[test]
    fn desktop_at_or_above_the_floor_passes() {
        assert!(check_desktop_version(EXPECTED_FLOOR).is_ok());
        assert!(check_desktop_version("99.0.0").is_ok());
    }

    #[test]
    fn beta_in_target_release_line_passes() {
        // X.Y.Z-beta.1 > X.Y.Z-beta.0 (the sentinel) so a beta user on the
        // targeted release line is treated as up to date — they already have
        // the new tools.
        assert!(check_desktop_version("0.10.1-beta.1").is_ok());
        assert!(check_desktop_version("0.10.1-beta.6").is_ok());
    }

    #[test]
    fn old_beta_desktop_triggers_warning() {
        // Beta builds of older release lines should still warn — they don't
        // ship the new surface area.
        let gap = check_desktop_version("0.3.6-beta.6").unwrap_err();
        assert_eq!(gap.actual, "0.3.6-beta.6");
    }

    #[test]
    fn releases_below_the_floor_warn() {
        assert!(check_desktop_version("0.3.5").is_err());
        assert!(check_desktop_version("0.1.0").is_err());
        // The three-scope list / app_name / additive-tags surface landed in
        // desktop 0.10.1. 0.10.0 has the note tools but the OLD
        // full-replacement `tags` semantics — letting it through would turn
        // this CLI's additive `--tags` into a silent tag wipe, so it must
        // warn (and its betas too: 0.10.0-beta.N < 0.10.1-beta.0).
        assert!(check_desktop_version("0.10.0").is_err());
        assert!(check_desktop_version("0.10.0-beta.6").is_err());
        assert!(check_desktop_version("0.9.1").is_err());
    }

    #[test]
    fn non_semver_version_is_silent() {
        // dev builds, git hashes, anything we can't parse as SemVer
        // gets the benefit of the doubt — no warning.
        assert!(check_desktop_version("dev").is_ok());
        assert!(check_desktop_version("abc123").is_ok());
        assert!(check_desktop_version("").is_ok());
    }
}
