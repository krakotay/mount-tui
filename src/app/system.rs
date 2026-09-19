use super::*;

pub(super) fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

pub(super) fn device_info_lines(entry: &UiEntry) -> Vec<String> {
    if entry.fstab_line.is_some() {
        return vec![
            format!("fstab line: {}", entry.fstab_line.unwrap_or_default() + 1),
            format!("Source: {}", entry.source),
            format!(
                "Target: {}",
                entry.mount_points.first().map_or("-", String::as_str)
            ),
            format!("Options: {}", display_mount_options(&entry.options)),
        ];
    }
    if entry
        .fstype
        .as_deref()
        .is_some_and(|fstype| is_smb_fstype(fstype) || is_sshfs_fstype(fstype))
    {
        let mut out = vec![format!("Share: {}", entry.source)];
        out.extend(
            entry
                .mount_points
                .iter()
                .map(|target| format!("Target: {target}")),
        );
        if !entry.options.is_empty() {
            out.push(format!(
                "Options: {}",
                display_mount_options(&entry.options)
            ));
        }
        return out;
    }
    let mut out = Vec::new();
    let dev_path = entry_device_path(entry);
    out.push(format!("Device: {}", dev_path));
    if let Some(label) = find_label_for_device(&dev_path) {
        out.push(format!("Label: {}", label));
    }
    if let Some(fstype) = detected_fstype(&dev_path) {
        out.push(format!("FS Type: {}", fstype));
        let encrypted = fstype == "crypto_LUKS";
        if encrypted {
            out.push("Encrypted: yes (LUKS)".to_string());
        }
    }

    if let Some(data) = udev_data_for_device(&dev_path) {
        let keys = [
            "ID_FS_LABEL",
            "ID_FS_UUID",
            "ID_FS_USAGE",
            "ID_FS_VERSION",
            "ID_PART_ENTRY_NAME",
            "ID_PART_ENTRY_UUID",
            "ID_PART_ENTRY_TYPE",
            "ID_MODEL",
            "ID_VENDOR",
            "ID_SERIAL_SHORT",
        ];
        for key in keys {
            if let Some(val) = data.get(key) {
                out.push(format!("{}: {}", key, val));
            }
        }
        if let Some(usage) = data.get("ID_FS_USAGE")
            && usage == "crypto"
            && !out.iter().any(|l| l.starts_with("Encrypted:"))
        {
            out.push("Encrypted: yes".to_string());
        }
    } else {
        out.push("udev: no info".to_string());
    }

    if out.len() == 1 {
        out.push("No extra info available".to_string());
    }
    out
}

pub(super) fn display_mount_options(options: &[String]) -> String {
    options
        .iter()
        .map(|option| {
            let key = option
                .split_once('=')
                .map_or(option.as_str(), |(key, _)| key);
            if matches!(key, "password" | "pass" | "credentials") {
                format!("{key}=<hidden>")
            } else {
                option.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn entry_device_path(entry: &UiEntry) -> String {
    if entry.source.starts_with("/dev/") {
        return entry.source.clone();
    }
    let token = entry.name.split_whitespace().next().unwrap_or(&entry.name);
    format!("/dev/{}", token)
}

pub(super) fn udev_data_for_device(dev_path: &str) -> Option<HashMap<String, String>> {
    let dev_name = canonical_device_name(dev_path)?;
    let dev_file = format!("/sys/class/block/{}/dev", dev_name);
    let dev_id = fs::read_to_string(dev_file).ok()?;
    let dev_id = dev_id.trim();
    let udev_path = format!("/run/udev/data/b{}", dev_id);
    let data = fs::read_to_string(udev_path).ok()?;
    let mut map = HashMap::new();
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("E:")
            && let Some((k, v)) = rest.split_once('=')
        {
            map.insert(k.to_string(), v.to_string());
        }
    }
    Some(map)
}

pub(super) fn default_fstype(dev_path: &str) -> Option<String> {
    detected_fstype(dev_path).map(|fstype| preferred_mount_fstype(&fstype).to_string())
}

pub(super) fn detected_fstype(dev_path: &str) -> Option<String> {
    let dev_name = canonical_device_name(dev_path)?;
    let dev_file = format!("/sys/class/block/{}/dev", dev_name);
    let dev_id = fs::read_to_string(dev_file).ok()?;
    let dev_id = dev_id.trim();
    let udev_path = format!("/run/udev/data/b{}", dev_id);
    let data = fs::read_to_string(udev_path).ok()?;
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("E:ID_FS_TYPE=") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

pub(super) fn preferred_mount_fstype(detected: &str) -> &str {
    match detected {
        "ntfs" | "ntfs3" | "ntfs-3g" => "ntfs-3g",
        other => other,
    }
}

pub(super) fn is_ntfs_driver(fstype: &str) -> bool {
    matches!(fstype, "ntfs" | "ntfs3" | "ntfs-3g")
}

pub(super) fn toggled_ntfs_driver(fstype: &str) -> &'static str {
    if fstype == "ntfs3" {
        "ntfs-3g"
    } else {
        "ntfs3"
    }
}

pub(super) fn canonical_device_name(dev_path: &str) -> Option<String> {
    let canon = fs::canonicalize(dev_path).ok()?;
    canon.file_name().map(|s| s.to_string_lossy().to_string())
}

pub(super) fn default_mount_opts(fstype: &str) -> String {
    if fstype.is_empty() {
        return String::new();
    }
    match fstype {
        "vfat" | "exfat" | "ntfs" | "ntfs3" | "ntfs-3g" => {
            let (uid, gid) = effective_user_ids();
            format!("rw,{}", ownership_options(fstype, uid, gid))
        }
        _ => "defaults".to_string(),
    }
}

pub(super) fn effective_user_ids() -> (u32, u32) {
    let uid = env::var("SUDO_UID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .or_else(|| {
            env::var("PKEXEC_UID")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
        })
        .or_else(|| {
            env::var("DOAS_USER")
                .ok()
                .and_then(|name| passwd_entry_by_name(&name))
                .map(|(_, uid, _)| uid)
        })
        .unwrap_or_else(|| rustix::process::getuid().as_raw());
    let gid = env::var("SUDO_GID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .or_else(|| passwd_entry_by_uid(uid).map(|(_, _, gid)| gid))
        .unwrap_or_else(|| rustix::process::getgid().as_raw());
    (uid, gid)
}

pub(super) fn effective_user_name() -> String {
    for key in ["SUDO_USER", "DOAS_USER"] {
        if let Ok(name) = env::var(key)
            && !name.is_empty()
        {
            return name;
        }
    }
    let (uid, _) = effective_user_ids();
    passwd_entry_by_uid(uid)
        .map(|(name, _, _)| name)
        .or_else(|| env::var("USER").ok())
        .unwrap_or_else(|| uid.to_string())
}

pub(super) fn effective_user_home() -> std::path::PathBuf {
    let (uid, _) = effective_user_ids();
    fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            (fields.len() >= 6 && fields[2].parse::<u32>().ok() == Some(uid))
                .then(|| std::path::PathBuf::from(fields[5]))
        })
        .or_else(|| env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

pub(super) fn prepare_user_known_hosts() -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let home = effective_user_home();
    let ssh_dir = home.join(".ssh");
    let created_dir = !ssh_dir.exists();
    fs::create_dir_all(&ssh_dir)?;
    if created_dir {
        fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))?;
    }

    let known_hosts = ssh_dir.join("known_hosts");
    let created_file = !known_hosts.exists();
    if created_file {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&known_hosts)?;
    }

    let (uid, gid) = effective_user_ids();
    if is_root() && uid != 0 {
        if created_dir {
            rustix::fs::chown(
                &ssh_dir,
                Some(rustix::process::Uid::from_raw(uid)),
                Some(rustix::process::Gid::from_raw(gid)),
            )?;
        }
        if created_file {
            rustix::fs::chown(
                &known_hosts,
                Some(rustix::process::Uid::from_raw(uid)),
                Some(rustix::process::Gid::from_raw(gid)),
            )?;
        }
    }
    Ok(())
}

pub(super) fn prepare_ssh_mount_target(target: &Path) -> Result<(), mount_tui::MountError> {
    let (uid, gid) = effective_user_ids();
    if is_root() && uid != 0 {
        rustix::fs::chown(
            target,
            Some(rustix::process::Uid::from_raw(uid)),
            Some(rustix::process::Gid::from_raw(gid)),
        )?;
    }
    Ok(())
}

pub(super) fn passwd_entries() -> impl Iterator<Item = (String, u32, u32)> {
    fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?.to_string();
            fields.next()?;
            let uid = fields.next()?.parse().ok()?;
            let gid = fields.next()?.parse().ok()?;
            Some((name, uid, gid))
        })
        .collect::<Vec<_>>()
        .into_iter()
}

pub(super) fn passwd_entry_by_name(name: &str) -> Option<(String, u32, u32)> {
    passwd_entries().find(|entry| entry.0 == name)
}

pub(super) fn passwd_entry_by_uid(uid: u32) -> Option<(String, u32, u32)> {
    passwd_entries().find(|entry| entry.1 == uid)
}

pub(super) fn find_label_for_device(dev_path: &str) -> Option<String> {
    let canon = fs::canonicalize(dev_path).ok()?;
    for entry in fs::read_dir("/dev/disk/by-label").ok()? {
        let entry = entry.ok()?;
        let label = entry.file_name().to_string_lossy().to_string();
        let link = fs::read_link(entry.path()).ok()?;
        let full = if link.is_absolute() {
            link
        } else {
            entry.path().parent()?.join(link)
        };
        if let Ok(target) = fs::canonicalize(full)
            && target == canon
        {
            return Some(label);
        }
    }
    None
}

pub(super) fn sanitize_mount_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "_".to_string() } else { out }
}

pub(super) fn reexec_with_sudo() -> anyhow::Result<()> {
    disable_raw_mode().ok();
    execute!(io::stdout(), LeaveAlternateScreen).ok();
    let exe = env::current_exe()?;
    let args: Vec<String> = env::args().skip(1).collect();
    let status = Command::new("sudo").arg(exe).args(args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}
