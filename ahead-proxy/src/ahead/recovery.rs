use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;

use super::store::SessionStore;

pub(super) struct RecoveryOwner {
    pub id: String,
    directory: PathBuf,
    _lease: RecoveryLease,
}

struct RecoveryLease(File);

impl RecoveryLease {
    fn try_acquire(directory: &Path, owner: &str) -> Result<Option<Self>> {
        let file = open_lease(directory, owner)?;
        if file.try_lock_exclusive()? {
            Ok(Some(Self(file)))
        } else {
            Ok(None)
        }
    }
}

impl Drop for RecoveryLease {
    fn drop(&mut self) {
        // A fork-inherited descriptor must not outlive the editor's lease.
        if let Err(error) = FileExt::unlock(&self.0) {
            tracing::error!(%error, "failed to unlock editor recovery lease");
        }
    }
}

impl RecoveryOwner {
    pub fn new(workspace: &Path) -> Result<Self> {
        let workspace = workspace.canonicalize()?;
        let ahead = workspace.join(".ahead");
        private_directory(&ahead)?;
        let directory = ahead.join("editor-leases");
        private_directory(&directory)?;
        let id = uuid::Uuid::new_v4().to_string();
        let lease = RecoveryLease::try_acquire(&directory, &id)?
            .context("editor recovery owner is already active")?;
        Ok(Self {
            id,
            directory,
            _lease: lease,
        })
    }

    pub fn reclaim_abandoned(&self, store: &SessionStore) -> Result<()> {
        for owner in store.editor_recovery_owners()? {
            if owner == self.id {
                continue;
            }
            if let Some(_lease) =
                RecoveryLease::try_acquire(&self.directory, &owner)?
            {
                store.claim_editor_recoveries(&owner, &self.id)?;
            }
            // Do not unlink lock files: a concurrent opener could hold the old inode.
        }
        Ok(())
    }
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    anyhow::ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "editor recovery directory is not a regular directory"
    );
    Ok(())
}

fn open_lease(directory: &Path, owner: &str) -> Result<File> {
    let owner =
        uuid::Uuid::parse_str(owner).context("invalid editor recovery owner")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let path = directory.join(format!("{owner}.lock"));
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        anyhow::ensure!(
            metadata.is_file(),
            "editor recovery lease is not a regular file"
        );
    }
    let file = options.open(path)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "editor recovery lease is not a regular file"
    );
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahead_rpc::file::EditorRecoverySnapshot;

    #[test]
    fn recovery_claim_lease_releases_on_error_with_an_inherited_handle() -> Result<()>
    {
        let directory = tempfile::tempdir()?;
        let owner = uuid::Uuid::new_v4().to_string();
        let lease = RecoveryLease::try_acquire(directory.path(), &owner)?
            .context("claim lease")?;
        let inherited = lease.0.try_clone()?;
        assert!(RecoveryLease::try_acquire(directory.path(), &owner)?.is_none());
        let result = (move || -> Result<()> {
            let _lease = lease;
            anyhow::bail!("injected claim failure");
        })();
        assert!(result.is_err());
        assert!(RecoveryLease::try_acquire(directory.path(), &owner)?.is_some());
        drop(inherited);
        assert!(directory.path().join(format!("{owner}.lock")).is_file());
        Ok(())
    }

    #[test]
    fn live_editor_ownership_is_preserved_and_abandoned_data_is_claimed()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let path = workspace.path().join("session.db");
        let store = SessionStore::open(&path)?;
        let first = RecoveryOwner::new(workspace.path())?;
        let inherited = first._lease.0.try_clone()?;
        let second = RecoveryOwner::new(workspace.path())?;
        let snapshot = EditorRecoverySnapshot {
            buffer_id: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            path: "src/main.py".into(),
            contents: Some("print('unsaved')\n".into()),
            saved_sha256: None,
        };
        assert!(store.write_editor_recovery(&first.id, &snapshot)?);
        second.reclaim_abandoned(&store)?;
        assert!(store.editor_recovery_summaries(&second.id)?.is_empty());
        let previous_owner = first.id.clone();
        drop(first);
        second.reclaim_abandoned(&store)?;
        assert_eq!(
            store.editor_recovery(&second.id, &snapshot.buffer_id)?,
            Some(snapshot.clone())
        );
        drop(inherited);
        assert!(!store.write_editor_recovery(
            &previous_owner,
            &EditorRecoverySnapshot {
                revision: 2,
                ..snapshot.clone()
            }
        )?);
        let mut dismissed = snapshot.clone();
        dismissed.revision = 2;
        dismissed.contents = None;
        assert!(store.write_editor_recovery(&second.id, &dismissed)?);
        drop(second);
        let third = RecoveryOwner::new(workspace.path())?;
        third.reclaim_abandoned(&store)?;
        assert!(store.editor_recovery_summaries(&third.id)?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn recovery_leases_reject_symlinks() -> Result<()> {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        symlink(outside.path(), workspace.path().join(".ahead"))?;
        assert!(RecoveryOwner::new(workspace.path()).is_err());
        assert!(fs::read_dir(outside.path())?.next().is_none());
        let owner = uuid::Uuid::new_v4().to_string();
        let target = outside.path().join("untouched");
        fs::write(&target, "unchanged")?;
        symlink(&target, outside.path().join(format!("{owner}.lock")))?;
        assert!(open_lease(outside.path(), &owner).is_err());
        assert_eq!(fs::read_to_string(target)?, "unchanged");
        Ok(())
    }
}
