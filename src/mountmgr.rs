use rustix::mount::{MountFlags, UnmountFlags};
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::{ffi::CString, path::PathBuf};
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
    /// Filesystem signature reported by udev, if available. Detection is
    /// best-effort: missing or malformed udev data never hides the device.
    pub fstype: Option<String>,
}

/// Ошибки библиотеки
#[derive(Error, Debug)]
pub enum MountError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("System error: {0}")]
    System(#[from] rustix::io::Errno),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Other: {0}")]
    Other(String),
    #[error("{program} failed: {message}")]
    Command { program: String, message: String },
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
        flags: MountFlags,
    ) -> Result<(), MountError> {
        // ensure target exists
        let p = Path::new(target);
        if !p.exists() {
            return Err(MountError::Other(format!(
                "target path does not exist: {}",
                target
            )));
        }

        // ntfs-3g is a FUSE userspace driver, not a filesystem accepted by
        // mount(2). It must be started through the mount helper.
        if fstype == Some("ntfs-3g") {
            let options = opts
                .unwrap_or_default()
                .split(',')
                .filter(|option| !option.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            return mount_with_helper(source, target, "ntfs-3g", &options);
        }

        let Some(fstype) = fstype else {
            // The mount syscall does not auto-detect filesystems. Delegate an
            // unspecified type to mount(8), which performs the expected probe.
            let options = opts
                .unwrap_or_default()
                .split(',')
                .filter(|option| !option.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            return mount_with_helper(source, target, "auto", &options);
        };
        let (flags, data_options) = prepare_mount_options(opts, flags);
        let data = (!data_options.is_empty())
            .then(|| CString::new(data_options.join(",")))
            .transpose()
            .map_err(|_| MountError::Other("mount options contain a NUL byte".to_string()))?;
        rustix::mount::mount(source, target, fstype, flags, data.as_deref())?;
        Ok(())
    }

    /// Mount an SMB share through mount.cifs(8).  The helper is deliberately
    /// invoked instead of the mount(2) syscall because it also resolves server
    /// names and prepares CIFS-specific kernel options.
    ///
    /// Credentials are passed in a short-lived, mode 0600 file and never as a
    /// command-line argument (where they would be visible through `ps`).
    pub fn mount_smb(
        source: &str,
        target: &str,
        username: Option<&str>,
        password: Option<&str>,
        domain: Option<&str>,
        opts: Option<&str>,
    ) -> Result<(), MountError> {
        let source = normalize_smb_source(source)?;
        ensure_mount_target(target)?;
        let (options, credentials) = prepare_smb_options(username, password, domain, opts)?;
        let result = mount_cifs_with_options(&source, target, &options);
        drop(credentials);
        result
    }

    /// Reconnect an existing SMB mount with credentials entered in the TUI.
    /// This avoids mount.cifs opening `/dev/tty` for its own password prompt,
    /// which is incompatible with the application's raw terminal mode.
    pub fn reconnect_smb(
        source: &str,
        target: &str,
        username: Option<&str>,
        password: Option<&str>,
        domain: Option<&str>,
        opts: Option<&str>,
        previous_options: &[String],
    ) -> Result<(), MountError> {
        let source = normalize_smb_source(source)?;
        ensure_mount_target(target)?;
        let (options, credentials) = prepare_smb_options(username, password, domain, opts)?;
        let authentication = options
            .iter()
            .find(|option| option.starts_with("credentials=") || option.as_str() == "guest")
            .cloned();
        let mut rollback_options = reusable_smb_options(previous_options);
        if let Some(authentication) = authentication {
            rollback_options.push(authentication);
        }

        Self::umount(target)?;
        let result = mount_cifs_with_options(&source, target, &options);
        if let Err(error) = result {
            let rollback = mount_cifs_with_options(&source, target, &rollback_options);
            let rollback_message = match rollback {
                Ok(()) => "the original mount options were restored".to_string(),
                Err(rollback_error) => format!("restore also failed: {rollback_error}"),
            };
            drop(credentials);
            return Err(MountError::Other(format!(
                "SMB reconnect failed: {error}; {rollback_message}"
            )));
        }
        drop(credentials);
        Ok(())
    }

    /// Give the invoking desktop user access to a mounted local filesystem.
    /// Filesystems with synthetic ownership are reconnected with uid/gid. On
    /// Unix-native filesystems the mount root itself is chowned; existing child
    /// entries keep their ownership and permissions. SMB must use
    /// `reconnect_smb`, because reconnecting it requires credentials.
    pub fn make_user_accessible(
        source: &str,
        target: &str,
        fstype: &str,
        current_options: &[String],
        uid: u32,
        gid: u32,
    ) -> Result<UserAccessMethod, MountError> {
        ensure_mount_target(target)?;
        if is_smb_fstype(fstype) {
            return Err(MountError::Other(
                "SMB access changes require reconnect_smb with credentials".to_string(),
            ));
        }
        if uses_mount_ownership(fstype) {
            let updated_options = replace_ownership_options(current_options, fstype, uid, gid);
            Self::umount(target)?;
            if let Err(error) = mount_with_helper(source, target, fstype, &updated_options) {
                let rollback = mount_with_helper(source, target, fstype, current_options);
                let rollback_message = match rollback {
                    Ok(()) => "the original mount was restored".to_string(),
                    Err(rollback_error) => format!("restore also failed: {rollback_error}"),
                };
                return Err(MountError::Other(format!(
                    "mount with user ownership failed: {error}; {rollback_message}"
                )));
            }
            Ok(UserAccessMethod::Reconnected)
        } else {
            rustix::fs::chown(
                target,
                Some(rustix::process::Uid::from_raw(uid)),
                Some(rustix::process::Gid::from_raw(gid)),
            )?;
            Ok(UserAccessMethod::ChangedOwner)
        }
    }

    /// Unmount a filesystem without force or lazy flags.
    pub fn umount(target: &str) -> Result<(), MountError> {
        rustix::mount::unmount(target, UnmountFlags::empty())?;
        Ok(())
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
            let fstype = read_udev_property(&sys_path, "ID_FS_TYPE");

            out.push(BlockDevice {
                name,
                path: dev_path,
                size_bytes,
                removable,
                is_partition,
                mapper_name,
                model,
                vendor,
                fstype,
            });
        }
        Ok(out)
    }
}

fn prepare_mount_options(opts: Option<&str>, mut flags: MountFlags) -> (MountFlags, Vec<&str>) {
    let mut data = Vec::new();
    for option in opts
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
    {
        match option {
            // mount(8)/fstab options must not be passed as filesystem data.
            // In particular, ext4 rejects `defaults` with EINVAL.
            "defaults" | "auto" | "noauto" | "user" | "nouser" | "users" | "nofail" | "owner"
            | "group" | "_netdev" => {}
            "ro" => flags.insert(MountFlags::RDONLY),
            "rw" => flags.remove(MountFlags::RDONLY),
            "nosuid" => flags.insert(MountFlags::NOSUID),
            "suid" => flags.remove(MountFlags::NOSUID),
            "nodev" => flags.insert(MountFlags::NODEV),
            "dev" => flags.remove(MountFlags::NODEV),
            "noexec" => flags.insert(MountFlags::NOEXEC),
            "exec" => flags.remove(MountFlags::NOEXEC),
            "sync" => flags.insert(MountFlags::SYNCHRONOUS),
            "async" => flags.remove(MountFlags::SYNCHRONOUS),
            "dirsync" => flags.insert(MountFlags::DIRSYNC),
            "noatime" => flags.insert(MountFlags::NOATIME),
            "atime" => flags.remove(MountFlags::NOATIME),
            "nodiratime" => flags.insert(MountFlags::NODIRATIME),
            "diratime" => flags.remove(MountFlags::NODIRATIME),
            "relatime" => flags.insert(MountFlags::RELATIME),
            "strictatime" => flags.insert(MountFlags::STRICTATIME),
            "lazytime" => flags.insert(MountFlags::LAZYTIME),
            "nolazytime" => flags.remove(MountFlags::LAZYTIME),
            "nosymfollow" => flags.insert(MountFlags::NOSYMFOLLOW),
            other => data.push(other),
        }
    }
    (flags, data)
}

fn read_udev_property(sys_path: &str, property: &str) -> Option<String> {
    // /sys and /run/udev are local virtual/regular files. Unlike invoking
    // blkid, reading them does not probe the block device and cannot hold up
    // the UI on a damaged or sleeping drive.
    let dev_id = fs::read_to_string(format!("{sys_path}/dev")).ok()?;
    let data = fs::read_to_string(format!("/run/udev/data/b{}", dev_id.trim())).ok()?;
    let prefix = format!("E:{property}=");
    data.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAccessMethod {
    Reconnected,
    ChangedOwner,
}

pub fn is_smb_fstype(fstype: &str) -> bool {
    matches!(fstype, "cifs" | "smb3")
}

pub fn uses_mount_ownership(fstype: &str) -> bool {
    matches!(
        fstype,
        "cifs" | "smb3" | "vfat" | "exfat" | "ntfs" | "ntfs3"
    )
}

pub fn ownership_options(fstype: &str, uid: u32, gid: u32) -> String {
    if is_smb_fstype(fstype) {
        format!("uid={uid},gid={gid},forceuid,forcegid,file_mode=0664,dir_mode=0775")
    } else {
        format!("uid={uid},gid={gid},umask=022")
    }
}

fn replace_ownership_options(
    current_options: &[String],
    fstype: &str,
    uid: u32,
    gid: u32,
) -> Vec<String> {
    let mut options: Vec<String> = current_options
        .iter()
        .filter(|option| {
            let key = option
                .split_once('=')
                .map_or(option.as_str(), |(key, _)| key);
            !matches!(key, "uid" | "gid" | "umask" | "fmask" | "dmask")
        })
        .cloned()
        .collect();
    options.extend(
        ownership_options(fstype, uid, gid)
            .split(',')
            .map(str::to_string),
    );
    options
}

fn mount_with_helper(
    source: &str,
    target: &str,
    fstype: &str,
    options: &[String],
) -> Result<(), MountError> {
    let mut command = Command::new("mount");
    command.args(["-t", fstype, source, target]);
    if !options.is_empty() {
        command.args(["-o", &options.join(",")]);
    }
    run_command(command, "mount")
}

fn prepare_smb_options(
    username: Option<&str>,
    password: Option<&str>,
    domain: Option<&str>,
    opts: Option<&str>,
) -> Result<(Vec<String>, Option<CredentialFile>), MountError> {
    for (name, value) in [
        ("username", username),
        ("password", password),
        ("domain", domain),
    ] {
        if value.is_some_and(|value| value.contains(['\n', '\r'])) {
            return Err(MountError::Other(format!(
                "SMB {name} must not contain a newline"
            )));
        }
    }

    let mut options = sanitized_smb_options(opts)?;
    let has_username = username.is_some_and(|value| !value.is_empty());
    if !has_username
        && (password.is_some_and(|value| !value.is_empty())
            || domain.is_some_and(|value| !value.is_empty()))
    {
        return Err(MountError::Other(
            "SMB username is required when password or domain is set".to_string(),
        ));
    }
    let credentials = if has_username {
        options.retain(|option| option != "guest");
        let credentials =
            CredentialFile::create(username, Some(password.unwrap_or_default()), domain)?;
        options.push(format!("credentials={}", credentials.path().display()));
        Some(credentials)
    } else {
        if !options.iter().any(|option| option == "guest") {
            options.push("guest".to_string());
        }
        None
    };
    Ok((options, credentials))
}

fn mount_cifs_with_options(
    source: &str,
    target: &str,
    options: &[String],
) -> Result<(), MountError> {
    let mut command = Command::new("mount");
    command.args(["-t", "cifs", source, target]);
    if !options.is_empty() {
        command.args(["-o", &options.join(",")]);
    }
    run_command(command, "mount -t cifs")
}

fn reusable_smb_options(options: &[String]) -> Vec<String> {
    options
        .iter()
        .filter(|option| {
            let key = option
                .split_once('=')
                .map_or(option.as_str(), |(key, _)| key);
            !matches!(
                key,
                "password"
                    | "pass"
                    | "credentials"
                    | "username"
                    | "user"
                    | "domain"
                    | "workgroup"
                    | "guest"
                    | "unc"
                    | "ip"
                    | "addr"
                    | "prefixpath"
                    | "user_id"
                    | "group_id"
            )
        })
        .cloned()
        .collect()
}

fn ensure_mount_target(target: &str) -> Result<(), MountError> {
    let path = Path::new(target);
    if !path.exists() {
        return Err(MountError::Other(format!(
            "target path does not exist: {target}"
        )));
    }
    if !path.is_dir() {
        return Err(MountError::Other(format!(
            "mount target is not a directory: {target}"
        )));
    }
    Ok(())
}

fn normalize_smb_source(source: &str) -> Result<String, MountError> {
    let source = source.trim();
    let normalized = source
        .strip_prefix("smb://")
        .map(|rest| format!("//{rest}"))
        .unwrap_or_else(|| source.to_string());
    let rest = normalized
        .strip_prefix("//")
        .ok_or_else(|| MountError::Other("SMB source must look like //server/share".to_string()))?;
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    if parts.next().is_none() || parts.next().is_none() {
        return Err(MountError::Other(
            "SMB source must include both server and share".to_string(),
        ));
    }
    Ok(normalized)
}

fn sanitized_smb_options(opts: Option<&str>) -> Result<Vec<String>, MountError> {
    let mut out = Vec::new();
    for option in opts.unwrap_or_default().split(',') {
        let option = option.trim();
        if option.is_empty() || option == "defaults" {
            continue;
        }
        let key = option.split_once('=').map_or(option, |(key, _)| key);
        if matches!(
            key,
            "password" | "pass" | "credentials" | "username" | "user" | "domain" | "workgroup"
        ) {
            return Err(MountError::Other(format!(
                "do not put {key}= in options; use the dedicated SMB form field"
            )));
        }
        if !out.iter().any(|existing| existing == option) {
            out.push(option.to_string());
        }
    }
    Ok(out)
}

fn run_command(mut command: Command, program: &str) -> Result<(), MountError> {
    let output = command.output().map_err(MountError::Io)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };
    Err(MountError::Command {
        program: program.to_string(),
        message,
    })
}

struct CredentialFile {
    path: PathBuf,
}

impl CredentialFile {
    fn create(
        username: Option<&str>,
        password: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Self, MountError> {
        let base = if Path::new("/run").is_dir() {
            Path::new("/run")
        } else {
            Path::new("/tmp")
        };
        for attempt in 0..100u32 {
            let path = base.join(format!(".mount-tui-cifs-{}-{attempt}", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    let credentials = Self { path };
                    if let Some(username) = username.filter(|value| !value.is_empty()) {
                        writeln!(file, "username={username}")?;
                    }
                    if let Some(password) = password {
                        writeln!(file, "password={password}")?;
                    }
                    if let Some(domain) = domain.filter(|value| !value.is_empty()) {
                        writeln!(file, "domain={domain}")?;
                    }
                    file.sync_all()?;
                    return Ok(credentials);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(MountError::Io(error)),
            }
        }
        Err(MountError::Other(
            "could not create a temporary SMB credentials file".to_string(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CredentialFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
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

    #[test]
    fn separates_mount_flags_from_filesystem_options() {
        let (flags, data) = prepare_mount_options(
            Some("defaults,ro,nodev,noexec,errors=remount-ro"),
            MountFlags::empty(),
        );

        assert!(flags.contains(MountFlags::RDONLY));
        assert!(flags.contains(MountFlags::NODEV));
        assert!(flags.contains(MountFlags::NOEXEC));
        assert_eq!(data, ["errors=remount-ro"]);
    }

    #[test]
    fn defaults_are_not_sent_to_ext4_as_mount_data() {
        let (flags, data) = prepare_mount_options(Some("defaults"), MountFlags::empty());

        assert!(flags.is_empty());
        assert!(data.is_empty());
    }

    #[test]
    fn normalizes_smb_urls() {
        assert_eq!(
            normalize_smb_source("smb://nas/media").unwrap(),
            "//nas/media"
        );
        assert_eq!(normalize_smb_source("//nas/media").unwrap(), "//nas/media");
        assert!(normalize_smb_source("nas").is_err());
        assert!(normalize_smb_source("//nas").is_err());
    }

    #[test]
    fn rejects_secrets_in_mount_options() {
        assert!(sanitized_smb_options(Some("rw,password=secret")).is_err());
        assert!(sanitized_smb_options(Some("credentials=/tmp/file")).is_err());
        assert!(sanitized_smb_options(Some("username=alice")).is_err());
        assert_eq!(
            sanitized_smb_options(Some("defaults,rw,rw,nodev")).unwrap(),
            ["rw", "nodev"]
        );
    }

    #[test]
    fn ownership_options_match_filesystem_semantics() {
        assert_eq!(
            ownership_options("cifs", 1000, 100),
            "uid=1000,gid=100,forceuid,forcegid,file_mode=0664,dir_mode=0775"
        );
        assert_eq!(
            ownership_options("vfat", 1000, 100),
            "uid=1000,gid=100,umask=022"
        );
    }

    #[test]
    fn replaces_existing_ownership_options() {
        let existing = vec![
            "rw".to_string(),
            "uid=0".to_string(),
            "gid=0".to_string(),
            "fmask=0177".to_string(),
            "iocharset=utf8".to_string(),
        ];
        assert_eq!(
            replace_ownership_options(&existing, "vfat", 1000, 100),
            ["rw", "iocharset=utf8", "uid=1000", "gid=100", "umask=022"]
        );
    }

    #[test]
    fn reusable_smb_options_remove_authentication_and_kernel_generated_values() {
        let existing = vec![
            "rw".to_string(),
            "username=alice".to_string(),
            "credentials=/run/secret".to_string(),
            "addr=192.0.2.1".to_string(),
            "vers=3.1.1".to_string(),
        ];
        assert_eq!(reusable_smb_options(&existing), ["rw", "vers=3.1.1"]);
    }
}
