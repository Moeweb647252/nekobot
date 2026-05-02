//! Skill discovery: scan directories for `SKILL.md` files and parse their
//! YAML frontmatter.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Metadata extracted from a `SKILL.md` frontmatter — used for the catalog.
#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    /// Absolute path to the `SKILL.md` file.
    pub location: PathBuf,
}

/// A fully loaded skill including its instruction body.
#[derive(Debug, Clone)]
pub struct Skill {
    pub meta: SkillMeta,
    /// The markdown body after the YAML frontmatter.
    pub body: String,
    /// The skill's root directory (parent of SKILL.md).
    pub base_dir: PathBuf,
}

/// The parsed YAML frontmatter from a `SKILL.md` file.
#[derive(Debug, Clone, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
}

/// Scan `root_dir` for immediate subdirectories containing a `SKILL.md` file.
/// Returns metadata for all discovered skills (catalog / tier 1).
pub fn discover(root_dir: &Path) -> anyhow::Result<Vec<SkillMeta>> {
    let mut skills = Vec::new();

    let entries = match fs::read_dir(root_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(target: "skill", "skills directory not found: {}", root_dir.display());
            return Ok(Vec::new());
        }
        Err(e) => return Err(anyhow::anyhow!("failed to read skills dir {}: {e}", root_dir.display())),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }

        match parse_frontmatter(&skill_md) {
            Ok(fm) => {
                skills.push(SkillMeta {
                    name: fm.name,
                    description: fm.description,
                    location: skill_md,
                });
            }
            Err(e) => {
                tracing::warn!(target: "skill", "skipping {}: {e}", path.display());
            }
        }
    }

    // Deduplicate by name: first-found wins (project-level before user-level
    // is handled by scanning order in the caller).
    let mut seen = std::collections::HashSet::new();
    skills.retain(|s| seen.insert(s.name.clone()));

    Ok(skills)
}

/// Load a skill's complete content (frontmatter + body) given its `SKILL.md` path.
pub fn load(location: &Path) -> anyhow::Result<Skill> {
    let fm = parse_frontmatter(location)?;
    let content = fs::read_to_string(location)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", location.display()))?;
    let body = extract_body(&content);

    let base_dir = location
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no parent dir for {}", location.display()))?
        .to_path_buf();

    Ok(Skill {
        meta: SkillMeta {
            name: fm.name,
            description: fm.description,
            location: location.to_path_buf(),
        },
        body,
        base_dir,
    })
}

/// Parse the YAML frontmatter from a `SKILL.md` file. Returns `Frontmatter{name, description}`.
fn parse_frontmatter(path: &Path) -> anyhow::Result<Frontmatter> {
    let content = fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;

    let frontmatter_str = extract_frontmatter(&content)
        .ok_or_else(|| anyhow::anyhow!("{}: missing YAML frontmatter (--- delimiters)", path.display()))?;

    let fm: Frontmatter = serde_yml::from_str(&frontmatter_str)
        .map_err(|e| anyhow::anyhow!("{}: invalid YAML frontmatter: {e}", path.display()))?;

    // Validate required fields
    if fm.name.trim().is_empty() {
        anyhow::bail!("{}: 'name' field is empty", path.display());
    }
    if fm.description.trim().is_empty() {
        anyhow::bail!("{}: 'description' field is empty", path.display());
    }

    Ok(fm)
}

/// Extract the YAML frontmatter string between the first pair of `---` delimiters.
fn extract_frontmatter(content: &str) -> Option<String> {
    let mut lines = content.lines();
    // The first line must be `---`
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut yaml_lines = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            return Some(yaml_lines.join("\n"));
        }
        yaml_lines.push(line);
    }
    None
}

/// Extract the markdown body after the closing `---` delimiter.
fn extract_body(content: &str) -> String {
    let mut after_first = false;
    let mut in_frontmatter = false;
    let mut found_closing = false;
    let mut body_lines = Vec::new();

    for line in content.lines() {
        if !after_first && line.trim() == "---" {
            after_first = true;
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter && !found_closing && line.trim() == "---" {
            found_closing = true;
            in_frontmatter = false;
            continue;
        }
        if found_closing || !in_frontmatter {
            body_lines.push(line);
        }
    }
    body_lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str, description: &str, body: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let content = format!(
            "---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"
        );
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn discover_finds_skills() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "alpha", "First skill", "# Alpha\nInstructions here.");
        write_skill(dir.path(), "beta", "Second skill", "# Beta\nMore instructions.");
        // Not a skill (no SKILL.md)
        fs::create_dir_all(dir.path().join("not-a-skill")).unwrap();

        let skills = discover(dir.path()).unwrap();
        assert_eq!(skills.len(), 2);
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn discover_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = discover(dir.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn discover_missing_dir() {
        let skills = discover(Path::new("/nonexistent/skills/path")).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn discover_deduplicates_by_name() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "dup", "First", "");
        write_skill(dir.path(), "dup-2", "Second", "");
        // Manually create a duplicate name
        let dup_dir = dir.path().join("dup-2");
        let content = "---\nname: dup\ndescription: Duplicate name\n---\n\nbody\n";
        fs::write(dup_dir.join("SKILL.md"), content).unwrap();

        let skills = discover(dir.path()).unwrap();
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        // "dup" appears only once (first-found)
        assert_eq!(names.iter().filter(|n| **n == "dup").count(), 1);
    }

    #[test]
    fn load_returns_full_skill() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "test", "A test skill", "# Instructions\nDo the thing.");

        let skills = discover(dir.path()).unwrap();
        assert_eq!(skills.len(), 1);

        let skill = load(&skills[0].location).unwrap();
        assert_eq!(skill.meta.name, "test");
        assert_eq!(skill.meta.description, "A test skill");
        assert!(skill.body.contains("Do the thing."));
        assert!(skill.base_dir.ends_with("test"));
    }

    #[test]
    fn parse_rejects_missing_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("bad");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# No frontmatter here\n").unwrap();

        let skills = discover(dir.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn parse_rejects_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "bad", "", "");
        let skills = discover(dir.path()).unwrap();
        assert!(skills.is_empty());
    }
}
