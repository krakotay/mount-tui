use super::*;

pub(super) fn default_mount_fields(state: &AppState) -> (String, String, String, String) {
    if let Some(entry) = state.entries.get(state.selected) {
        let source = if entry.source.starts_with("/dev/") {
            entry.source.clone()
        } else {
            entry.name.clone()
        };
        let target = default_mount_target(&source);
        let fstype = default_fstype(&source).unwrap_or_default();
        let opts = default_mount_opts(&fstype);
        (source, target, fstype, opts)
    } else {
        (String::new(), String::new(), String::new(), String::new())
    }
}

pub(super) fn mount_form_value<'a>(
    source: &'a str,
    target: &'a str,
    fstype: &'a str,
    opts: &'a str,
    field: usize,
) -> &'a str {
    match field {
        0 => source,
        1 => target,
        2 => fstype,
        3 => opts,
        _ => "",
    }
}

pub(super) fn mount_form_value_mut<'a>(
    source: &'a mut String,
    target: &'a mut String,
    fstype: &'a mut String,
    opts: &'a mut String,
    field: usize,
) -> &'a mut String {
    match field {
        0 => source,
        1 => target,
        2 => fstype,
        3 => opts,
        _ => opts,
    }
}

pub(super) fn smb_form_value<'a>(
    source: &'a str,
    target: &'a str,
    username: &'a str,
    password: &'a str,
    domain: &'a str,
    opts: &'a str,
    field: usize,
) -> &'a str {
    match field {
        0 => source,
        1 => target,
        2 => username,
        3 => password,
        4 => domain,
        5 => opts,
        _ => "",
    }
}

pub(super) fn smb_form_value_mut<'a>(
    source: &'a mut String,
    target: &'a mut String,
    username: &'a mut String,
    password: &'a mut String,
    domain: &'a mut String,
    opts: &'a mut String,
    field: usize,
) -> &'a mut String {
    match field {
        0 => source,
        1 => target,
        2 => username,
        3 => password,
        4 => domain,
        5 => opts,
        _ => opts,
    }
}

pub(super) fn char_to_byte_index(value: &str, cursor: usize) -> usize {
    value
        .char_indices()
        .nth(cursor)
        .map_or(value.len(), |(index, _)| index)
}

pub(super) fn insert_char_at(value: &mut String, cursor: &mut usize, ch: char) {
    let byte_index = char_to_byte_index(value, *cursor);
    value.insert(byte_index, ch);
    *cursor += 1;
}

pub(super) fn remove_char_before(value: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    *cursor -= 1;
    let start = char_to_byte_index(value, *cursor);
    let end = char_to_byte_index(value, *cursor + 1);
    value.replace_range(start..end, "");
}

pub(super) fn remove_char_at(value: &mut String, cursor: usize) {
    let start = char_to_byte_index(value, cursor);
    let end = char_to_byte_index(value, cursor + 1);
    if start < end {
        value.replace_range(start..end, "");
    }
}

pub(super) fn mount_options_are_read_only(options: &str) -> bool {
    options.split(',').any(|option| option.trim() == "ro")
}

pub(super) fn mount_options_are_forced(options: &str) -> bool {
    options.split(',').any(|option| option.trim() == "force")
}

pub(super) fn read_only_mount_options(options: &str) -> String {
    std::iter::once("ro")
        .chain(
            options
                .split(',')
                .map(str::trim)
                .filter(|option| !option.is_empty() && !matches!(*option, "rw" | "ro")),
        )
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn force_mount_options(options: &str) -> String {
    std::iter::once("force")
        .chain(
            options
                .split(',')
                .map(str::trim)
                .filter(|option| !option.is_empty() && *option != "force"),
        )
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn without_force_mount_options(options: &str) -> String {
    options
        .split(',')
        .map(str::trim)
        .filter(|option| !option.is_empty() && *option != "force")
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct MountRetry {
    pub(super) label: &'static str,
    pub(super) options: String,
    pub(super) force: bool,
}

pub(super) fn mount_retry_options(fstype: &str, failed_options: &str) -> Vec<MountRetry> {
    let read_only = mount_options_are_read_only(failed_options);
    let forced = mount_options_are_forced(failed_options);

    if fstype != "ntfs3" {
        return (!read_only)
            .then(|| MountRetry {
                label: "read-only",
                options: read_only_mount_options(failed_options),
                force: false,
            })
            .into_iter()
            .collect();
    }

    match (read_only, forced) {
        (false, false) => vec![
            MountRetry {
                label: "read-only",
                options: read_only_mount_options(failed_options),
                force: false,
            },
            MountRetry {
                label: "read-write with force (DANGER)",
                options: force_mount_options(failed_options),
                force: true,
            },
        ],
        (false, true) => vec![MountRetry {
            label: "read-only",
            options: read_only_mount_options(&without_force_mount_options(failed_options)),
            force: false,
        }],
        (true, false) => vec![MountRetry {
            label: "read-only with force (DANGER)",
            options: force_mount_options(failed_options),
            force: true,
        }],
        (true, true) => Vec::new(),
    }
}

pub(super) fn retry_prompt(retries: &[MountRetry]) -> String {
    let safe = retries.iter().find(|retry| !retry.force);
    let forced = retries.iter().find(|retry| retry.force);
    match (safe, forced) {
        (Some(safe), Some(forced)) => {
            format!("Enter={}   F={}   Esc=cancel", safe.label, forced.label)
        }
        (Some(safe), None) => format!("Enter={}   Esc=cancel", safe.label),
        (None, Some(forced)) => format!("F={}   Esc=cancel", forced.label),
        (None, None) => "Esc=cancel".to_string(),
    }
}

pub(super) fn default_smb_fields() -> (String, String, String, String) {
    let source = String::new();
    let target = default_smb_target(&source);
    let username = effective_user_name();
    let (uid, gid) = effective_user_ids();
    let opts = format!(
        "rw,nosuid,nodev,{},iocharset=utf8",
        ownership_options("cifs", uid, gid)
    );
    (source, target, username, opts)
}

pub(super) fn smb_reconnect_fields(entry: &UiEntry) -> (String, String, String, String, String) {
    let username = mount_option_value(&entry.options, &["username", "user"])
        .map(str::to_string)
        .unwrap_or_else(|| {
            if entry.options.iter().any(|option| option == "guest") {
                String::new()
            } else {
                effective_user_name()
            }
        });
    let domain = mount_option_value(&entry.options, &["domain", "workgroup"])
        .unwrap_or_default()
        .to_string();
    let mut options = vec!["rw".to_string(), "nosuid".to_string(), "nodev".to_string()];
    for option in &entry.options {
        let key = option
            .split_once('=')
            .map_or(option.as_str(), |(key, _)| key);
        if matches!(
            key,
            "vers"
                | "sec"
                | "cache"
                | "iocharset"
                | "noperm"
                | "perm"
                | "seal"
                | "sign"
                | "multichannel"
                | "noserverino"
                | "serverino"
                | "actimeo"
                | "echo_interval"
                | "closetimeo"
                | "soft"
                | "hard"
                | "nounix"
                | "unix"
                | "nobrl"
                | "mfsymlinks"
                | "rsize"
                | "wsize"
        ) && !options.contains(option)
        {
            options.push(option.clone());
        }
    }
    let (uid, gid) = effective_user_ids();
    options.extend(
        ownership_options("cifs", uid, gid)
            .split(',')
            .map(str::to_string),
    );
    (
        entry.source.clone(),
        entry
            .mount_points
            .first()
            .cloned()
            .unwrap_or_else(|| default_smb_target(&entry.source)),
        username,
        domain,
        options.join(","),
    )
}

pub(super) fn mount_option_value<'a>(options: &'a [String], keys: &[&str]) -> Option<&'a str> {
    options.iter().find_map(|option| {
        let (key, value) = option.split_once('=')?;
        keys.contains(&key).then_some(value)
    })
}

pub(super) fn maybe_update_smb_target(source: &str, target: &mut String, field: usize) {
    if field == 0 && *target == default_smb_target("") && !source.trim().is_empty() {
        *target = default_smb_target(source);
    }
}

pub(super) fn is_valid_smb_source(source: &str) -> bool {
    let source = source.trim();
    let rest = source
        .strip_prefix("//")
        .or_else(|| source.strip_prefix("smb://"));
    let Some(rest) = rest else {
        return false;
    };
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    parts.next().is_some() && parts.next().is_some()
}

pub(super) fn smb_form_error(
    source: &str,
    target: &str,
    username: &str,
    password: &str,
    domain: &str,
) -> Option<(usize, &'static str)> {
    if source.trim().is_empty() {
        return Some((0, "SMB resource is required; use //server/share"));
    }
    if !is_valid_smb_source(source) {
        return Some((0, "Invalid SMB resource; expected //server/share"));
    }
    if target.trim().is_empty() {
        return Some((1, "Mount target is required"));
    }
    if !Path::new(target).is_absolute() {
        return Some((1, "Mount target must be an absolute path"));
    }
    if username.trim().is_empty() && (!password.is_empty() || !domain.trim().is_empty()) {
        return Some((2, "Username is required when password or domain is set"));
    }
    None
}

pub(super) fn default_smb_target(source: &str) -> String {
    let share = source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("smb");
    format!(
        "/media/{}/{}",
        sanitize_mount_name(&effective_user_name()),
        sanitize_mount_name(share)
    )
}

pub(super) fn default_mount_target(source: &str) -> String {
    let name = find_label_for_device(source).unwrap_or_else(|| {
        source
            .strip_prefix("/dev/")
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .unwrap_or(source)
            .to_string()
    });
    media_mount_target(&effective_user_name(), &name)
}

pub(super) fn media_mount_target(user: &str, name: &str) -> String {
    format!(
        "/media/{}/{}",
        sanitize_mount_name(user),
        sanitize_mount_name(name)
    )
}
