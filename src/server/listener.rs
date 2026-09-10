//! Safe Unix-domain listener lifecycle.

use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_SOCKET: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

impl SocketIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
    }
}

/// Armed immediately after binding a unique pathname in an owner-private
/// directory. Before metadata succeeds, that private unique name is the safe
/// cleanup identity; afterwards device/inode matching protects replacements.
#[derive(Debug)]
struct CreatedSocketGuard {
    path: PathBuf,
    published_path: Option<PathBuf>,
    identity: Option<SocketIdentity>,
    armed: bool,
}

impl CreatedSocketGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            published_path: None,
            identity: None,
            armed: true,
        }
    }

    fn capture_identity(&mut self) -> io::Result<SocketIdentity> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "created Unix socket path is not a socket",
            ));
        }
        let identity = SocketIdentity::from_metadata(&metadata);
        self.identity = Some(identity);
        Ok(identity)
    }

    fn publish_no_replace(&mut self, destination: &Path) -> io::Result<()> {
        // Atomic no-replace publication. If the final entry already exists,
        // hard_link leaves it untouched and the guard cleans only our temp.
        fs::hard_link(&self.path, destination)?;
        self.published_path = Some(destination.to_path_buf());
        fs::remove_file(&self.path)?;
        self.path = destination.to_path_buf();
        self.published_path = None;
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        let primary = self.path.clone();
        let published = self.published_path.take();
        self.remove_if_owned(&primary)?;
        if let Some(path) = published
            && path != primary
        {
            self.remove_if_owned(&path)?;
        }
        Ok(())
    }

    fn remove_if_owned(&self, path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                let remove = self.identity.map_or_else(
                    || metadata.file_type().is_socket(),
                    |id| id.matches(&metadata),
                );
                if remove {
                    fs::remove_file(path)
                } else {
                    Ok(())
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for CreatedSocketGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[derive(Debug)]
pub struct BoundUnixListener {
    listener: UnixListener,
    cleanup: CreatedSocketGuard,
}

impl BoundUnixListener {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::bind_inner(path.as_ref(), || Ok(()))
    }

    #[doc(hidden)]
    #[cfg(debug_assertions)]
    pub fn bind_with_post_bind_hook_for_test(
        path: impl AsRef<Path>,
        hook: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::bind_inner(path.as_ref(), hook)
    }

    fn bind_inner(path: &Path, post_bind: impl FnOnce() -> io::Result<()>) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket path must be absolute",
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket path has no parent",
            )
        })?;
        let parent_metadata = fs::metadata(parent)?;
        if !parent_metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket parent is not a directory",
            ));
        }
        let effective_uid = unsafe { libc::geteuid() };
        if parent_metadata.uid() != effective_uid
            || parent_metadata.permissions().mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unix socket parent must be owner-only (0700-style) and owned by the current user",
            ));
        }

        if let Ok(existing) = fs::symlink_metadata(path) {
            if !existing.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "refusing to unlink a non-socket path",
                ));
            }
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "Unix socket already has a live listener",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "Unix socket liveness is uncertain; refusing to unlink",
                    ));
                }
            }
            if existing.uid() != effective_uid {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "stale Unix socket is not owned by the current user",
                ));
            }
            let current = fs::symlink_metadata(path)?;
            if !current.file_type().is_socket()
                || current.dev() != existing.dev()
                || current.ino() != existing.ino()
            {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Unix socket path changed during stale-socket validation",
                ));
            }
            fs::remove_file(path)?;
        }

        // Bind a unique name inside the owner-private directory. The guard is
        // armed immediately, before metadata/chmod/rename can fail.
        let temporary = parent.join(format!(
            ".vcd-tools-{}-{}.sock",
            std::process::id(),
            NEXT_TEMP_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&temporary)?;
        let mut cleanup = CreatedSocketGuard::new(temporary);
        post_bind()?;
        let identity = cleanup.capture_identity()?;
        fs::set_permissions(&cleanup.path, fs::Permissions::from_mode(0o600))?;
        let chmod_metadata = fs::symlink_metadata(&cleanup.path)?;
        if !identity.matches(&chmod_metadata) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Unix socket temporary path changed during creation",
            ));
        }
        cleanup.publish_no_replace(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !identity.matches(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Unix socket path changed during creation",
            ));
        }
        Ok(Self { listener, cleanup })
    }

    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    pub fn path(&self) -> &Path {
        &self.cleanup.path
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.listener.set_nonblocking(nonblocking)
    }

    pub fn cleanup(&mut self) -> io::Result<()> {
        self.cleanup.cleanup()
    }
}
