use super::*;
use tempfile::TempDir;

fn file(content: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("topic.md");
    std::fs::write(&path, content).unwrap();
    (temp, path)
}

fn section(heading: &str, level: u8, start: usize, end: usize) -> Section {
    Section {
        heading: heading.to_owned(),
        level,
        start: start as u64,
        end: end as u64,
    }
}

#[test]
fn outline_nests_sections_and_ignores_headings_in_code_fences() {
    let content = "# Title\nintro\n## Setup\nsteps\n```\n# not a heading\n```\n### Linux\napt\n## Usage\nrun\n";
    let (_temp, path) = file(content);
    let setup = content.find("## Setup").unwrap();
    let linux = content.find("### Linux").unwrap();
    let usage = content.find("## Usage").unwrap();

    let (sections, is_truncated) = outline(&path, &BatchDreamControl::default(), 10).unwrap();

    assert_eq!(
        vec![
            section("# Title", 1, 0, content.len()),
            section("## Setup", 2, setup, usage),
            section("### Linux", 3, linux, usage),
            section("## Usage", 2, usage, content.len()),
        ],
        sections
    );
    assert!(!is_truncated);
}
