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
    let mut bytes = Vec::new();
    file.take(limit.checked_add(1).ok_or("invalid component size limit")?)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read component")?;
    if bytes.len() as u64 > limit {
        return Err("component too large");
    }
    Ok(bytes)
}
