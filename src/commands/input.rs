use std::{fs, io::Read, path::Path};

pub(super) fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "missing component")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("unsupported component type or size");
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| "cannot open component")?;
    if !file
        .metadata()
        .map_err(|_| "cannot inspect component")?
        .is_file()
    {
        return Err("component is not a regular file");
    }
    read_bounded(file, limit)
}

pub(super) fn read_bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    reader
        .take(limit.checked_add(1).ok_or("invalid component size limit")?)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read component")?;
    if bytes.len() as u64 > limit {
        return Err("component too large");
    }
    Ok(bytes)
}

#[cfg(any(unix, windows))]
pub(super) fn read_stdin(limit: u64) -> Result<Vec<u8>, &'static str> {
    let stdin = std::io::stdin().lock();
    #[cfg(unix)]
    let handle = {
        use std::os::fd::AsFd;
        stdin.as_fd().try_clone_to_owned()
    };
    #[cfg(windows)]
    let handle = {
        use std::os::windows::io::AsHandle;
        stdin.as_handle().try_clone_to_owned()
    };
    // StdinLock is buffered; a duplicate OS handle keeps the read bound physical.
    let file = fs::File::from(handle.map_err(|_| "cannot open standard input")?);
    read_bounded(file, limit)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn read_stdin(_: u64) -> Result<Vec<u8>, &'static str> {
    Err("standard input unsupported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn bounded_reader_stops_at_one_extra_byte_without_draining() {
        for limit in [0, 1, 65_536] {
            for size in [0, limit, limit + 1, limit + 2] {
                let mut reader = Cursor::new(vec![b' '; size as usize]);
                let result = read_bounded(&mut reader, limit);
                if size <= limit {
                    assert_eq!(result.unwrap().len() as u64, size);
                } else {
                    assert_eq!(result.unwrap_err(), "component too large");
                }
                assert_eq!(reader.position(), size.min(limit + 1));
            }
        }
        let mut reader = Cursor::new(b"FAKE_PRIVATE_BYTES");
        assert_eq!(
            read_bounded(&mut reader, u64::MAX).unwrap_err(),
            "invalid component size limit"
        );
        assert_eq!(reader.position(), 0);
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("FAKE_PRIVATE_READER_ERROR"))
        }
    }

    #[test]
    fn bounded_reader_never_exposes_io_error_details() {
        assert_eq!(
            read_bounded(FailingReader, 65_536).unwrap_err(),
            "cannot read component"
        );
        let mut partial = Cursor::new(b"FAKE_PRIVATE_BYTES").chain(FailingReader);
        assert_eq!(
            read_bounded(&mut partial, 65_536).unwrap_err(),
            "cannot read component"
        );
    }
}
