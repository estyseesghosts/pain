// Release version formatting shared by the build script and the CLI.
//
// The format is `vYY-MM-xxxxxxx`: the last two digits of the commit year,
// the two-digit commit month, and Git's short commit hash. Chaus uses the
// same tag shape, so a release tag and `--version` name the same source.

/// Format a release version from a commit date (`YYYY-MM-DD`) and short hash.
#[must_use]
pub fn release_version(commit_date: &str, short_hash: &str) -> String {
    let mut parts = commit_date.split('-');
    let year = parts.next().unwrap_or("00");
    let month = parts.next().unwrap_or("00");
    let short_year = if year.is_empty() {
        "00"
    } else {
        &year[year.len().saturating_sub(2)..]
    };
    format!("v{short_year}-{month:0>2}-{short_hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_version_matches_chaus_tag_shape() {
        assert_eq!(release_version("2026-11-25", "0566384"), "v26-11-0566384");
        assert_eq!(release_version("2026-01-02", "d888c5f"), "v26-01-d888c5f");
    }

    #[test]
    fn release_version_falls_back_for_unexpected_dates() {
        assert_eq!(release_version("", "abcdef0"), "v00-00-abcdef0");
        assert_eq!(release_version("2026", "abcdef0"), "v26-00-abcdef0");
    }
}
