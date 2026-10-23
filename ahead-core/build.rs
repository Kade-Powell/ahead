use std::{env, fs, path::Path, process::Command};

use anyhow::Result;

#[derive(Debug)]
struct ReleaseInfo {
    version: String,
    branch: String,
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DISTRIBUTION");
    println!("cargo:rerun-if-env-changed=RELEASE_TAG_NAME");

    let release_info = get_info();
    // Print info to terminal during compilation
    println!("cargo::warning=Compiling meta: {release_info:?}");

    let meta_file = Path::new(&env::var("OUT_DIR")?).join("meta.rs");

    let ReleaseInfo { version, branch } = release_info;

    #[rustfmt::skip]
    let meta = format!(r#"
        pub const NAME: &str = "AHEAD-{branch}";
        pub const VERSION: &str = "{version}";
        pub const RELEASE: ReleaseType = ReleaseType::{branch};
    "#);

    fs::write(meta_file, meta)?;

    Ok(())
}

fn get_info() -> ReleaseInfo {
    // CARGO_PKG_* are always available, even in build scripts
    let cargo_tag = env!("CARGO_PKG_VERSION");

    // For any downstream that complains about us doing magic
    if env::var("CARGO_FEATURE_DISTRIBUTION").is_ok() {
        return ReleaseInfo {
            version: cargo_tag.to_string(),
            branch: String::from("Stable"),
        };
    }

    let release_tag = env::var("RELEASE_TAG_NAME").unwrap_or_default();

    if release_tag.starts_with('v') {
        ReleaseInfo {
            version: cargo_tag.to_string(),
            branch: "Stable".to_string(),
        }
    } else {
        #[cfg(not(debug_assertions))]
        let release = "Nightly";
        #[cfg(debug_assertions)]
        let release = "Debug";

        let tag = format!(
            "{cargo_tag}+{release}.{}",
            get_head().unwrap_or_else(|| "unknown".to_string())
        );
        ReleaseInfo {
            version: tag,
            branch: release.to_string(),
        }
    }
}

fn get_head() -> Option<String> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")?;
    let repository_root = Path::new(&manifest_dir).parent()?;
    let output = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .current_dir(repository_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty()).then_some(commit)
}
