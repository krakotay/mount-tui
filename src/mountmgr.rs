use nix::errno::Errno;
use nix::mount::{MsFlags, mount as nix_mount};
use std::fs;
use std::io;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDevice {
    pub name: String,
    pub path: String,
    pub size_bytes: Option<u64>,
    pub removable: bool,
    pub is_partition: bool,
    pub mapper_name: Option<String>,
    pub model: Option<String>,
    pub vendor: Option<String>,
}

/// Ошибки библиотеки
#[derive(Error, Debug)]
pub enum MountError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Nix error: {0}")]
    Nix(#[from] Errno),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Other: {0}")]
    Other(String),
}

/// Небольшой менеджер монтирования
pub struct MountManager;

impl MountManager {
    /// Пробует прочитать /proc/mounts, а если нет — /etc/mtab.
    /// Формат в /proc/mounts (space-separated):
    /// source target fstype options dump pass
    pub fn list_mounts() -> Result<Vec<MountEntry>, MountError> {
        let content = fs::read_to_string("/proc/mounts")
            .or_else(|_| fs::read_to_string("/etc/mtab"))
            .map_err(MountError::Io)?;

        let mut out = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            // split by spaces, but fields may contain escape sequences like \040 for space.
            // /proc/mounts uses C-style escaping: spaces are encoded as \040.
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                return Err(MountError::Parse(format!(
                    "invalid mounts line {}: {}",
                    i + 1,
                    line
                )));
            }
            let src = unescape_mount_field(parts[0]);
            let target = unescape_mount_field(parts[1]);
            let fstype = parts[2].to_string();
            let options = parts[3].split(',').map(|s| s.to_string()).collect();

            out.push(MountEntry {
                source: src,
                target,
                fstype,
                options,
            });
        }
        Ok(out)
    }

    /// Примонтировать.
    /// - `source`: device or source (e.g. "/dev/sdb1" or "tmpfs")
    /// - `target`: existing directory where mount should happen
    /// - `fstype`: filesystem type (Some("ext4"), Some("tmpfs")) or None (let kernel decide)
    /// - `opts`: comma-separated options string, e.g. "rw,nodev,noexec"
    pub fn mount(
        source: &str,
        target: &str,
        fstype: Option<&str>,
        opts: Option<&str>,
        flags: MsFlags,
    ) -> Result<(), MountError> {
        // ensure target exists
        let p = Path::new(target);
        if !p.exists() {
            return Err(MountError::Other(format!(
                "target path does not exist: {}",
                target
            )));
        }

        // nix::mount::mount accepts Option<&Path> or Option<&str> that implement NixPath.
        // data param is Option<&str> — pass opts.
        nix_mount(Some(source), target, fstype, flags, opts).map_err(|e| MountError::Nix(e))
    }

    /// Отмонтировать (обычное umount). Для форсированного отмонтирования можно вызвать umount2 с флагами,
    /// но здесь — простая обёртка над libc umount/umount2 не реализована; используем nix::mount::umount.
    pub fn umount(target: &str) -> Result<(), MountError> {
        // nix exposes umount
        nix::mount::umount(target).map_err(|e| MountError::Nix(e))
    }

    /// Список блочных устройств через /sys/class/block.
    pub fn list_block_devices() -> Result<Vec<BlockDevice>, MountError> {
        let mut out = Vec::new();
        for entry in fs::read_dir("/sys/class/block").map_err(MountError::Io)? {
            let entry = entry.map_err(MountError::Io)?;
            let name = entry.file_name().to_string_lossy().to_string();
            let dev_path = format!("/dev/{}", name);
            let sys_path = format!("/sys/class/block/{}", name);
            let size_bytes = read_u64_from_file(&format!("{}/size", sys_path))
                .map(|sectors| sectors.saturating_mul(512));
            let removable = read_u64_from_file(&format!("{}/removable", sys_path))
                .map(|v| v == 1)
                .unwrap_or(false);
            let is_partition = Path::new(&format!("{}/partition", sys_path)).exists();

            let mapper_name = if name.starts_with("dm-") {
                read_trimmed(&format!("{}/dm/name", sys_path))
            } else {
                None
            };

            let model = read_trimmed(&format!("{}/device/model", sys_path));
            let vendor = read_trimmed(&format!("{}/device/vendor", sys_path));

            out.push(BlockDevice {
                name,
                path: dev_path,
                size_bytes,
                removable,
                is_partition,
                mapper_name,
                model,
                vendor,
            });
        }
        Ok(out)
    }
}

/// В /proc/mounts пробелы и некоторые символы закодированы как \040 и т.д.
/// Эта функция разворачивает такие escape-последовательности обратно в обычные символы.
fn unescape_mount_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // try read next three digits (octal)
            let mut oct = String::new();
            for _ in 0..3 {
                if let Some(d) = chars.next() {
                    oct.push(d);
                } else {
                    break;
                }
            }
            if oct.len() == 3 {
                if let Ok(v) = u8::from_str_radix(&oct, 8) {
                    out.push(v as char);
                    continue;
                } else {
                    out.push('\\');
                    out.push_str(&oct);
                    continue;
                }
            } else {
                out.push('\\');
                out.push_str(&oct);
                continue;
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_u64_from_file(path: &str) -> Option<u64> {
    read_trimmed(path).and_then(|s| s.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescape() {
        assert_eq!(unescape_mount_field("foo\\040bar"), "foo bar");
        assert_eq!(unescape_mount_field("a\\040b\\011c"), "a b\tc");
    }
}
