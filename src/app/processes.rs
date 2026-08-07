use super::*;
use mount_tui::MountError;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BusyProcess {
    pub(super) pid: u32,
    pub(super) name: String,
}

pub(super) fn is_resource_busy(error: &MountError) -> bool {
    matches!(error, MountError::System(errno) if *errno == rustix::io::Errno::BUSY)
}

pub(super) fn concise_mount_error(error: &MountError) -> String {
    match error {
        MountError::Io(error) => error.to_string(),
        MountError::System(error) => error.to_string(),
        MountError::Parse(message) | MountError::Other(message) => message.clone(),
        MountError::Command { program, message } => format!("{program}: {message}"),
    }
}

/// Best-effort Linux /proc scan. Permission races are expected and skipped.
pub(super) fn processes_using_mount(target: &str) -> Vec<BusyProcess> {
    let target = fs::canonicalize(target).unwrap_or_else(|_| PathBuf::from(target));
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut processes = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            process_uses_path(&entry.path(), &target).then(|| BusyProcess {
                pid,
                name: process_name(&entry.path()).unwrap_or_else(|| "?".to_string()),
            })
        })
        .collect::<Vec<_>>();
    processes.sort_unstable_by_key(|process| process.pid);
    processes
}

fn process_uses_path(process_dir: &Path, target: &Path) -> bool {
    ["cwd", "root", "exe"]
        .iter()
        .any(|name| symlink_points_inside(&process_dir.join(name), target))
        || fs::read_dir(process_dir.join("fd"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| symlink_points_inside(&entry.path(), target))
        || fs::read_to_string(process_dir.join("maps"))
            .ok()
            .is_some_and(|maps| {
                maps.lines()
                    .filter_map(mapped_file_path)
                    .any(|path| path_is_inside(path, target))
            })
}

fn symlink_points_inside(link: &Path, target: &Path) -> bool {
    fs::read_link(link)
        .ok()
        .is_some_and(|path| path_is_inside(&path, target))
}

pub(super) fn path_is_inside(path: &Path, target: &Path) -> bool {
    let path = path
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
        .map_or_else(|| path.to_path_buf(), PathBuf::from);
    path == target || path.starts_with(target)
}

pub(super) fn mapped_file_path(line: &str) -> Option<&Path> {
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        rest = &rest[end..];
    }
    let path = rest.trim_start();
    path.starts_with('/').then(|| Path::new(path))
}

fn process_name(process_dir: &Path) -> Option<String> {
    fs::read_to_string(process_dir.join("comm"))
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}
