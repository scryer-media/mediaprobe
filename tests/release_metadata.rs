//! The release workflow refuses a version without a dated changelog section.
//! This fails the same way before a pull request is opened.

#[test]
fn changelog_has_a_dated_section_for_the_manifest_version() {
    let changelog = include_str!("../CHANGELOG.md");
    let version = env!("CARGO_PKG_VERSION");
    let prefix = format!("## {version} - ");
    let heading = changelog
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("CHANGELOG.md has no `{prefix}<date>` heading"));
    let date = &heading[prefix.len()..];
    let bytes = date.as_bytes();
    assert!(
        bytes.len() == 10
            && bytes.iter().enumerate().all(|(i, b)| match i {
                4 | 7 => *b == b'-',
                _ => b.is_ascii_digit(),
            }),
        "`{heading}` is not dated YYYY-MM-DD"
    );
}
