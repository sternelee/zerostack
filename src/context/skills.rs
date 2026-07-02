//! Skills system — discovers and loads skill directories (per agentskills.io spec).
//!
//! Skills are directories containing a `SKILL.md` file with YAML frontmatter.
//! They provide specialized instructions that the agent loads on demand via `/skill:name`.
//!
//! ## Skill format (SKILL.md)
//!
//! ```markdown
//! ---
//! name: my-skill
//! description: What this skill does and when to use it
//! license: MIT
//! compatibility: requires jq
//! allowed-tools: bash read grep
//! ---
//!
//! # My Skill
//!
//! Instructions here...
//! ```
//!
//! ## Discovery locations
//!
//! - Global:  `~/.local/share/zerostack/skills/`
//! - Project: `.zerostack/skills/`
//! - Embedded: `data/skills/` (bundled at compile time)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};
use serde::Deserialize;

// ── Embedded skills ─────────────────────────────────────────

static EMBEDDED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/data/skills");

// ── Skill metadata ──────────────────────────────────────────

/// Parsed frontmatter from a SKILL.md file.
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct SkillFrontmatter {
    /// Skill name (1-64 chars, lowercase, hyphens).
    pub name: String,
    /// Description: when to use this skill.
    pub description: String,
    /// Optional SPDX license identifier.
    #[serde(default)]
    pub license: Option<String>,
    /// Environment requirements.
    #[serde(default)]
    pub compatibility: Option<String>,
    /// Space-delimited list of pre-approved tools.
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
    /// Arbitrary key-value metadata.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
    /// When true, skill is hidden from system prompt.
    #[serde(default, rename = "disable-model-invocation")]
    pub disable_model_invocation: Option<bool>,
}

/// A fully loaded skill.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Parsed frontmatter.
    pub meta: SkillFrontmatter,
    /// Full directory path (for resolving relative assets).
    pub dir: PathBuf,
    /// Full SKILL.md content (after frontmatter).
    pub content: String,
}

// ── Frontmatter parsing ─────────────────────────────────────

/// Parse YAML frontmatter from a SKILL.md file.
/// Returns (frontmatter, body) on success.
pub fn parse_frontmatter(raw: &str) -> Result<(SkillFrontmatter, String), String> {
    let content = raw.trim();

    // Frontmatter must start with "---" on the first line.
    if !content.starts_with("---") {
        return Err("missing frontmatter delimiter '---'".to_string());
    }

    let after_first = &content[3..]; // skip opening "---"
    let end = after_first
        .find("\n---")
        .or_else(|| after_first.find("\r\n---"))
        .ok_or_else(|| "missing closing frontmatter delimiter '---'".to_string())?;

    let fm_text = after_first[..end].trim();
    let body = after_first[end + 4..].trim().to_string(); // skip "\n---" (4 chars)

    let fm: SkillFrontmatter =
        serde_yaml_ng::from_str(fm_text).map_err(|e| format!("invalid frontmatter: {e}"))?;

    // Validate required fields.
    if fm.name.is_empty() {
        return Err("skill name is required".to_string());
    }
    if fm.description.is_empty() {
        return Err("skill description is required".to_string());
    }

    // Validate name format (best-effort, warn only).
    if !is_valid_skill_name(&fm.name) {
        tracing::warn!(
            "skill name '{}' does not match recommended format (lowercase, hyphens)",
            fm.name
        );
    }

    Ok((fm, body))
}

fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

// ── Skill discovery ─────────────────────────────────────────

/// Scan a directory for skill directories (subdirectories containing SKILL.md).
pub fn discover_skills(base_dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if !base_dir.exists() {
        return results;
    }
    discover_recursive(base_dir, &mut results);
    results
}

fn discover_recursive(dir: &Path, results: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").exists() {
            results.push(path.clone());
        }
        // Recurse into subdirectories.
        discover_recursive(&path, results);
    }
}

/// Load a skill from a directory containing SKILL.md.
pub fn load_skill(dir: &Path) -> Result<Skill, String> {
    let skill_path = dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&skill_path)
        .map_err(|e| format!("failed to read {skill_path:?}: {e}"))?;

    let (meta, content) = parse_frontmatter(&raw)?;

    Ok(Skill {
        meta,
        dir: dir.to_path_buf(),
        content,
    })
}

// ── Collection loading ──────────────────────────────────────

/// Load all skills from standard locations.
/// Returns a map of skill name → Skill.
pub fn load_all() -> HashMap<String, Skill> {
    let mut skills = HashMap::new();

    // 1. Embedded skills (lowest priority).
    for dir in EMBEDDED_SKILLS.dirs() {
        if let Some(name) = dir.path().file_name().and_then(|s| s.to_str()) {
            let skill = load_embedded_skill(dir, name);
            if let Some(s) = skill {
                skills.entry(s.meta.name.clone()).or_insert(s);
            }
        }
    }

    // 2. Global skills.
    if let Some(data_dir) = dirs::data_dir() {
        load_skills_from_dir(&data_dir.join("zerostack").join("skills"), &mut skills);
    }

    // 3. Project-local skills.
    if let Ok(cwd) = std::env::current_dir() {
        load_skills_from_dir(&cwd.join(".zerostack").join("skills"), &mut skills);
    }

    skills
}

fn load_skills_from_dir(dir: &Path, skills: &mut HashMap<String, Skill>) {
    for skill_dir in discover_skills(dir) {
        match load_skill(&skill_dir) {
            Ok(skill) => {
                let name = skill.meta.name.clone();
                if skills.contains_key(&name) {
                    tracing::warn!("skill '{name}' from {skill_dir:?} overrides existing");
                }
                skills.insert(name, skill);
            }
            Err(e) => {
                tracing::warn!("failed to load skill from {skill_dir:?}: {e}");
            }
        }
    }
}

fn load_embedded_skill(dir: &Dir, name: &str) -> Option<Skill> {
    let file = dir.get_file("SKILL.md")?;
    let raw = file.contents_utf8()?;
    let (meta, content) = parse_frontmatter(raw).ok()?;

    // Embedded skills don't have a real dir path, use a virtual path.
    Some(Skill {
        meta,
        dir: PathBuf::from(format!("<embedded>/skills/{name}")),
        content,
    })
}

/// Format skills as XML for the system prompt (per agentskills.io spec).
pub fn format_skills_xml(skills: &HashMap<String, Skill>) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut xml = String::from("<available_skills>\n");
    for skill in skills.values() {
        // Only list skills that allow model invocation.
        if skill.meta.disable_model_invocation.unwrap_or(false) {
            continue;
        }
        let desc = skill.meta.description.replace('\n', " ");
        xml.push_str(&format!("  <skill>\n"));
        xml.push_str(&format!("    <name>{}</name>\n", skill.meta.name));
        xml.push_str(&format!("    <description>{desc}</description>\n"));
        if let Some(ref license) = skill.meta.license {
            xml.push_str(&format!("    <license>{license}</license>\n"));
        }
        xml.push_str(&format!(
            "    <location>{}</location>\n",
            skill.dir.display()
        ));
        xml.push_str("  </skill>\n");
    }
    xml.push_str("</available_skills>\n");

    xml.push_str("\nUse `/skill:<name>` to load a skill's full instructions, or use `read` on the skill's SKILL.md path.\n");

    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_frontmatter() {
        let raw = "---\nname: test-skill\ndescription: A test skill\n---\n\n# Hello\n\nDo stuff.\n";
        let (fm, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(fm.name, "test-skill");
        assert_eq!(fm.description, "A test skill");
        assert_eq!(body, "# Hello\n\nDo stuff.");
    }

    #[test]
    fn test_parse_full_frontmatter() {
        let raw = r#"---
name: pdf-tools
description: Extract text and tables from PDF files.
license: MIT
compatibility: requires pdftotext
allowed-tools: bash read
metadata:
  version: "1.0"
---
# PDF Tools
Instructions...
"#;
        let (fm, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(fm.name, "pdf-tools");
        assert_eq!(fm.license, Some("MIT".into()));
        assert_eq!(fm.compatibility, Some("requires pdftotext".into()));
        assert_eq!(fm.allowed_tools, Some("bash read".into()));
        assert!(body.contains("Instructions..."));
    }

    #[test]
    fn test_missing_frontmatter() {
        let raw = "# No frontmatter\n";
        assert!(parse_frontmatter(raw).is_err());
    }

    #[test]
    fn test_missing_name() {
        let raw = "---\ndescription: test\n---\n\nBody\n";
        assert!(parse_frontmatter(raw).is_err());
    }

    #[test]
    fn test_missing_description() {
        let raw = "---\nname: test\n---\n\nBody\n";
        assert!(parse_frontmatter(raw).is_err());
    }

    #[test]
    fn test_valid_skill_names() {
        assert!(is_valid_skill_name("pdf-processing"));
        assert!(is_valid_skill_name("data-analysis2"));
        assert!(!is_valid_skill_name("PDF-Processing"));
        assert!(!is_valid_skill_name("-bad"));
        assert!(!is_valid_skill_name("bad--name"));
        assert!(!is_valid_skill_name(
            "very-long-name-that-exceeds-the-sixty-four-character-limit-xxxxxxxx"
        ));
    }
}
