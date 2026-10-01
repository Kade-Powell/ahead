use pretty_assertions::assert_eq;
use std::{fs, path::Path};

use super::ParsedSkillFrontmatter;
use super::parse_skill_frontmatter_metadata;

#[test]
fn parses_repairs_and_sanitizes_frontmatter() {
    let parsed = parse_skill_frontmatter_metadata(
        "---\nname: deploy-service\ndescription: Build for AWS: ECS\ndisable-model-invocation: true\nmetadata:\n  short-description:  Deploy   safely\n---\n",
        "deploy-service",
    )
    .expect("valid frontmatter");

    assert_eq!(
        parsed,
        ParsedSkillFrontmatter {
            name: "deploy-service".to_string(),
            description: "Build for AWS: ECS".to_string(),
            short_description: Some("Deploy safely".to_string()),
            disable_model_invocation: true,
        }
    );
}

#[test]
fn requires_name_and_description() {
    let missing_name =
        parse_skill_frontmatter_metadata("---\ndescription: Demo skill\n---\n", "demo")
            .expect_err("name should be required");
    assert_eq!(missing_name.to_string(), "missing field `name`");

    let missing_description = parse_skill_frontmatter_metadata("---\nname: demo\n---\n", "demo")
        .expect_err("description should be required");
    assert_eq!(
        missing_description.to_string(),
        "missing field `description`"
    );
}

#[test]
fn repository_and_bundled_skills_follow_the_agentskills_format() {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    for skills_root in [".agents/skills", "ahead-agent/skills"] {
        let skills_root = repository_root.join(skills_root);
        for entry in fs::read_dir(&skills_root)
            .unwrap_or_else(|error| panic!("read {}: {error}", skills_root.display()))
        {
            let entry = entry.expect("read skill directory entry");
            if !entry.file_type().expect("read skill entry type").is_dir() {
                continue;
            }
            let skill_path = entry.path().join("SKILL.md");
            if !skill_path.is_file() {
                continue;
            }
            let directory_name = entry.file_name();
            let directory_name = directory_name
                .to_str()
                .expect("skill directory name is UTF-8");
            let contents = fs::read_to_string(&skill_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", skill_path.display()));
            parse_skill_frontmatter_metadata(&contents, directory_name)
                .unwrap_or_else(|error| panic!("{}: {error}", skill_path.display()));
        }
    }
}

#[test]
fn validates_name_characters_and_parent_directory() {
    for (name, directory_name) in [
        ("Demo", "Demo"),
        ("-demo", "-demo"),
        ("demo-", "demo-"),
        ("demo--name", "demo--name"),
        ("demo_name", "demo_name"),
        ("demo", "other"),
    ] {
        let contents = format!("---\nname: {name}\ndescription: Demo skill\n---\n");
        assert!(matches!(
            parse_skill_frontmatter_metadata(&contents, directory_name),
            Err(super::SkillParseError::InvalidField { field: "name", .. })
        ));
    }
}

#[test]
fn accepts_unicode_lowercase_letters_and_digits_in_names() {
    for name in ["démø", "παράδειγμα", "skill-٢"] {
        let contents = format!("---\nname: {name}\ndescription: Demo skill\n---\n");
        let parsed = parse_skill_frontmatter_metadata(&contents, name)
            .expect("Unicode lowercase alphanumerics are valid skill-name characters");
        assert_eq!(parsed.name, name);
    }
}

#[test]
fn rejects_names_and_descriptions_over_the_spec_limits() {
    let name = "a".repeat(65);
    let contents = format!("---\nname: {name}\ndescription: Demo skill\n---\n");
    assert!(matches!(
        parse_skill_frontmatter_metadata(&contents, &name),
        Err(super::SkillParseError::InvalidField { field: "name", .. })
    ));

    let description = "x".repeat(1025);
    let contents = format!("---\nname: demo\ndescription: {description}\n---\n");
    assert!(matches!(
        parse_skill_frontmatter_metadata(&contents, "demo"),
        Err(super::SkillParseError::InvalidField {
            field: "description",
            ..
        })
    ));
}

#[test]
fn repairs_short_descriptions_containing_colons_and_apostrophes() {
    let parsed = parse_skill_frontmatter_metadata(
        "---\nname: short\ndescription: Short skill\nmetadata:\n  short-description: What's included: builds and tests\n---\n",
        "short",
    )
    .expect("frontmatter with an unquoted short description should be repaired");

    assert_eq!(
        parsed,
        ParsedSkillFrontmatter {
            name: "short".to_string(),
            description: "Short skill".to_string(),
            short_description: Some("What's included: builds and tests".to_string()),
            disable_model_invocation: false,
        }
    );
}

#[test]
fn repairs_unrecognized_frontmatter_fields_that_need_quotes() {
    let parsed = parse_skill_frontmatter_metadata(
        "---\nname: unknown\ndescription: Unknown fields\nargument-hint: <duration: e.g. 7d, 2w>\ntags: [next,@supabase/ssr]\n---\n",
        "unknown",
    )
    .expect("frontmatter with unrecognized fields should be repaired");

    assert_eq!(
        parsed,
        ParsedSkillFrontmatter {
            name: "unknown".to_string(),
            description: "Unknown fields".to_string(),
            short_description: None,
            disable_model_invocation: false,
        }
    );
}

#[test]
fn preserves_block_scalar_bodies_while_repairing_other_fields() {
    let parsed = parse_skill_frontmatter_metadata(
        "---\nname: block\ndescription: |-\n  Build for AWS: ECS\nargument-hint: <duration: e.g. 7d>\n---\n",
        "block",
    )
    .expect("frontmatter repair should preserve block scalar bodies");

    assert_eq!(
        parsed,
        ParsedSkillFrontmatter {
            name: "block".to_string(),
            description: "Build for AWS: ECS".to_string(),
            short_description: None,
            disable_model_invocation: false,
        }
    );
}

#[test]
fn rejects_overlong_descriptions_and_preserves_short_description_metadata() {
    let description = "💡".repeat(/*n*/ 1_025);
    let short_description = "x".repeat(/*n*/ 1_025);
    let overlong_description = parse_skill_frontmatter_metadata(
        &format!("---\nname: long\ndescription: {description}\n---\n"),
        "long",
    )
    .expect_err("descriptions above the standard limit must be rejected");
    assert!(matches!(
        overlong_description,
        super::SkillParseError::InvalidField {
            field: "description",
            ..
        }
    ));

    let parsed = parse_skill_frontmatter_metadata(
        &format!(
            "---\nname: long\ndescription: Valid description\nmetadata:\n  short-description: {short_description}\n---\n"
        ),
        "long",
    )
    .expect("AHEAD short-description metadata remains an extension");
    assert_eq!(parsed.short_description, Some(short_description));
}
