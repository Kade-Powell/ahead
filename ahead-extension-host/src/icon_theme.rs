use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

const MAX_ICON_THEME_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct IconThemeFile {
    themes: Vec<IconThemeDefinition>,
}

#[derive(Debug, Deserialize)]
struct IconThemeDefinition {
    name: String,
    #[serde(default)]
    file_icons: HashMap<String, IconPath>,
    #[serde(default)]
    file_stems: HashMap<String, String>,
    #[serde(default)]
    file_suffixes: HashMap<String, String>,
    #[serde(default)]
    directory_icons: FolderIcons,
    #[serde(default)]
    named_directory_icons: HashMap<String, FolderIcons>,
}

#[derive(Deserialize)]
struct DefaultIconAssociations {
    file_stems: HashMap<String, String>,
    file_suffixes: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct IconPath {
    path: String,
}

#[derive(Debug, Default, Deserialize)]
struct FolderIcons {
    collapsed: Option<String>,
    expanded: Option<String>,
}

#[derive(Clone, Debug)]
pub struct IconTheme {
    pub name: String,
    file_icons: HashMap<String, PathBuf>,
    file_stems: HashMap<String, String>,
    file_suffixes: HashMap<String, String>,
    directory_icons: [Option<PathBuf>; 2],
    named_directory_icons: HashMap<String, [Option<PathBuf>; 2]>,
}

impl IconTheme {
    pub fn image_paths(&self) -> impl Iterator<Item = &Path> {
        self.file_icons
            .values()
            .map(PathBuf::as_path)
            .chain(self.directory_icons.iter().flatten().map(PathBuf::as_path))
            .chain(
                self.named_directory_icons
                    .values()
                    .flat_map(|icons| icons.iter().flatten().map(PathBuf::as_path)),
            )
    }

    pub fn icon_for_file(&self, path: &Path) -> Option<&Path> {
        let name = path.file_name()?.to_str()?;
        std::iter::once(name)
            .chain(name.match_indices('.').map(|(index, _)| &name[index + 1..]))
            .find_map(|suffix| {
                self.file_stems
                    .get(suffix)
                    .or_else(|| self.file_suffixes.get(suffix))
                    .and_then(|id| self.file_icons.get(id))
            })
            .or_else(|| self.file_icons.get("file"))
            .or_else(|| self.file_icons.get("default"))
            .map(PathBuf::as_path)
    }

    pub fn icon_for_directory(&self, path: &Path, expanded: bool) -> Option<&Path> {
        let index = usize::from(expanded);
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| self.named_directory_icons.get(name))
            .and_then(|icons| icons[index].as_deref().or(icons[0].as_deref()))
            .or_else(|| {
                self.directory_icons[index]
                    .as_deref()
                    .or(self.directory_icons[0].as_deref())
            })
    }
}

fn checked_icon_path(root: &Path, path: &str) -> Result<PathBuf> {
    let relative = Path::new(path.trim_start_matches("./"));
    ensure!(
        relative
            .extension()
            .is_some_and(|extension| extension == "svg")
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "icon path must be a relative SVG file: {path}"
    );
    let resolved = root
        .join(relative)
        .canonicalize()
        .with_context(|| format!("resolving icon {path}"))?;
    ensure!(
        resolved.starts_with(root),
        "icon path escapes the extension: {path}"
    );
    ensure!(resolved.is_file(), "icon is not a file: {path}");
    Ok(resolved)
}

fn checked_folder_icons(
    root: &Path,
    icons: FolderIcons,
) -> Result<[Option<PathBuf>; 2]> {
    Ok([
        icons
            .collapsed
            .map(|path| checked_icon_path(root, &path))
            .transpose()?,
        icons
            .expanded
            .map(|path| checked_icon_path(root, &path))
            .transpose()?,
    ])
}

pub fn load_icon_theme(
    extension_root: &Path,
    theme_file: &Path,
) -> Result<IconTheme> {
    ensure!(
        theme_file
            .extension()
            .is_some_and(|extension| extension == "json")
            && theme_file
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "icon theme path must be a relative JSON file"
    );
    let root = extension_root.canonicalize()?;
    let theme_path = root.join(theme_file).canonicalize()?;
    ensure!(
        theme_path.starts_with(&root),
        "icon theme path escapes the extension"
    );
    ensure!(
        theme_path.metadata()?.len() <= MAX_ICON_THEME_BYTES,
        "icon theme exceeds the 2 MiB limit"
    );
    let content = std::fs::read(&theme_path)?;
    ensure!(
        content.len() as u64 <= MAX_ICON_THEME_BYTES,
        "icon theme exceeds the 2 MiB limit"
    );
    let file: IconThemeFile = serde_json::from_slice(&content)
        .with_context(|| format!("parsing {}", theme_path.display()))?;
    let theme = file
        .themes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("icon theme has no themes"))?;
    let file_icons = theme
        .file_icons
        .into_iter()
        .map(|(name, icon)| Ok((name, checked_icon_path(&root, &icon.path)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let directory_icons = checked_folder_icons(&root, theme.directory_icons)?;
    let named_directory_icons = theme
        .named_directory_icons
        .into_iter()
        .map(|(name, icons)| Ok((name, checked_folder_icons(&root, icons)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let mut defaults: DefaultIconAssociations =
        serde_json::from_str(include_str!("default_icon_associations.json"))?;
    defaults.file_stems.extend(theme.file_stems);
    defaults.file_suffixes.extend(theme.file_suffixes);
    Ok(IconTheme {
        name: theme.name,
        file_icons,
        file_stems: defaults.file_stems,
        file_suffixes: defaults.file_suffixes,
        directory_icons,
        named_directory_icons,
    })
}

#[cfg(test)]
mod tests {
    use super::load_icon_theme;
    use std::path::Path;

    #[test]
    fn resolves_stems_compound_suffixes_folders_and_fallbacks() {
        let root = tempfile::tempdir().expect("extension");
        std::fs::create_dir(root.path().join("icon_themes")).expect("themes");
        std::fs::create_dir(root.path().join("icons")).expect("icons");
        for name in [
            "file",
            "rust",
            "python",
            "typescript",
            "test-ts",
            "docker",
            "helm",
            "vcs",
            "folder",
            "folder-open",
            "src",
        ] {
            std::fs::write(root.path().join(format!("icons/{name}.svg")), "<svg/>")
                .expect("icon");
        }
        std::fs::write(root.path().join("icon_themes/test.json"), r#"{
            "themes": [{"name":"Test", "file_icons": {
                "file":{"path":"./icons/file.svg"},
                "rust":{"path":"./icons/rust.svg"},
                "python":{"path":"./icons/python.svg"},
                "typescript":{"path":"./icons/typescript.svg"},
                "test-ts":{"path":"./icons/test-ts.svg"},
                "docker":{"path":"./icons/docker.svg"},
                "helm":{"path":"./icons/helm.svg"},
                "vcs":{"path":"./icons/vcs.svg"}
            }, "file_stems":{"special.ts":"rust"},
            "file_suffixes":{"cts":"python","test.ts":"test-ts","d.ts":"typescript"},
            "directory_icons":{"collapsed":"./icons/folder.svg","expanded":"./icons/folder-open.svg"},
            "named_directory_icons":{"src":{"collapsed":"./icons/src.svg"}}
            }]}"#).expect("theme");
        let theme = load_icon_theme(root.path(), Path::new("icon_themes/test.json"))
            .expect("load theme");
        for (name, icon) in [
            ("lib.rs", "rust"),
            ("main.py", "python"),
            ("types.d.ts", "typescript"),
            ("main.ts", "typescript"),
            ("main.mts", "typescript"),
            ("main.cts", "python"),
            ("math.test.ts", "test-ts"),
            ("special.ts", "rust"),
            ("Dockerfile", "docker"),
            ("Chart.yaml", "helm"),
            ("COMMIT_EDITMSG", "vcs"),
            ("main.js", "file"),
            ("unknown.zz", "file"),
        ] {
            assert_eq!(
                theme
                    .icon_for_file(Path::new(name))
                    .and_then(|path| path.file_stem())
                    .and_then(|name| name.to_str()),
                Some(icon)
            );
        }
        assert_eq!(
            theme
                .icon_for_directory(Path::new("src"), false)
                .and_then(|path| path.file_stem())
                .and_then(|name| name.to_str()),
            Some("src")
        );
        assert_eq!(
            theme
                .icon_for_directory(Path::new("src"), true)
                .and_then(|path| path.file_stem())
                .and_then(|name| name.to_str()),
            Some("src")
        );
        assert_eq!(
            theme
                .icon_for_directory(Path::new("other"), true)
                .and_then(|path| path.file_stem())
                .and_then(|name| name.to_str()),
            Some("folder-open")
        );
    }

    #[test]
    fn rejects_icon_paths_outside_extension() {
        let root = tempfile::tempdir().expect("extension");
        std::fs::create_dir(root.path().join("icon_themes")).expect("themes");
        std::fs::write(root.path().join("icon_themes/test.json"), r#"{
            "themes": [{"name":"Test", "file_icons":{"file":{"path":"../outside.svg"}}}]
        }"#).expect("theme");
        assert!(
            load_icon_theme(root.path(), Path::new("icon_themes/test.json"))
                .is_err()
        );
    }
}
