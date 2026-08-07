use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mount_tui::{
    BlockDevice, MountEntry, MountManager, UserAccessMethod, is_smb_fstype, ownership_options,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState},
};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};
use std::{env, process::Command};
const VIRTUAL_PREFIXES: &[&str] = &["loop", "ram", "zram", "fd"];
const PSEUDO_FSTYPES: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "cgroup2",
    "pstore",
    "efivarfs",
    "bpf",
    "autofs",
    "debugfs",
    "mqueue",
    "hugetlbfs",
    "tracefs",
    "fusectl",
    "configfs",
    "overlay",
];

#[derive(Debug, Clone)]
struct UiEntry {
    name: String,
    kind: String,
    size_bytes: Option<u64>,
    mount_points: Vec<String>,
    fstype: Option<String>,
    source: String,
    removable: bool,
    model: Option<String>,
    vendor: Option<String>,
    options: Vec<String>,
    ownership: Ownership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Unmounted,
    CurrentUser,
    Other(u32),
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Filter,
}

#[derive(Debug)]
struct AppState {
    mounts: Vec<MountEntry>,
    devices: Vec<BlockDevice>,
    entries: Vec<UiEntry>,
    table_state: TableState,
    selected: usize,
    filter: String,
    input_mode: InputMode,
    show_pseudo: bool,
    show_smb: bool,
    show_partitions: bool,
    show_disks: bool,
    status: String,
    last_refresh: SystemTime,
    modal: Modal,
    info_extra: Vec<String>,
    info_extra_visible: bool,
}

#[derive(Debug, Clone)]
enum Modal {
    None,
    NeedRoot,
    ConfirmUnmount {
        mount_points: Vec<String>,
        selected: usize,
    },
    UnmountError {
        target: String,
        error: String,
        processes: Vec<BusyProcess>,
    },
    UserAccess {
        mount_points: Vec<String>,
        selected: usize,
        fstype: String,
    },
    MountForm {
        source: String,
        target: String,
        fstype: String,
        opts: String,
        field: usize,
        cursor: usize,
    },
    ConfirmMountRetry {
        source: String,
        target: String,
        fstype: String,
        opts: String,
        error: String,
    },
    SmbForm {
        source: String,
        target: String,
        username: String,
        password: String,
        domain: String,
        opts: String,
        field: usize,
        cursor: usize,
        previous_mount: Option<MountEntry>,
    },
}
mod editor;
mod forms;
mod processes;
mod system;
mod ui;

use editor::*;
use forms::*;
use processes::*;
use system::*;
use ui::*;

#[cfg(test)]
mod tests;

pub fn run() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = init_state()?;

    let tick_rate = Duration::from_millis(250);
    loop {
        draw_ui(&mut terminal, &mut state)?;

        if event::poll(tick_rate)? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(&mut state, key)? {
                        break;
                    }
                }
                Event::Mouse(_) => {}
                Event::Resize(_, _) => {}
                Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn init_state() -> anyhow::Result<AppState> {
    let mounts = MountManager::list_mounts().unwrap_or_default();
    let devices = MountManager::list_block_devices().unwrap_or_default();
    let mut state = AppState {
        mounts,
        devices,
        entries: Vec::new(),
        table_state: TableState::default(),
        selected: 0,
        filter: String::new(),
        input_mode: InputMode::Normal,
        show_pseudo: false,
        show_smb: true,
        show_partitions: true,
        show_disks: true,
        status: String::new(),
        last_refresh: SystemTime::now(),
        modal: Modal::None,
        info_extra: Vec::new(),
        info_extra_visible: false,
    };
    rebuild_entries(&mut state);
    Ok(state)
}

fn rebuild_entries(state: &mut AppState) {
    state.entries = build_entries(
        &state.mounts,
        &state.devices,
        state.show_pseudo,
        state.show_smb,
        state.show_disks,
        state.show_partitions,
        &state.filter,
    );
    if state.selected >= state.entries.len() {
        state.selected = state.entries.len().saturating_sub(1);
    }
    if !state.entries.is_empty() {
        state.table_state.select(Some(state.selected));
    } else {
        state.table_state.select(None);
    }
    state.info_extra.clear();
    state.info_extra_visible = false;
}

fn handle_key(state: &mut AppState, key: KeyEvent) -> anyhow::Result<bool> {
    if !matches!(state.modal, Modal::None) {
        return handle_modal_key(state, key);
    }

    match state.input_mode {
        InputMode::Filter => {
            match key.code {
                KeyCode::Esc => {
                    state.input_mode = InputMode::Normal;
                }
                KeyCode::Enter => {
                    state.input_mode = InputMode::Normal;
                }
                KeyCode::Backspace => {
                    state.filter.pop();
                    rebuild_entries(state);
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.filter.push(c);
                    rebuild_entries(state);
                }
                _ => {}
            }
            return Ok(false);
        }
        InputMode::Normal => {}
    }

    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('r') => {
            state.mounts = MountManager::list_mounts().unwrap_or_default();
            state.devices = MountManager::list_block_devices().unwrap_or_default();
            state.last_refresh = SystemTime::now();
            rebuild_entries(state);
            state.status = "Refreshed".to_string();
        }
        KeyCode::Char('f') => {
            state.input_mode = InputMode::Filter;
        }
        KeyCode::Char('p') => {
            state.show_pseudo = !state.show_pseudo;
            rebuild_entries(state);
        }
        KeyCode::Char('d') => {
            state.show_disks = !state.show_disks;
            rebuild_entries(state);
        }
        KeyCode::Char('t') => {
            state.show_partitions = !state.show_partitions;
            rebuild_entries(state);
        }
        KeyCode::Up => select_prev(state, 1),
        KeyCode::Down => select_next(state, 1),
        KeyCode::PageUp => select_prev(state, 10),
        KeyCode::PageDown => select_next(state, 10),
        KeyCode::Home => select_first(state),
        KeyCode::End => select_last(state),
        KeyCode::Char('u') => {
            if let Some(entry) = state.entries.get(state.selected) {
                if !entry.mount_points.is_empty() {
                    if !is_root() {
                        state.modal = Modal::NeedRoot;
                    } else {
                        state.modal = Modal::ConfirmUnmount {
                            mount_points: entry.mount_points.clone(),
                            selected: 0,
                        };
                    }
                } else {
                    state.status = "Nothing mounted here".to_string();
                }
            }
        }
        KeyCode::Char('m') => {
            if let Some(entry) = state.entries.get(state.selected) {
                if !entry.mount_points.is_empty() {
                    state.status = "Already mounted (use unmount)".to_string();
                } else if !is_root() {
                    state.modal = Modal::NeedRoot;
                } else {
                    let (source, target, fstype, opts) = default_mount_fields(state);
                    let cursor = source.chars().count();
                    state.modal = Modal::MountForm {
                        source,
                        target,
                        fstype,
                        opts,
                        field: 0,
                        cursor,
                    };
                }
            }
        }
        KeyCode::Char('s') => {
            state.show_smb = !state.show_smb;
            rebuild_entries(state);
            state.status = if state.show_smb {
                "SMB mounts are visible".to_string()
            } else {
                "SMB mounts are hidden".to_string()
            };
        }
        KeyCode::Char('n') => {
            if !is_root() {
                state.modal = Modal::NeedRoot;
            } else {
                let (source, target, username, opts) = default_smb_fields();
                state.modal = Modal::SmbForm {
                    cursor: source.chars().count(),
                    source,
                    target,
                    username,
                    password: String::new(),
                    domain: String::new(),
                    opts,
                    field: 0,
                    previous_mount: None,
                };
            }
        }
        KeyCode::Char('a') => {
            if let Some(entry) = state.entries.get(state.selected) {
                if entry.mount_points.is_empty() {
                    state.status = "Mount the filesystem first".to_string();
                } else if entry.ownership == Ownership::CurrentUser {
                    state.status = format!(
                        "Already owned by {} ({})",
                        effective_user_name(),
                        effective_user_ids().0
                    );
                } else if effective_user_ids().0 == 0 {
                    state.status =
                        "No regular user detected; start mount-tui via sudo as that user"
                            .to_string();
                } else if !is_root() {
                    state.modal = Modal::NeedRoot;
                } else if entry.fstype.as_deref().is_some_and(is_smb_fstype) {
                    let (source, target, username, domain, opts) = smb_reconnect_fields(entry);
                    let previous_mount = state
                        .mounts
                        .iter()
                        .find(|mount| mount.target == target)
                        .cloned();
                    state.modal = Modal::SmbForm {
                        source,
                        target,
                        username,
                        password: String::new(),
                        domain,
                        opts,
                        field: 3,
                        cursor: 0,
                        previous_mount,
                    };
                } else {
                    let mount_points = mount_points_needing_access(
                        &state.mounts,
                        &entry.mount_points,
                        effective_user_ids().0,
                    );
                    state.modal = Modal::UserAccess {
                        mount_points,
                        selected: 0,
                        fstype: entry.fstype.clone().unwrap_or_default(),
                    };
                }
            }
        }
        KeyCode::Char('i') => {
            if let Some(entry) = state.entries.get(state.selected) {
                state.info_extra = device_info_lines(entry);
                state.info_extra_visible = !state.info_extra_visible;
            }
        }
        _ => {}
    }

    Ok(false)
}

fn handle_modal_key(state: &mut AppState, key: KeyEvent) -> anyhow::Result<bool> {
    match &mut state.modal {
        Modal::NeedRoot => match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => {
                reexec_with_sudo()?;
            }
            KeyCode::Esc => state.modal = Modal::None,
            _ => {}
        },
        Modal::UserAccess {
            mount_points,
            selected,
            fstype: _,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Up => {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
            KeyCode::Down => {
                if *selected + 1 < mount_points.len() {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(target) = mount_points.get(*selected).cloned() {
                    let (uid, gid) = effective_user_ids();
                    let mount = state
                        .mounts
                        .iter()
                        .find(|mount| mount.target == target)
                        .cloned();
                    let Some(mount) = mount else {
                        state.status = format!("Mount disappeared: {target}");
                        state.modal = Modal::None;
                        return Ok(false);
                    };
                    match MountManager::make_user_accessible(
                        &mount.source,
                        &target,
                        &mount.fstype,
                        &mount.options,
                        uid,
                        gid,
                    ) {
                        Ok(UserAccessMethod::Reconnected) => {
                            state.status = format!(
                                "Reconnected {target} for {} ({uid}:{gid})",
                                effective_user_name()
                            );
                        }
                        Ok(UserAccessMethod::ChangedOwner) => {
                            state.status = format!(
                                "Mount root {target} now belongs to {} ({uid}:{gid}); child permissions are unchanged",
                                effective_user_name()
                            );
                        }
                        Err(error) => state.status = format!("user access failed: {error}"),
                    }
                    state.mounts = MountManager::list_mounts().unwrap_or_default();
                    rebuild_entries(state);
                }
                state.modal = Modal::None;
            }
            _ => {}
        },
        Modal::ConfirmUnmount {
            mount_points,
            selected,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Up => {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
            KeyCode::Down => {
                if *selected + 1 < mount_points.len() {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(target) = mount_points.get(*selected) {
                    let target = target.clone();
                    match MountManager::umount(&target) {
                        Ok(()) => {
                            state.mounts = MountManager::list_mounts().unwrap_or_default();
                            state.devices = MountManager::list_block_devices().unwrap_or_default();
                            rebuild_entries(state);
                            state.status = format!("Unmounted {target}");
                        }
                        Err(error) => {
                            let message = concise_mount_error(&error);
                            let processes = if is_resource_busy(&error) {
                                processes_using_mount(&target)
                            } else {
                                Vec::new()
                            };
                            state.status = if processes.is_empty() {
                                format!("Unmount {target} failed: {message}")
                            } else {
                                let pids = processes
                                    .iter()
                                    .map(|process| process.pid.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                format!("Unmount {target} failed: {message}; PIDs: {pids}")
                            };
                            state.modal = Modal::UnmountError {
                                target,
                                error: message,
                                processes,
                            };
                            return Ok(false);
                        }
                    }
                }
                state.modal = Modal::None;
            }
            _ => {}
        },
        Modal::UnmountError { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter => state.modal = Modal::None,
            _ => {}
        },
        Modal::MountForm {
            source,
            target,
            fstype,
            opts,
            field,
            cursor,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Tab => {
                *field = (*field + 1) % 4;
                *cursor = mount_form_value(source, target, fstype, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::BackTab | KeyCode::Up => {
                *field = field.saturating_sub(1);
                *cursor = mount_form_value(source, target, fstype, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::Down => {
                *field = (*field + 1).min(3);
                *cursor = mount_form_value(source, target, fstype, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if *field == 2 && is_ntfs_driver(fstype) =>
            {
                *fstype = toggled_ntfs_driver(fstype).to_string();
                *cursor = fstype.chars().count();
            }
            KeyCode::Enter => {
                if *field < 3 {
                    *field += 1;
                    *cursor = mount_form_value(source, target, fstype, opts, *field)
                        .chars()
                        .count();
                } else {
                    if !Path::new(target.as_str()).exists()
                        && let Err(e) = fs::create_dir_all(target.as_str())
                    {
                        state.status = format!("mkdir failed: {e}");
                        state.modal = Modal::None;
                        return Ok(false);
                    }
                    match MountManager::mount(
                        source,
                        target,
                        if fstype.is_empty() {
                            None
                        } else {
                            Some(fstype.as_str())
                        },
                        if opts.is_empty() {
                            None
                        } else {
                            Some(opts.as_str())
                        },
                        rustix::mount::MountFlags::empty(),
                    ) {
                        Ok(()) => {
                            state.mounts = MountManager::list_mounts().unwrap_or_default();
                            state.devices = MountManager::list_block_devices().unwrap_or_default();
                            rebuild_entries(state);
                            state.status = "Mounted".to_string();
                        }
                        Err(e) => {
                            state.status = format!("mount failed: {e}");
                            if !mount_retry_options(fstype, opts).is_empty() {
                                state.modal = Modal::ConfirmMountRetry {
                                    source: source.clone(),
                                    target: target.clone(),
                                    fstype: fstype.clone(),
                                    opts: opts.clone(),
                                    error: e.to_string(),
                                };
                                return Ok(false);
                            }
                        }
                    }
                    state.modal = Modal::None;
                }
            }
            _ => {
                handle_line_editor_key(
                    mount_form_value_mut(source, target, fstype, opts, *field),
                    cursor,
                    key,
                );
            }
        },
        Modal::ConfirmMountRetry {
            source,
            target,
            fstype,
            opts,
            error: _,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Enter | KeyCode::Char('f' | 'F') => {
                let retries = mount_retry_options(fstype, opts);
                let selected = match key.code {
                    KeyCode::Char('f' | 'F') => retries.iter().find(|retry| retry.force),
                    _ => retries.iter().find(|retry| !retry.force),
                };
                let Some(selected) = selected else {
                    return Ok(false);
                };
                let retry_opts = selected.options.clone();
                let retry_label = selected.label;
                let mounted_target = target.clone();
                match MountManager::mount(
                    source,
                    target,
                    if fstype.is_empty() {
                        None
                    } else {
                        Some(fstype.as_str())
                    },
                    Some(retry_opts.as_str()),
                    rustix::mount::MountFlags::empty(),
                ) {
                    Ok(()) => {
                        state.mounts = MountManager::list_mounts().unwrap_or_default();
                        state.devices = MountManager::list_block_devices().unwrap_or_default();
                        rebuild_entries(state);
                        state.status = format!("Mounted {mounted_target} {retry_label}");
                        state.modal = Modal::None;
                    }
                    Err(error) => {
                        state.status = format!("{retry_label} mount failed: {error}");
                        if mount_retry_options(fstype, &retry_opts).is_empty() {
                            state.modal = Modal::None;
                        } else {
                            state.modal = Modal::ConfirmMountRetry {
                                source: source.clone(),
                                target: target.clone(),
                                fstype: fstype.clone(),
                                opts: retry_opts,
                                error: error.to_string(),
                            };
                        }
                    }
                }
            }
            _ => {}
        },
        Modal::SmbForm {
            source,
            target,
            username,
            password,
            domain,
            opts,
            field,
            cursor,
            previous_mount,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Tab => {
                maybe_update_smb_target(source, target, *field);
                *field = (*field + 1) % 6;
                *cursor = smb_form_value(source, target, username, password, domain, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::BackTab | KeyCode::Up => {
                *field = field.saturating_sub(1);
                *cursor = smb_form_value(source, target, username, password, domain, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::Down => {
                maybe_update_smb_target(source, target, *field);
                *field = (*field + 1).min(5);
                *cursor = smb_form_value(source, target, username, password, domain, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::Enter => {
                if *field < 5 {
                    if let Some((invalid_field, message)) =
                        smb_form_error(source, target, username, password, domain)
                        && invalid_field == *field
                    {
                        state.status = message.to_string();
                        return Ok(false);
                    }
                    maybe_update_smb_target(source, target, *field);
                    *field += 1;
                    *cursor =
                        smb_form_value(source, target, username, password, domain, opts, *field)
                            .chars()
                            .count();
                } else {
                    if let Some((invalid_field, message)) =
                        smb_form_error(source, target, username, password, domain)
                    {
                        *field = invalid_field;
                        *cursor = smb_form_value(
                            source, target, username, password, domain, opts, *field,
                        )
                        .chars()
                        .count();
                        state.status = message.to_string();
                        return Ok(false);
                    }
                    let mounted_source = source.clone();
                    let reconnecting = previous_mount.is_some();
                    if let Some(previous) = previous_mount.as_ref()
                        && (previous.source != *source || previous.target != *target)
                    {
                        state.status =
                            "Share and target cannot be changed while reconnecting; use n for a new mount"
                                .to_string();
                        return Ok(false);
                    }
                    if !Path::new(target.as_str()).exists()
                        && let Err(error) = fs::create_dir_all(target.as_str())
                    {
                        state.status = format!("mkdir failed: {error}");
                        state.modal = Modal::None;
                        return Ok(false);
                    }
                    let username_arg = (!username.is_empty()).then_some(username.as_str());
                    let password_arg = username_arg.map(|_| password.as_str());
                    let domain_arg =
                        username_arg.and_then(|_| (!domain.is_empty()).then_some(domain.as_str()));
                    let opts_arg = (!opts.is_empty()).then_some(opts.as_str());
                    let result = if let Some(previous) = previous_mount.as_ref() {
                        MountManager::reconnect_smb(
                            source,
                            target,
                            username_arg,
                            password_arg,
                            domain_arg,
                            opts_arg,
                            &previous.options,
                        )
                    } else {
                        MountManager::mount_smb(
                            source,
                            target,
                            username_arg,
                            password_arg,
                            domain_arg,
                            opts_arg,
                        )
                    };
                    password.clear();
                    match result {
                        Ok(()) => {
                            state.mounts = MountManager::list_mounts().unwrap_or_default();
                            state.devices = MountManager::list_block_devices().unwrap_or_default();
                            rebuild_entries(state);
                            state.status = if reconnecting {
                                format!("Reconnected SMB share {mounted_source}")
                            } else {
                                format!("Mounted SMB share {mounted_source}")
                            };
                            state.modal = Modal::None;
                        }
                        Err(error) => {
                            state.status = format!("SMB mount failed: {error}");
                            *field = 3;
                            *cursor = 0;
                        }
                    }
                }
            }
            _ => {
                if handle_line_editor_key(
                    smb_form_value_mut(source, target, username, password, domain, opts, *field),
                    cursor,
                    key,
                ) {
                    state.status.clear();
                }
            }
        },
        Modal::None => {}
    }

    Ok(false)
}
