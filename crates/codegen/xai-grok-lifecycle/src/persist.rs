//! Durable registrations: one JSON file per handler name, stamped with the kernel boot id.
//!
//! A workspace-server restart within one boot keeps registrations; a new boot (a VM restore) drops them, because the
//! processes they point at are gone. Writes are atomic (a temp file in the same dir, then a rename over the target),
//! so a crash never leaves a half-written record. Temp files start with a dot, which no handler name can.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::exec_spec::ExecSpec;
use crate::registry::MAX_REGISTERED_HANDLERS;
use crate::token::HandlerName;
use crate::wire::PersistedHandlerWire;

/// Cap on one record file; a valid record is at most the 4 KiB PUT body plus the stamp.
const MAX_RECORD_BYTES: u64 = 8 * 1024;

/// Cap on directory entries examined at load.
const MAX_STATE_DIR_ENTRIES: usize = 256;

const RECORD_SUFFIX: &str = ".json";
const TEMP_PREFIX: &str = ".tmp-";

/// The kernel boot id a registration was made under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BootId(String);

impl From<&str> for BootId {
    fn from(id: &str) -> Self {
        BootId(id.to_owned())
    }
}

impl BootId {
    /// The running kernel's boot id. `None` off Linux, or when it cannot be read; registrations then stay in memory.
    pub(crate) fn current() -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
            const MAX_BOOT_ID_BYTES: u64 = 64;
            let mut id = String::new();
            let read = fs::File::open(BOOT_ID_PATH)
                .and_then(|file| file.take(MAX_BOOT_ID_BYTES).read_to_string(&mut id));
            match read {
                Ok(_) if !id.trim().is_empty() => Some(BootId::from(id.trim())),
                Ok(_) => {
                    tracing::warn!(
                        "kernel boot id is empty; lifecycle registrations stay in memory"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(error = %e, "kernel boot id unreadable; lifecycle registrations stay in memory");
                    None
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}

/// On-disk home of the registered handlers.
#[derive(Debug)]
pub(crate) struct RegistrationStore {
    dir: PathBuf,
    boot_id: BootId,
}

impl RegistrationStore {
    pub(crate) fn new(dir: PathBuf, boot_id: BootId) -> Self {
        RegistrationStore { dir, boot_id }
    }

    /// Loads this boot's records. Deletes records from another boot, records that no longer parse, records past the
    /// registered-handler cap, and temp files a crashed write left behind. Other files are left alone.
    pub(crate) fn load_sync(&self) -> BTreeMap<HandlerName, ExecSpec> {
        let mut loaded = BTreeMap::new();
        if let Err(e) = fs::create_dir_all(&self.dir) {
            tracing::warn!(dir = %self.dir.display(), error = %e, "lifecycle state dir unusable");
            return loaded;
        }
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(dir = %self.dir.display(), error = %e, "lifecycle state dir unreadable");
                return loaded;
            }
        };
        for entry in entries.take(MAX_STATE_DIR_ENTRIES) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!(error = %e, "lifecycle state dir entry unreadable");
                    continue;
                }
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if file_name.starts_with(TEMP_PREFIX) {
                remove_logged(&path, "leftover temp file");
                continue;
            }
            let Some(name) = file_name
                .strip_suffix(RECORD_SUFFIX)
                .and_then(|stem| HandlerName::try_from(stem).ok())
            else {
                continue;
            };
            match read_record(&path) {
                Ok(record) if record.boot_id != self.boot_id.0 => {
                    remove_logged(&path, "registration from another boot");
                }
                Ok(record) => match ExecSpec::from_wire(record.spec) {
                    Ok(spec) => {
                        loaded.insert(name, spec);
                    }
                    Err(class) => {
                        tracing::warn!(handler = %name, ?class, "persisted registration is invalid");
                        remove_logged(&path, "invalid registration");
                    }
                },
                Err(e) => {
                    tracing::warn!(handler = %name, error = %e, "persisted registration unreadable");
                    remove_logged(&path, "unreadable registration");
                }
            }
        }
        while loaded.len() > MAX_REGISTERED_HANDLERS {
            if let Some((name, _)) = loaded.pop_last() {
                remove_logged(&self.record_path(&name), "registration past the cap");
            }
        }
        loaded
    }

    pub(crate) fn save_sync(&self, name: &HandlerName, spec: &ExecSpec) -> io::Result<()> {
        let record = PersistedHandlerWire {
            boot_id: self.boot_id.0.clone(),
            spec: spec.to_wire(),
        };
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        fs::create_dir_all(&self.dir)?;
        let mut temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .tempfile_in(&self.dir)?;
        // No fsync: a record only matters within one kernel boot, and the page cache outlives a process restart.
        temp.write_all(&bytes)?;
        temp.persist(self.record_path(name)).map_err(|e| e.error)?;
        Ok(())
    }

    /// Succeeds when the record is already absent.
    pub(crate) fn remove_sync(&self, name: &HandlerName) -> io::Result<()> {
        match fs::remove_file(self.record_path(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn record_path(&self, name: &HandlerName) -> PathBuf {
        self.dir.join(format!("{name}{RECORD_SUFFIX}"))
    }
}

fn read_record(path: &Path) -> io::Result<PersistedHandlerWire> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record too large",
        ));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn remove_logged(path: &Path, what: &'static str) {
    match fs::remove_file(path) {
        Ok(()) => tracing::info!(path = %path.display(), what, "removed stale lifecycle state"),
        Err(e) => {
            tracing::warn!(path = %path.display(), what, error = %e, "failed to remove stale lifecycle state")
        }
    }
}

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
