//! Streaming hashes and byte splices, so topic size never bounds memory use.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;

use crate::batch_dream::{BatchDreamError, Result, Splice};
use crate::batch_dream_control::BatchDreamControl;

const STREAM_BYTES: usize = 64 * 1024;

pub(crate) fn hash_file(path: &Path, control: &BatchDreamControl) -> Result<String> {
    control.check()?;
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; STREAM_BYTES];
    loop {
        control.check()?;
        let count = file.read(&mut buffer)?;
        let Some(chunk) = buffer.get(..count) else {
            return Err(BatchDreamError::Invalid("invalid read length".to_owned()));
        };
        if chunk.is_empty() {
            break;
        }
        hasher.update(chunk);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Hash for content already in memory; matches `hash_file` for the same bytes.
pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub(crate) fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(BatchDreamError::Invalid(format!(
            "{} is not a regular file of at most {max_bytes} bytes",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(BatchDreamError::Invalid(format!(
            "{} grew past {max_bytes} bytes",
            path.display()
        )));
    }
    Ok(bytes)
}

pub(crate) fn read_range(path: &Path, start: u64, max_bytes: u64) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if start > size || !is_char_boundary(&mut file, start, size)? {
        return Err(BatchDreamError::Invalid(format!(
            "start {start} is past the end or inside a UTF-8 character"
        )));
    }
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes)?;
    let is_eof = start + bytes.len() as u64 == size;
    let valid = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() && !is_eof => error.valid_up_to(),
        Err(_) => {
            return Err(BatchDreamError::Invalid(format!(
                "{} is not valid UTF-8",
                path.display()
            )));
        }
    };
    bytes.truncate(valid);
    if !is_eof && let Some(line_end) = bytes.iter().rposition(|byte| *byte == b'\n') {
        bytes.truncate(line_end + 1);
    }
    let end = start + bytes.len() as u64;
    let text =
        String::from_utf8(bytes).map_err(|error| BatchDreamError::Invalid(error.to_string()))?;
    Ok((text, end))
}

/// `splices` must be sorted, non-overlapping, and on UTF-8 character boundaries.
pub(crate) fn write_spliced(
    source: &Path,
    destination: &mut File,
    splices: &[Splice],
    control: &BatchDreamControl,
) -> Result<()> {
    let mut input = File::open(source)?;
    let size = input.metadata()?.len();
    let mut position = 0u64;
    for splice in splices {
        if splice.start < position || splice.end < splice.start || splice.end > size {
            return Err(BatchDreamError::Invalid(
                "splices must be sorted, non-overlapping, and inside the file".to_owned(),
            ));
        }
        for boundary in [splice.start, splice.end] {
            if !is_char_boundary(&mut input, boundary, size)? {
                return Err(BatchDreamError::Invalid(
                    "splice boundary splits a UTF-8 character".to_owned(),
                ));
            }
        }
        input.seek(SeekFrom::Start(position))?;
        copy_controlled(
            &mut (&mut input).take(splice.start - position),
            destination,
            control,
        )?;
        destination.write_all(splice.text.as_bytes())?;
        position = splice.end;
    }
    input.seek(SeekFrom::Start(position))?;
    copy_controlled(&mut input, destination, control)?;
    destination.sync_all()?;
    Ok(())
}

fn is_char_boundary(file: &mut File, offset: u64, size: u64) -> io::Result<bool> {
    if offset == 0 || offset == size {
        return Ok(true);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte)?;
    Ok(byte[0] & 0xC0 != 0x80)
}

pub(crate) fn publish_copy(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| BatchDreamError::Invalid("destination has no parent".to_owned()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut File::open(source)?, temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| BatchDreamError::Io(error.error))?;
    sync_directory(parent)
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn copy_controlled(
    input: &mut impl io::Read,
    output: &mut impl io::Write,
    control: &BatchDreamControl,
) -> Result<()> {
    let mut buffer = vec![0; STREAM_BYTES];
    loop {
        control.check()?;
        let count = input.read(&mut buffer)?;
        let Some(chunk) = buffer.get(..count) else {
            return Err(BatchDreamError::Invalid("invalid read length".to_owned()));
        };
        if chunk.is_empty() {
            return Ok(());
        }
        output.write_all(chunk)?;
    }
}
