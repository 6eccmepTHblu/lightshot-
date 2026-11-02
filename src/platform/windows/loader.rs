//! Keeps files placed beside the portable executable from being loaded or run.

#![allow(unsafe_code)]

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, process::Command};

use anyhow::{bail, Result};
use windows::Win32::System::{
    LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32},
    SystemInformation::GetWindowsDirectoryW,
};

const MAX_WINDOWS_PATH: usize = 1024;

/// Limits later `LoadLibrary` calls by bare name, including those made by
/// dependencies, to System32. The import table is covered by a link flag in
/// `build.rs`. Call this first in `main`.
pub fn restrict_dll_search() -> Result<()> {
    // SAFETY: the call takes only a flag and changes this process's search order.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) }?;
    Ok(())
}

/// Builds a command for the real Explorer. A bare `explorer.exe` would be
/// searched for in the executable's own directory first.
pub fn explorer_command() -> Result<Command> {
    Ok(Command::new(windows_directory()?.join("explorer.exe")))
}

fn windows_directory() -> Result<PathBuf> {
    let mut buffer = [0_u16; MAX_WINDOWS_PATH];
    // SAFETY: the buffer is writable and its length is passed through the slice.
    let length = unsafe { GetWindowsDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        bail!("не удалось определить папку Windows");
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_command_uses_an_absolute_path_to_an_existing_file() {
        let command = explorer_command().unwrap();
        let program = PathBuf::from(command.get_program());
        assert!(program.is_absolute(), "{}", program.display());
        assert!(program.ends_with("explorer.exe"));
        assert!(program.is_file(), "{}", program.display());
    }
}
