use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mount_tui::{
    BlockDevice, FstabEntry, FstabRecord, MountEntry, MountManager, SshHost, UserAccessMethod,
    is_smb_fstype, is_sshfs_fstype, ownership_options, read_fstab, read_ssh_config,
    remove_fstab_entry, replace_fstab_entry,
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
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime};
use std::{env, process::Command};
const VIRTUAL_PREFIXES: &[&str] = &["loop", "ram", "zram", "fd"];
const SSH_KEY_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
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
    fstab_line: Option<usize>,
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
    ssh_hosts: Vec<SshHost>,
    fstab_entries: Vec<FstabRecord>,
    show_fstab: bool,
    ssh_key_verification: Option<SshKeyVerification>,
}

#[derive(Debug)]
enum SshKeyVerification {
    Running {
        receiver: Receiver<Result<(), String>>,
        frame: usize,
        frames_shown: usize,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone)]
enum Modal {
    None,
    NeedRoot {
        action: &'static str,
    },
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
    SshHosts {
        selected: usize,
    },
    SshForm {
        host: String,
        remote_path: String,
        target: String,
        password: String,
        opts: String,
        permanent: bool,
        field: usize,
        cursor: usize,
    },
    Export {
        entry: FstabEntry,
        path: String,
        cursor: usize,
        editing_path: bool,
    },
    FstabEdit {
        line_index: usize,
        original: String,
        raw: String,
        cursor: usize,
    },
    ConfirmFstabRemove {
        line_index: usize,
        raw: String,
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
        poll_ssh_key_verification(&mut state);
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
        ssh_hosts: read_ssh_config(&effective_user_home()).unwrap_or_default(),
        fstab_entries: read_fstab(Path::new("/etc/fstab")).unwrap_or_default(),
        show_fstab: false,
        ssh_key_verification: None,
    };
    rebuild_entries(&mut state);
    Ok(state)
}

fn poll_ssh_key_verification(state: &mut AppState) {
    enum Outcome {
        Pending,
        Verified,
        Failed(String),
    }

    let outcome = match state.ssh_key_verification.as_mut() {
        Some(SshKeyVerification::Running {
            receiver,
            frame,
            frames_shown,
        }) => {
            *frame = (*frame + 1) % SSH_KEY_SPINNER.len();
            if *frames_shown == 0 {
                *frames_shown = 1;
                Outcome::Pending
            } else {
                match receiver.try_recv() {
                    Ok(Ok(())) => Outcome::Verified,
                    Ok(Err(message)) => Outcome::Failed(message),
                    Err(TryRecvError::Empty) => {
                        *frames_shown += 1;
                        Outcome::Pending
                    }
                    Err(TryRecvError::Disconnected) => Outcome::Failed(
                        "the SSH verification worker stopped without returning a result"
                            .to_string(),
                    ),
                }
            }
        }
        Some(SshKeyVerification::Failed { .. }) | None => return,
    };

    match outcome {
        Outcome::Pending => {}
        Outcome::Verified => {
            state.ssh_key_verification = None;
            if let Modal::SshForm { permanent, .. } = &mut state.modal {
                *permanent = true;
            }
            state.status = "SSH key verified; /etc/fstab is enabled".to_string();
        }
        Outcome::Failed(message) => {
            if let Modal::SshForm { permanent, .. } = &mut state.modal {
                *permanent = false;
            }
            state.status = format!("SSH key verification failed: {message}");
            state.ssh_key_verification = Some(SshKeyVerification::Failed { message });
        }
    }
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
    if state.show_fstab {
        append_fstab_entries(
            &mut state.entries,
            &state.fstab_entries,
            &state.mounts,
            &state.filter,
        );
    }
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
    if let Some(verification) = &state.ssh_key_verification {
        match verification {
            SshKeyVerification::Running { .. } => return Ok(false),
            SshKeyVerification::Failed { .. } => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                    state.ssh_key_verification = None;
                }
                return Ok(false);
            }
        }
    }

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
            state.ssh_hosts = read_ssh_config(&effective_user_home()).unwrap_or_default();
            state.fstab_entries = read_fstab(Path::new("/etc/fstab")).unwrap_or_default();
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
        KeyCode::Char('b') => {
            state.show_fstab = !state.show_fstab;
            state.fstab_entries = read_fstab(Path::new("/etc/fstab")).unwrap_or_default();
            rebuild_entries(state);
            state.status = if state.show_fstab {
                "fstab entries are visible; e edits and Delete removes".to_string()
            } else {
                "fstab entries are hidden".to_string()
            };
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
                if entry.fstab_line.is_some() {
                    state.status =
                        "This is an fstab configuration entry, not a live mount".to_string();
                } else if !entry.mount_points.is_empty() {
                    if !is_root() {
                        state.modal = Modal::NeedRoot {
                            action: "Unmounting",
                        };
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
                if entry.fstab_line.is_some() {
                    state.status =
                        "Edit the fstab entry with e; mounting configured entries is not automatic"
                            .to_string();
                } else if !entry.mount_points.is_empty() {
                    state.status = "Already mounted (use unmount)".to_string();
                } else if !is_root() {
                    state.modal = Modal::NeedRoot { action: "Mounting" };
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
                state.modal = Modal::NeedRoot {
                    action: "Mounting SMB",
                };
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
        KeyCode::Char('h') => {
            if !is_root() {
                state.modal = Modal::NeedRoot {
                    action: "Mounting SSH",
                };
            } else {
                state.modal = Modal::SshHosts { selected: 0 };
            }
        }
        KeyCode::Char('x') => {
            if let Some(entry) = state.entries.get(state.selected) {
                let fstab_entry = entry
                    .fstab_line
                    .and_then(|line| {
                        state
                            .fstab_entries
                            .iter()
                            .find(|record| record.line_index == line)
                    })
                    .map(|record| Ok(record.entry.clone()))
                    .unwrap_or_else(|| fstab_entry_for_ui(entry));
                match fstab_entry {
                    Ok(entry) => {
                        let path = effective_user_home().join("mount-tui-fstab.txt");
                        let path = path.to_string_lossy().to_string();
                        state.modal = Modal::Export {
                            cursor: path.chars().count(),
                            entry,
                            path,
                            editing_path: false,
                        };
                    }
                    Err(error) => state.status = format!("export failed: {error}"),
                }
            }
        }
        KeyCode::Char('e') => {
            if let Some(entry) = state.entries.get(state.selected)
                && let Some(line_index) = entry.fstab_line
                && let Some(record) = state
                    .fstab_entries
                    .iter()
                    .find(|record| record.line_index == line_index)
            {
                if !is_root() {
                    state.modal = Modal::NeedRoot {
                        action: "Editing /etc/fstab",
                    };
                } else {
                    let raw = record.entry.render();
                    state.modal = Modal::FstabEdit {
                        cursor: raw.chars().count(),
                        line_index,
                        original: raw.clone(),
                        raw,
                    };
                }
            }
        }
        KeyCode::Delete => {
            if let Some(entry) = state.entries.get(state.selected)
                && let Some(line_index) = entry.fstab_line
                && let Some(record) = state
                    .fstab_entries
                    .iter()
                    .find(|record| record.line_index == line_index)
            {
                if !is_root() {
                    state.modal = Modal::NeedRoot {
                        action: "Removing from /etc/fstab",
                    };
                } else {
                    state.modal = Modal::ConfirmFstabRemove {
                        line_index,
                        raw: record.entry.render(),
                    };
                }
            }
        }
        KeyCode::Char('a') => {
            if let Some(entry) = state.entries.get(state.selected) {
                if entry.fstab_line.is_some() {
                    state.status = "Edit ownership options in this fstab entry with e".to_string();
                } else if entry.mount_points.is_empty() {
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
                    state.modal = Modal::NeedRoot {
                        action: "Changing mount ownership",
                    };
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
        Modal::NeedRoot { .. } => match key.code {
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
        Modal::SshHosts { selected } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                *selected = (*selected + 1).min(state.ssh_hosts.len());
            }
            KeyCode::Enter => {
                let configured = selected
                    .checked_sub(1)
                    .and_then(|index| state.ssh_hosts.get(index));
                let (host, remote_path, target, opts) = default_ssh_form(configured);
                state.modal = Modal::SshForm {
                    cursor: host.chars().count(),
                    host,
                    remote_path,
                    target,
                    password: String::new(),
                    opts,
                    permanent: false,
                    field: 0,
                };
            }
            _ => {}
        },
        Modal::SshForm {
            host,
            remote_path,
            target,
            password,
            opts,
            permanent,
            field,
            cursor,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Tab => {
                *field = (*field + 1) % 6;
                *cursor = ssh_form_value(host, remote_path, target, password, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::BackTab | KeyCode::Up => {
                *field = field.saturating_sub(1);
                *cursor = ssh_form_value(host, remote_path, target, password, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::Down => {
                *field = (*field + 1).min(5);
                *cursor = ssh_form_value(host, remote_path, target, password, opts, *field)
                    .chars()
                    .count();
            }
            KeyCode::Char(' ') if *field == 5 => {
                if *permanent {
                    *permanent = false;
                    state.status = "Permanent SSH mount disabled".to_string();
                } else if !password.is_empty() {
                    *permanent = false;
                    state.status =
                        "Password authentication cannot be stored in /etc/fstab; clear the password and use a key"
                            .to_string();
                } else if let Err(error) = boot_ssh_identity(opts) {
                    *permanent = false;
                    state.status = format!("SSH key verification failed: {error}");
                    state.ssh_key_verification =
                        Some(SshKeyVerification::Failed { message: error });
                } else if let Err(error) = prepare_user_known_hosts() {
                    *permanent = false;
                    let message = format!("known_hosts setup failed: {error}");
                    state.status = format!("SSH key verification failed: {message}");
                    state.ssh_key_verification = Some(SshKeyVerification::Failed { message });
                } else {
                    let options = opts
                        .split(',')
                        .map(str::trim)
                        .filter(|option| !option.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    let config = effective_user_home().join(".ssh/config");
                    let local_user = effective_user_name();
                    let host = host.clone();
                    let (sender, receiver) = mpsc::channel();
                    thread::spawn(move || {
                        let result = MountManager::check_ssh_key(
                            &host,
                            &options,
                            Some(&config),
                            Some(&local_user),
                        )
                        .map_err(|error| error.to_string());
                        let _ = sender.send(result);
                    });
                    *permanent = false;
                    state.ssh_key_verification = Some(SshKeyVerification::Running {
                        receiver,
                        frame: 0,
                        frames_shown: 0,
                    });
                    state.status = "Verifying SSH key with the server...".to_string();
                }
            }
            KeyCode::Enter => {
                if *field < 5 {
                    if *field == 0 && *target == default_ssh_target("") {
                        *target = default_ssh_target(host);
                    }
                    *field += 1;
                    *cursor = ssh_form_value(host, remote_path, target, password, opts, *field)
                        .chars()
                        .count();
                    return Ok(false);
                }
                if let Some((invalid_field, message)) = ssh_form_error(host, remote_path, target) {
                    *field = invalid_field;
                    *cursor = ssh_form_value(host, remote_path, target, password, opts, *field)
                        .chars()
                        .count();
                    state.status = message.to_string();
                    return Ok(false);
                }
                if !password.is_empty() {
                    *permanent = false;
                }
                if !Path::new(target.as_str()).exists()
                    && let Err(error) = fs::create_dir_all(target.as_str())
                {
                    state.status = format!("mkdir failed: {error}");
                    state.modal = Modal::None;
                    return Ok(false);
                }
                if let Err(error) = prepare_ssh_mount_target(Path::new(target.as_str())) {
                    state.status = format!("SSH mount target setup failed: {error}");
                    return Ok(false);
                }
                let source = ssh_source(host, remote_path);
                let options = opts
                    .split(',')
                    .map(str::trim)
                    .filter(|option| !option.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let config = effective_user_home().join(".ssh/config");
                if let Err(error) = prepare_user_known_hosts() {
                    state.status = format!("known_hosts setup failed: {error}");
                    return Ok(false);
                }
                let password_arg = (!password.is_empty()).then_some(password.as_str());
                let local_user = effective_user_name();
                let result = MountManager::mount_sshfs(
                    &source,
                    target,
                    &options,
                    Some(&config),
                    password_arg,
                    Some(&local_user),
                );
                password.clear();
                match result {
                    Ok(()) => {
                        let mut status = format!("Mounted SSH filesystem {source}");
                        if *permanent {
                            match FstabEntry::new(
                                source.clone(),
                                target.clone(),
                                "fuse.sshfs",
                                ssh_persistent_options(&options),
                            )
                            .and_then(|entry| entry.append_to(Path::new("/etc/fstab")))
                            {
                                Ok(()) => status.push_str(" and added it to /etc/fstab"),
                                Err(error) => {
                                    status = format!(
                                        "Mounted SSH filesystem, but /etc/fstab update failed: {error}"
                                    )
                                }
                            }
                        }
                        state.mounts = MountManager::list_mounts().unwrap_or_default();
                        rebuild_entries(state);
                        state.status = status;
                        state.modal = Modal::None;
                    }
                    Err(error) => {
                        *permanent = false;
                        *field = 3;
                        *cursor = 0;
                        state.status = format!("SSHFS mount failed: {error}");
                    }
                }
            }
            _ if *field < 5
                && handle_line_editor_key(
                    ssh_form_value_mut(host, remote_path, target, password, opts, *field),
                    cursor,
                    key,
                ) =>
            {
                if *field == 3 && !password.is_empty() {
                    *permanent = false;
                }
                state.status.clear();
            }
            _ => {}
        },
        Modal::Export {
            entry,
            path,
            cursor,
            editing_path,
        } => {
            if *editing_path {
                match key.code {
                    KeyCode::Esc => *editing_path = false,
                    KeyCode::Enter => {
                        let export_path = path.clone();
                        match entry.export_new(Path::new(&export_path)) {
                            Ok(()) => {
                                let (uid, gid) = effective_user_ids();
                                if is_root() && uid != 0 {
                                    let _ = rustix::fs::chown(
                                        export_path.as_str(),
                                        Some(rustix::process::Uid::from_raw(uid)),
                                        Some(rustix::process::Gid::from_raw(gid)),
                                    );
                                }
                                state.status = format!("Exported fstab entry to {export_path}")
                            }
                            Err(error) => state.status = format!("export failed: {error}"),
                        }
                        *editing_path = false;
                    }
                    _ => {
                        handle_line_editor_key(path, cursor, key);
                    }
                }
            } else {
                match key.code {
                    KeyCode::Esc => state.modal = Modal::None,
                    KeyCode::Char('e' | 'E') => {
                        *editing_path = true;
                        *cursor = path.chars().count();
                    }
                    KeyCode::Char('p' | 'P') => {
                        if is_sshfs_fstype(&entry.fstype) {
                            state.status =
                                "SSH /etc/fstab entries require verified key authentication; use the h SSH form"
                                    .to_string();
                        } else if !is_root() {
                            state.modal = Modal::NeedRoot {
                                action: "Adding to /etc/fstab",
                            };
                        } else {
                            if !Path::new(&entry.target).exists()
                                && let Err(error) = fs::create_dir_all(&entry.target)
                            {
                                state.status = format!("mkdir failed: {error}");
                                return Ok(false);
                            }
                            match entry.append_to(Path::new("/etc/fstab")) {
                                Ok(()) => {
                                    state.status = format!(
                                        "Added {} on {} to /etc/fstab",
                                        entry.source, entry.target
                                    );
                                    state.modal = Modal::None;
                                }
                                Err(error) => {
                                    state.status = format!("fstab update failed: {error}")
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Modal::FstabEdit {
            line_index,
            original,
            raw,
            cursor,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Enter => {
                if !is_root() {
                    state.modal = Modal::NeedRoot {
                        action: "Editing /etc/fstab",
                    };
                    return Ok(false);
                }
                let current = read_fstab(Path::new("/etc/fstab")).and_then(|records| {
                    records
                        .into_iter()
                        .find(|record| record.line_index == *line_index)
                        .map(|record| record.entry.render())
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::NotFound, "fstab entry disappeared")
                        })
                });
                let result = current.and_then(|current| {
                    if current != *original {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "fstab changed on disk; refresh and try again",
                        ));
                    }
                    FstabEntry::parse(raw).and_then(|entry| {
                        replace_fstab_entry(Path::new("/etc/fstab"), *line_index, &entry)
                    })
                });
                match result {
                    Ok(()) => {
                        state.fstab_entries =
                            read_fstab(Path::new("/etc/fstab")).unwrap_or_default();
                        rebuild_entries(state);
                        state.status =
                            "Updated /etc/fstab (backup: /etc/fstab.mount-tui.bak)".to_string();
                        state.modal = Modal::None;
                    }
                    Err(error) => state.status = format!("fstab edit failed: {error}"),
                }
            }
            _ => {
                handle_line_editor_key(raw, cursor, key);
            }
        },
        Modal::ConfirmFstabRemove { line_index, raw } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Enter => {
                if !is_root() {
                    state.modal = Modal::NeedRoot {
                        action: "Removing from /etc/fstab",
                    };
                    return Ok(false);
                }
                let unchanged = read_fstab(Path::new("/etc/fstab")).and_then(|records| {
                    records
                        .into_iter()
                        .find(|record| record.line_index == *line_index)
                        .filter(|record| record.entry.render() == *raw)
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "fstab changed on disk; refresh and try again",
                            )
                        })
                });
                match unchanged
                    .and_then(|_| remove_fstab_entry(Path::new("/etc/fstab"), *line_index))
                {
                    Ok(()) => {
                        state.fstab_entries =
                            read_fstab(Path::new("/etc/fstab")).unwrap_or_default();
                        rebuild_entries(state);
                        state.status =
                            "Removed fstab entry (backup: /etc/fstab.mount-tui.bak)".to_string();
                        state.modal = Modal::None;
                    }
                    Err(error) => state.status = format!("fstab removal failed: {error}"),
                }
            }
            _ => {}
        },
        Modal::None => {}
    }

    Ok(false)
}
