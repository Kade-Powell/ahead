use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::io;
use std::sync::Arc;

use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemReadStream;
use codex_exec_server::LOCAL_FS;
use codex_exec_server::ReadFileOptions;
use codex_skills::SkillError;
use codex_skills::SkillMetadata;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use futures::StreamExt;

pub(crate) const MAX_SKILL_RESOURCE_CONTENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct SkillLoadOutcome {
    pub skills: Vec<SkillMetadata>,
    pub errors: Vec<SkillError>,
    pub disabled_paths: HashSet<AbsolutePathBuf>,
    pub(crate) skill_discovery_path_by_path: Arc<HashMap<AbsolutePathBuf, AbsolutePathBuf>>,
    pub(crate) file_systems_by_skill_path: SkillFileSystemsByPath,
    pub(crate) implicit_skills_by_scripts_dir: Arc<HashMap<AbsolutePathBuf, SkillMetadata>>,
    pub(crate) implicit_skills_by_doc_path: Arc<HashMap<AbsolutePathBuf, SkillMetadata>>,
}

impl SkillLoadOutcome {
    /// Builds an already-composed outcome while retaining the filesystem that supplied each skill.
    pub(crate) fn from_parts(
        skills: Vec<SkillMetadata>,
        errors: Vec<SkillError>,
        skill_discovery_path_by_path: HashMap<AbsolutePathBuf, AbsolutePathBuf>,
        file_systems_by_skill_path: HashMap<AbsolutePathBuf, Arc<dyn ExecutorFileSystem>>,
    ) -> Self {
        Self {
            skills,
            errors,
            skill_discovery_path_by_path: Arc::new(skill_discovery_path_by_path),
            file_systems_by_skill_path: SkillFileSystemsByPath::new(file_systems_by_skill_path),
            ..Self::default()
        }
    }

    pub fn is_skill_enabled(&self, skill: &SkillMetadata) -> bool {
        !self.disabled_paths.contains(&skill.path_to_skills_md)
    }

    pub(crate) fn with_disabled_paths(mut self, disabled_paths: HashSet<AbsolutePathBuf>) -> Self {
        self.disabled_paths = disabled_paths;
        let mut by_scripts_dir = HashMap::new();
        let mut by_doc_path = HashMap::new();
        for skill in self
            .skills
            .iter()
            .filter(|skill| self.is_skill_enabled(skill))
        {
            let skill_doc_path = canonicalize_if_exists(&skill.path_to_skills_md);
            by_doc_path.insert(skill_doc_path, skill.clone());

            if let Some(skill_dir) = skill.path_to_skills_md.parent() {
                let scripts_dir = canonicalize_if_exists(&skill_dir.join("scripts"));
                by_scripts_dir.insert(scripts_dir, skill.clone());
            }
        }
        self.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
        self.implicit_skills_by_doc_path = Arc::new(by_doc_path);
        self
    }

    /// Returns the logical path used to discover a canonical skill path.
    pub(crate) fn skill_discovery_path_for_path(
        &self,
        path: &AbsolutePathBuf,
    ) -> Option<&AbsolutePathBuf> {
        self.skill_discovery_path_by_path.get(path)
    }

    pub(crate) fn file_system_for_skill(
        &self,
        skill: &SkillMetadata,
    ) -> Option<Arc<dyn ExecutorFileSystem>> {
        self.file_systems_by_skill_path
            .get(&skill.path_to_skills_md)
    }

    /// Reads one loaded skill through the filesystem that discovered it.
    pub(crate) async fn read_skill_text(&self, skill: &SkillMetadata) -> io::Result<String> {
        let fs = self
            .file_system_for_skill(skill)
            .unwrap_or_else(|| Arc::clone(&LOCAL_FS));
        let path = PathUri::from_abs_path(&skill.path_to_skills_md);
        fs.read_file_text(&path, ReadFileOptions::default(), /*sandbox*/ None)
            .await
    }

    pub(crate) async fn read_skill_resource(
        &self,
        skill: &SkillMetadata,
        relative_path: &str,
    ) -> io::Result<String> {
        let fs = self
            .file_system_for_skill(skill)
            .unwrap_or_else(|| Arc::clone(&LOCAL_FS));
        let main_prompt = PathUri::from_abs_path(&skill.path_to_skills_md);
        let skill_directory = main_prompt.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "skill has no package directory",
            )
        })?;
        let requested_path = skill_directory
            .join_descendant(relative_path)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let resolved_path = fs.canonicalize(&requested_path, /*sandbox*/ None).await?;
        if !resolved_path.starts_with(&skill_directory) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "skill resource resolves outside its package",
            ));
        }

        let mut stream: FileSystemReadStream = fs
            .read_file_stream(&resolved_path, /*sandbox*/ None)
            .await?;
        let mut contents = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if contents.len().saturating_add(chunk.len()) > MAX_SKILL_RESOURCE_CONTENT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "skill resource exceeds the size limit",
                ));
            }
            contents.extend_from_slice(&chunk);
        }
        String::from_utf8(contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

impl codex_skills::ImplicitSkillLookup for SkillLoadOutcome {
    fn implicit_skill_for_scripts_dir(&self, path: &AbsolutePathBuf) -> Option<&SkillMetadata> {
        self.implicit_skills_by_scripts_dir.get(path)
    }

    fn implicit_skill_for_doc_path(&self, path: &AbsolutePathBuf) -> Option<&SkillMetadata> {
        self.implicit_skills_by_doc_path.get(path)
    }
}

impl codex_skills::ExplicitSkillLookup for SkillLoadOutcome {
    fn skills(&self) -> &[SkillMetadata] {
        &self.skills
    }

    fn disabled_paths(&self) -> &HashSet<AbsolutePathBuf> {
        &self.disabled_paths
    }

    fn skill_discovery_path_for_path(&self, path: &AbsolutePathBuf) -> Option<&AbsolutePathBuf> {
        SkillLoadOutcome::skill_discovery_path_for_path(self, path)
    }

    fn is_skill_enabled(&self, skill: &SkillMetadata) -> bool {
        SkillLoadOutcome::is_skill_enabled(self, skill)
    }
}

#[derive(Clone, Default)]
pub(crate) struct SkillFileSystemsByPath {
    values: Arc<HashMap<AbsolutePathBuf, Arc<dyn ExecutorFileSystem>>>,
}

impl SkillFileSystemsByPath {
    pub(crate) fn new(values: HashMap<AbsolutePathBuf, Arc<dyn ExecutorFileSystem>>) -> Self {
        Self {
            values: Arc::new(values),
        }
    }

    fn get(&self, path: &AbsolutePathBuf) -> Option<Arc<dyn ExecutorFileSystem>> {
        self.values.get(path).map(Arc::clone)
    }
}

impl fmt::Debug for SkillFileSystemsByPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkillFileSystemsByPath")
            .field("len", &self.values.len())
            .finish()
    }
}

fn canonicalize_if_exists(path: &AbsolutePathBuf) -> AbsolutePathBuf {
    path.canonicalize().unwrap_or_else(|_| path.clone())
}
