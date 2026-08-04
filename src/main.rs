use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mount_tui::{
    BlockDevice, MountEntry, MountManager, UserAccessMethod, is_smb_fstype, ownership_options,
};
use nix::unistd::Uid;
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
    last_table_area: Rect,
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
    },
    SmbForm {
        source: String,
        target: String,
        username: String,
        password: String,
        domain: String,
        opts: String,
        field: usize,
        previous_mount: Option<MountEntry>,
    },
}

fn main() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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
                Event::Mouse(mouse) => {
                    handle_mouse(&mut state, mouse);
                }
                Event::Resize(_, _) => {}
                Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
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
        last_table_area: Rect::new(0, 0, 0, 0),
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
                    state.modal = Modal::MountForm {
                        source,
                        target,
                        fstype,
                        opts,
                        field: 0,
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
                        previous_mount,
                    };
                } else {
                    state.modal = Modal::UserAccess {
                        mount_points: entry.mount_points.clone(),
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

fn handle_mouse(state: &mut AppState, mouse: crossterm::event::MouseEvent) {
    match mouse.kind {
        MouseEventKind::ScrollUp => select_prev(state, 3),
        MouseEventKind::ScrollDown => select_next(state, 3),
        MouseEventKind::Down(_) => {
            if state.last_table_area.width == 0 || state.last_table_area.height == 0 {
                return;
            }
            let inner = state.last_table_area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 1,
            });
            let header_row = inner.y;
            let body_start = header_row.saturating_add(1);
            let body_end = body_start.saturating_add(state.entries.len() as u16);
            if mouse.column >= inner.x
                && mouse.column < inner.x + inner.width
                && mouse.row >= body_start
                && mouse.row < body_end
            {
                let row_index = (mouse.row - body_start) as usize;
                state.selected = row_index;
                state.table_state.select(Some(state.selected));
                update_info_extra_on_selection(state);
            }
        }
        _ => {}
    }
}

fn draw_ui<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
) -> anyhow::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    terminal.draw(|f| {
        let size = f.area();

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(5),
                Constraint::Length(4),
            ])
            .split(size);

        let header = render_header(state);
        f.render_widget(header, layout[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(layout[1]);

        let table = render_table(state, body[0]);
        state.last_table_area = body[0];
        f.render_stateful_widget(table, body[0], &mut state.table_state);

        let details = render_details(state);
        f.render_widget(details, body[1]);

        draw_footer(f, state, layout[2]);

        if state.input_mode == InputMode::Filter {
            let popup = render_filter_popup(state, size);
            f.render_widget(Clear, popup.area);
            f.render_widget(popup.widget, popup.area);
        }

        if !matches!(state.modal, Modal::None) {
            render_modal(f, state, size);
        }
    })?;
    Ok(())
}

struct Popup {
    area: Rect,
    widget: Paragraph<'static>,
}

fn render_filter_popup(state: &AppState, area: Rect) -> Popup {
    let width = area.width.saturating_mul(2) / 3;
    let height = 3;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    let widget = Paragraph::new(format!("Filter: {}", state.filter))
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Left)
        .block(Block::default().title("Filter").borders(Borders::ALL));
    Popup { area: rect, widget }
}

fn render_header(state: &AppState) -> Paragraph<'static> {
    let title = Span::styled(
        "Mount TUI",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let count = Span::styled(
        format!("{} entries", state.entries.len()),
        Style::default().fg(Color::Gray),
    );
    let filter = if state.filter.is_empty() {
        Span::raw("no filter")
    } else {
        Span::styled(
            format!("filter: {}", state.filter),
            Style::default().fg(Color::Yellow),
        )
    };

    Paragraph::new(vec![
        Line::from(vec![
            title,
            Span::raw("   "),
            count,
            Span::raw("   "),
            filter,
        ]),
        Line::from(vec![
            toggle_span("[d] disks", state.show_disks),
            Span::raw(" | "),
            toggle_span("[t] partitions", state.show_partitions),
            Span::raw(" | "),
            toggle_span("[s] smb", state.show_smb),
            Span::raw(" | "),
            toggle_span("[p] pseudo", state.show_pseudo),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL))
}

fn draw_footer(f: &mut ratatui::Frame<'_>, state: &AppState, area: Rect) {
    let status = if state.status.is_empty() {
        "Ready".to_string()
    } else {
        state.status.clone()
    };
    let (can_mount, can_unmount, can_access) = selected_actions(state);
    let root = is_root();
    let root_text = if root { "root: yes" } else { "root: no" };
    let root_style = if root {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    };

    f.render_widget(Block::default().borders(Borders::ALL), area);
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let hint = Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::DarkGray)),
        Span::raw(" move | "),
        Span::styled("f", Style::default().fg(Color::DarkGray)),
        Span::raw(" filter | "),
        Span::styled("r", Style::default().fg(Color::DarkGray)),
        Span::raw(" refresh | "),
        footer_action_span("m", "mount", can_mount),
        Span::raw(" | "),
        footer_action_span("n", "SMB", true),
        Span::raw(" | "),
        footer_action_span("u", "umount", can_unmount),
        Span::raw(" | "),
        footer_action_span("a", "access", can_access),
        Span::raw(" | "),
        Span::styled("q", Style::default().fg(Color::DarkGray)),
    ]);

    f.render_widget(Paragraph::new(hint), rows[0]);

    let status_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(rows[1]);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::default().fg(Color::Green),
        ))),
        status_row[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(root_text, root_style))).alignment(Alignment::Right),
        status_row[1],
    );
}

fn render_table(state: &AppState, _area: Rect) -> Table<'static> {
    let header_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let rows: Vec<Row> = state
        .entries
        .iter()
        .map(|e| {
            let name = if e.removable {
                format!("{} (R)", e.name)
            } else {
                e.name.clone()
            };
            let size = e
                .size_bytes
                .map(format_size)
                .unwrap_or_else(|| "-".to_string());
            let mount = if e.mount_points.is_empty() {
                "-".to_string()
            } else {
                e.mount_points.join(", ")
            };
            let fstype = e.fstype.clone().unwrap_or_else(|| "-".to_string());
            let kind = e.kind.clone();
            Row::new(vec![
                Cell::from(name),
                Cell::from(kind),
                Cell::from(size),
                Cell::from(mount),
                Cell::from(fstype),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(25),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Percentage(35),
        Constraint::Percentage(20),
    ];

    Table::new(rows, widths)
        .header(
            Row::new(vec![
                Cell::from("Name"),
                Cell::from("Kind"),
                Cell::from("Size"),
                Cell::from("Mount"),
                Cell::from("FS"),
            ])
            .style(header_style)
            .bottom_margin(0),
        )
        .block(Block::default().borders(Borders::ALL).title("Devices"))
        .row_highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
        .highlight_symbol(" ")
        .column_spacing(1)
        .widths(widths)
        .column_spacing(1)
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
}

fn render_details(state: &AppState) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if let Some(entry) = state.entries.get(state.selected) {
        lines.push(Line::from(Span::styled(
            "Details",
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(format!("Name: {}", entry.name)));
        lines.push(Line::from(format!("Kind: {}", entry.kind)));
        lines.push(Line::from(format!("Source: {}", entry.source)));
        if let Some(size) = entry.size_bytes {
            lines.push(Line::from(format!("Size: {}", format_size(size))));
        }
        if !entry.mount_points.is_empty() {
            lines.push(Line::from(format!(
                "Mount: {}",
                entry.mount_points.join(", ")
            )));
        }
        if let Some(fs) = &entry.fstype {
            lines.push(Line::from(format!("FS: {}", fs)));
        }
        if !entry.options.is_empty() {
            let useful_options: Vec<&str> = entry
                .options
                .iter()
                .map(String::as_str)
                .filter(|option| {
                    matches!(
                        option.split_once('=').map_or(*option, |(key, _)| key),
                        "ro" | "rw" | "uid" | "gid" | "file_mode" | "dir_mode" | "vers" | "domain"
                    )
                })
                .collect();
            if !useful_options.is_empty() {
                lines.push(Line::from(format!("Options: {}", useful_options.join(","))));
            }
        }
        if let Some(model) = &entry.model {
            lines.push(Line::from(format!("Model: {}", model)));
        }
        if let Some(vendor) = &entry.vendor {
            lines.push(Line::from(format!("Vendor: {}", vendor)));
        }
        lines.push(Line::from(format!(
            "Removable: {}",
            if entry.removable { "yes" } else { "no" }
        )));
        let hint = if state.info_extra_visible {
            "Press i to hide extra info"
        } else {
            "Press i to show extra info"
        };
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )));

        if state.info_extra_visible {
            for line in &state.info_extra {
                lines.push(Line::from(line.clone()));
            }
        }
    } else {
        lines.push(Line::from("No selection"));
    }

    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Info"))
        .alignment(Alignment::Left)
}

fn build_entries(
    mounts: &[MountEntry],
    devices: &[BlockDevice],
    show_pseudo: bool,
    show_smb: bool,
    show_disks: bool,
    show_partitions: bool,
    filter: &str,
) -> Vec<UiEntry> {
    let mount_map = build_mount_map(mounts);
    let mut entries = Vec::new();

    for dev in devices {
        if !is_real_device(&dev.name) {
            continue;
        }
        if dev.is_partition && !show_partitions {
            continue;
        }
        if !dev.is_partition && !show_disks {
            continue;
        }

        let mut mount_points = Vec::new();
        let mut mount_set = std::collections::HashSet::new();
        let mut fstype = None;
        let mut source = dev.path.clone();
        let mut options = Vec::new();

        let match_keys = device_match_keys(dev);
        for key in match_keys {
            if let Some(mounts) = mount_map.get(&key) {
                for m in mounts {
                    if mount_set.insert(m.target.clone()) {
                        mount_points.push(m.target.clone());
                    }
                    fstype = Some(m.fstype.clone());
                    source = m.source.clone();
                    options = m.options.clone();
                }
            }
        }

        let kind = if dev.is_partition { "part" } else { "disk" };
        let display_name = if let Some(mapper) = &dev.mapper_name {
            format!("{} ({})", dev.name, mapper)
        } else {
            dev.name.clone()
        };

        entries.push(UiEntry {
            name: display_name,
            kind: kind.to_string(),
            size_bytes: dev.size_bytes,
            mount_points,
            fstype,
            source,
            removable: dev.removable,
            model: dev.model.clone(),
            vendor: dev.vendor.clone(),
            options,
        });
    }

    if show_smb {
        for mount in mounts.iter().filter(|mount| is_smb_fstype(&mount.fstype)) {
            entries.push(UiEntry {
                name: mount.source.clone(),
                kind: "smb".to_string(),
                size_bytes: None,
                mount_points: vec![mount.target.clone()],
                fstype: Some(mount.fstype.clone()),
                source: mount.source.clone(),
                removable: false,
                model: None,
                vendor: None,
                options: mount.options.clone(),
            });
        }
    }

    if show_pseudo {
        for m in mounts {
            if m.source.starts_with("/dev/") {
                continue;
            }
            if !PSEUDO_FSTYPES.iter().any(|fs| fs == &m.fstype) {
                continue;
            }
            entries.push(UiEntry {
                name: m.target.clone(),
                kind: "pseudo".to_string(),
                size_bytes: None,
                mount_points: vec![m.target.clone()],
                fstype: Some(m.fstype.clone()),
                source: m.source.clone(),
                removable: false,
                model: None,
                vendor: None,
                options: m.options.clone(),
            });
        }
    }

    let filter_lc = filter.trim().to_lowercase();
    if !filter_lc.is_empty() {
        entries.retain(|e| {
            let hay = format!(
                "{} {} {} {} {}",
                e.name,
                e.source,
                e.mount_points.join(" "),
                e.fstype.clone().unwrap_or_default(),
                e.model.clone().unwrap_or_default()
            )
            .to_lowercase();
            hay.contains(&filter_lc)
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn build_mount_map(mounts: &[MountEntry]) -> HashMap<String, Vec<&MountEntry>> {
    let mut map: HashMap<String, Vec<&MountEntry>> = HashMap::new();
    for m in mounts {
        map.entry(m.source.clone()).or_default().push(m);
        if let Some(canon) = canonicalize_dev(&m.source) {
            map.entry(canon).or_default().push(m);
        }
    }
    map
}

fn canonicalize_dev(path: &str) -> Option<String> {
    if !path.starts_with("/dev/") {
        return None;
    }
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

fn device_match_keys(dev: &BlockDevice) -> Vec<String> {
    let mut keys = vec![dev.path.clone()];
    if let Some(canon) = canonicalize_dev(&dev.path) {
        keys.push(canon);
    }
    if dev.name.starts_with("dm-")
        && let Some(mapper) = &dev.mapper_name
    {
        keys.push(format!("/dev/mapper/{}", mapper));
    }
    keys
}

fn is_real_device(name: &str) -> bool {
    !VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

fn select_prev(state: &mut AppState, n: usize) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = state.selected.saturating_sub(n);
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

fn select_next(state: &mut AppState, n: usize) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = (state.selected + n).min(state.entries.len() - 1);
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

fn select_first(state: &mut AppState) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = 0;
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

fn select_last(state: &mut AppState) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = state.entries.len() - 1;
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

fn toggle_span(label: &str, enabled: bool) -> Span<'static> {
    if enabled {
        Span::styled(
            format!("{label}: ON"),
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("{label}: off"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )
    }
}

fn footer_action_span(key: &str, label: &str, enabled: bool) -> Span<'static> {
    if enabled {
        Span::styled(
            format!("{key} {label}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("{key} {label}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )
    }
}

fn update_info_extra_on_selection(state: &mut AppState) {
    if state.info_extra_visible {
        if let Some(entry) = state.entries.get(state.selected) {
            state.info_extra = device_info_lines(entry);
        } else {
            state.info_extra.clear();
        }
    }
}

fn selected_actions(state: &AppState) -> (bool, bool, bool) {
    if let Some(entry) = state.entries.get(state.selected) {
        let mounted = !entry.mount_points.is_empty();
        (!mounted, mounted, mounted)
    } else {
        (false, false, false)
    }
}

fn render_modal(f: &mut ratatui::Frame<'_>, state: &AppState, area: Rect) {
    let width = area.width.saturating_mul(2) / 3;
    let height = (area.height.saturating_mul(2) / 3).max(10).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    match &state.modal {
        Modal::NeedRoot => {
            let body = vec![
                Line::from("This action requires root privileges."),
                Line::from("Press R to re-run with sudo, or Esc to cancel."),
            ];
            let widget = Paragraph::new(body)
                .alignment(Alignment::Left)
                .block(Block::default().title("Need root").borders(Borders::ALL));
            f.render_widget(Clear, rect);
            f.render_widget(widget, rect);
        }
        Modal::ConfirmUnmount {
            mount_points,
            selected,
        } => {
            let items: Vec<ListItem> = mount_points
                .iter()
                .map(|m| ListItem::new(m.clone()))
                .collect();
            let mut list_state = ratatui::widgets::ListState::default();
            list_state.select(Some((*selected).min(items.len().saturating_sub(1))));
            let list = List::new(items)
                .block(
                    Block::default()
                        .title("Unmount which target?")
                        .borders(Borders::ALL),
                )
                .highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
                .highlight_symbol(" ");
            f.render_widget(Clear, rect);
            f.render_stateful_widget(list, rect, &mut list_state);
        }
        Modal::UserAccess {
            mount_points,
            selected,
            fstype,
        } => {
            let (uid, gid) = effective_user_ids();
            let user = effective_user_name();
            let method = if mount_tui::uses_mount_ownership(fstype) {
                format!(
                    "Reconnect with {} (the original mount is restored on failure)",
                    ownership_options(fstype, uid, gid)
                )
            } else {
                "Change owner of the mount root (existing children are unchanged)".to_string()
            };
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(5), Constraint::Min(2)])
                .split(rect);
            let explanation = Paragraph::new(vec![
                Line::from(format!("Grant access to {user} ({uid}:{gid})?")),
                Line::from(method),
                Line::from("Enter=apply  Esc=cancel"),
            ])
            .block(Block::default().title("User access").borders(Borders::ALL));
            let items: Vec<ListItem> = mount_points
                .iter()
                .map(|target| ListItem::new(target.clone()))
                .collect();
            let mut list_state = ratatui::widgets::ListState::default();
            list_state.select(Some((*selected).min(items.len().saturating_sub(1))));
            let list = List::new(items)
                .block(Block::default().title("Target").borders(Borders::ALL))
                .highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
                .highlight_symbol(" ");
            f.render_widget(Clear, rect);
            f.render_widget(explanation, rows[0]);
            f.render_stateful_widget(list, rows[1], &mut list_state);
        }
        Modal::MountForm {
            source,
            target,
            fstype,
            opts,
            field,
        } => {
            let lines = vec![
                form_line("Source", source, *field == 0),
                form_line("Target", target, *field == 1),
                form_line("Fstype", fstype, *field == 2),
                form_line("Options", opts, *field == 3),
                Line::from("Enter=next/confirm  Tab=next  Esc=cancel"),
            ];
            let widget = Paragraph::new(lines)
                .alignment(Alignment::Left)
                .block(Block::default().title("Mount").borders(Borders::ALL));
            f.render_widget(Clear, rect);
            f.render_widget(widget, rect);
        }
        Modal::SmbForm {
            source,
            target,
            username,
            password,
            domain,
            opts,
            field,
            previous_mount,
        } => {
            let masked_password = "*".repeat(password.chars().count());
            let source_invalid = !is_valid_smb_source(source);
            let target_invalid = !Path::new(target).is_absolute();
            let username_invalid =
                username.trim().is_empty() && (!password.is_empty() || !domain.trim().is_empty());
            let lines = vec![
                input_form_line(
                    "SMB resource *",
                    source,
                    "//server/share",
                    *field == 0,
                    source_invalid,
                ),
                input_form_line(
                    "Mount target *",
                    target,
                    "/media/user/share",
                    *field == 1,
                    target_invalid,
                ),
                input_form_line(
                    "Username",
                    username,
                    "blank = guest",
                    *field == 2,
                    username_invalid,
                ),
                input_form_line(
                    "Password",
                    &masked_password,
                    if username.is_empty() {
                        "blank for guest"
                    } else {
                        "enter SMB password"
                    },
                    *field == 3,
                    false,
                ),
                input_form_line("Domain", domain, "optional", *field == 4, false),
                input_form_line(
                    "Mount options",
                    opts,
                    "comma-separated, optional",
                    *field == 5,
                    false,
                ),
                Line::from("* required   ↑/↓/Tab/Shift+Tab: field   Ctrl+U: clear"),
                Line::from("Enter: next/connect   Esc: cancel"),
            ];
            let title = if previous_mount.is_some() {
                "Reconnect SMB/CIFS for user"
            } else {
                "Mount SMB/CIFS"
            };
            let widget = Paragraph::new(lines)
                .alignment(Alignment::Left)
                .block(Block::default().title(title).borders(Borders::ALL));
            f.render_widget(Clear, rect);
            f.render_widget(widget, rect);
        }
        Modal::None => {}
    }
}

fn form_line(label: &str, value: &str, active: bool) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!("{label}: "),
        Style::default().fg(Color::DarkGray),
    ));
    let style = if active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    spans.push(Span::styled(value.to_string(), style));
    Line::from(spans)
}

fn input_form_line(
    label: &str,
    value: &str,
    placeholder: &str,
    active: bool,
    invalid: bool,
) -> Line<'static> {
    let marker = if active { ">" } else { " " };
    let label_style = if invalid {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let value_style = if invalid {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if value.is_empty() {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(Color::White)
    };
    let shown_value = if value.is_empty() {
        format!("<{placeholder}>")
    } else {
        value.to_string()
    };
    let mut spans = vec![
        Span::styled(format!("{marker} {label}: "), label_style),
        Span::styled(shown_value, value_style),
    ];
    if invalid {
        spans.push(Span::styled(
            "  !",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
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
                        Err(e) => {
                            state.status = format!("umount failed: {e:?}");
                        }
                    }
                }
                state.modal = Modal::None;
            }
            _ => {}
        },
        Modal::MountForm {
            source,
            target,
            fstype,
            opts,
            field,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Tab => {
                *field = (*field + 1) % 4;
            }
            KeyCode::BackTab => {
                *field = field.saturating_sub(1);
            }
            KeyCode::Enter => {
                if *field < 3 {
                    *field += 1;
                } else {
                    let mut flags = nix::mount::MsFlags::empty();
                    if opts.split(',').any(|x| x == "ro") {
                        flags |= nix::mount::MsFlags::MS_RDONLY;
                    }
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
                        flags,
                    ) {
                        Ok(()) => {
                            state.mounts = MountManager::list_mounts().unwrap_or_default();
                            state.devices = MountManager::list_block_devices().unwrap_or_default();
                            rebuild_entries(state);
                            state.status = "Mounted".to_string();
                        }
                        Err(e) => {
                            state.status = format!("mount failed: {e:?}");
                        }
                    }
                    state.modal = Modal::None;
                }
            }
            KeyCode::Backspace => match *field {
                0 => {
                    source.pop();
                }
                1 => {
                    target.pop();
                }
                2 => {
                    fstype.pop();
                }
                3 => {
                    opts.pop();
                }
                _ => {}
            },
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => match *field {
                0 => source.push(c),
                1 => target.push(c),
                2 => fstype.push(c),
                3 => opts.push(c),
                _ => {}
            },
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
            previous_mount,
        } => match key.code {
            KeyCode::Esc => state.modal = Modal::None,
            KeyCode::Tab => {
                maybe_update_smb_target(source, target, *field);
                *field = (*field + 1).min(5);
            }
            KeyCode::BackTab | KeyCode::Up => *field = field.saturating_sub(1),
            KeyCode::Down => {
                maybe_update_smb_target(source, target, *field);
                *field = (*field + 1).min(5);
            }
            KeyCode::Home => *field = 0,
            KeyCode::End => *field = 5,
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
                } else {
                    if let Some((invalid_field, message)) =
                        smb_form_error(source, target, username, password, domain)
                    {
                        *field = invalid_field;
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
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                match *field {
                    0 => {
                        source.pop();
                    }
                    1 => {
                        target.pop();
                    }
                    2 => {
                        username.pop();
                    }
                    3 => {
                        password.pop();
                    }
                    4 => {
                        domain.pop();
                    }
                    5 => {
                        opts.pop();
                    }
                    _ => {}
                }
                state.status.clear();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match *field {
                    0 => source.clear(),
                    1 => target.clear(),
                    2 => username.clear(),
                    3 => password.clear(),
                    4 => domain.clear(),
                    5 => opts.clear(),
                    _ => {}
                }
                state.status.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                match *field {
                    0 => source.push(c),
                    1 => target.push(c),
                    2 => username.push(c),
                    3 => password.push(c),
                    4 => domain.push(c),
                    5 => opts.push(c),
                    _ => {}
                }
                state.status.clear();
            }
            _ => {}
        },
        Modal::None => {}
    }

    Ok(false)
}

fn default_mount_fields(state: &AppState) -> (String, String, String, String) {
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

fn default_smb_fields() -> (String, String, String, String) {
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

fn smb_reconnect_fields(entry: &UiEntry) -> (String, String, String, String, String) {
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

fn mount_option_value<'a>(options: &'a [String], keys: &[&str]) -> Option<&'a str> {
    options.iter().find_map(|option| {
        let (key, value) = option.split_once('=')?;
        keys.contains(&key).then_some(value)
    })
}

fn maybe_update_smb_target(source: &str, target: &mut String, field: usize) {
    if field == 0 && *target == default_smb_target("") && !source.trim().is_empty() {
        *target = default_smb_target(source);
    }
}

fn is_valid_smb_source(source: &str) -> bool {
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

fn smb_form_error(
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

fn default_smb_target(source: &str) -> String {
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

fn default_mount_target(source: &str) -> String {
    if source.starts_with("/dev/") {
        if let Some(label) = find_label_for_device(source) {
            let user = effective_user_name();
            let clean = sanitize_mount_name(&label);
            return format!("/media/{}/{}", user, clean);
        }
        if let Some(name) = source.split('/').next_back() {
            return format!("/mnt/{}", name);
        }
    }
    let clean = sanitize_mount_name(source);
    format!("/mnt/{}", clean)
}

fn is_root() -> bool {
    Uid::current().is_root()
}

fn device_info_lines(entry: &UiEntry) -> Vec<String> {
    if entry.fstype.as_deref().is_some_and(is_smb_fstype) {
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
    if let Some(fstype) = default_fstype(&dev_path) {
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

fn display_mount_options(options: &[String]) -> String {
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

fn entry_device_path(entry: &UiEntry) -> String {
    if entry.source.starts_with("/dev/") {
        return entry.source.clone();
    }
    let token = entry.name.split_whitespace().next().unwrap_or(&entry.name);
    format!("/dev/{}", token)
}

fn udev_data_for_device(dev_path: &str) -> Option<HashMap<String, String>> {
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

fn default_fstype(dev_path: &str) -> Option<String> {
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

fn canonical_device_name(dev_path: &str) -> Option<String> {
    let canon = fs::canonicalize(dev_path).ok()?;
    canon.file_name().map(|s| s.to_string_lossy().to_string())
}

fn default_mount_opts(fstype: &str) -> String {
    if fstype.is_empty() {
        return String::new();
    }
    match fstype {
        "vfat" | "exfat" | "ntfs" | "ntfs3" => {
            let (uid, gid) = effective_user_ids();
            format!("rw,{}", ownership_options(fstype, uid, gid))
        }
        _ => "defaults".to_string(),
    }
}

fn effective_user_ids() -> (u32, u32) {
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
                .and_then(|name| nix::unistd::User::from_name(&name).ok().flatten())
                .map(|user| user.uid.as_raw())
        })
        .unwrap_or_else(|| unsafe { nix::libc::getuid() as u32 });
    let gid = env::var("SUDO_GID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .or_else(|| {
            nix::unistd::User::from_uid(Uid::from_raw(uid))
                .ok()
                .flatten()
                .map(|user| user.gid.as_raw())
        })
        .unwrap_or_else(|| unsafe { nix::libc::getgid() as u32 });
    (uid, gid)
}

fn effective_user_name() -> String {
    for key in ["SUDO_USER", "DOAS_USER"] {
        if let Ok(name) = env::var(key)
            && !name.is_empty()
        {
            return name;
        }
    }
    let (uid, _) = effective_user_ids();
    nix::unistd::User::from_uid(Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|user| user.name)
        .or_else(|| env::var("USER").ok())
        .unwrap_or_else(|| uid.to_string())
}

fn find_label_for_device(dev_path: &str) -> Option<String> {
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

fn sanitize_mount_name(name: &str) -> String {
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

fn reexec_with_sudo() -> anyhow::Result<()> {
    disable_raw_mode().ok();
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
    let exe = env::current_exe()?;
    let args: Vec<String> = env::args().skip(1).collect();
    let status = Command::new("sudo").arg(exe).args(args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smb_mounts_are_visible_without_pseudo_filesystems() {
        let mounts = vec![MountEntry {
            source: "//nas.example/media".to_string(),
            target: "/media/alice/media".to_string(),
            fstype: "cifs".to_string(),
            options: vec!["rw".to_string(), "uid=1000".to_string()],
        }];

        let entries = build_entries(&mounts, &[], false, true, true, true, "");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "smb");
        assert_eq!(entries[0].source, "//nas.example/media");
        assert_eq!(entries[0].mount_points, ["/media/alice/media"]);
    }

    #[test]
    fn smb_mounts_participate_in_filtering() {
        let mounts = vec![MountEntry {
            source: "//nas.example/photos".to_string(),
            target: "/media/alice/photos".to_string(),
            fstype: "cifs".to_string(),
            options: Vec::new(),
        }];

        assert_eq!(
            build_entries(&mounts, &[], false, true, true, true, "photos").len(),
            1
        );
        assert!(build_entries(&mounts, &[], false, true, true, true, "documents").is_empty());
        assert!(build_entries(&mounts, &[], false, false, true, true, "").is_empty());
    }

    #[test]
    fn secret_mount_options_are_redacted() {
        let options = vec![
            "rw".to_string(),
            "password=secret".to_string(),
            "credentials=/run/private".to_string(),
        ];
        assert_eq!(
            display_mount_options(&options),
            "rw,password=<hidden>,credentials=<hidden>"
        );
    }

    #[test]
    fn reconnect_form_uses_existing_smb_identity_and_writable_user_options() {
        let entry = UiEntry {
            name: "//nas/share".to_string(),
            kind: "smb".to_string(),
            size_bytes: None,
            mount_points: vec!["/mnt/share".to_string()],
            fstype: Some("cifs".to_string()),
            source: "//nas/share".to_string(),
            removable: false,
            model: None,
            vendor: None,
            options: vec![
                "ro".to_string(),
                "vers=3.1.1".to_string(),
                "username=alice".to_string(),
                "domain=OFFICE".to_string(),
                "uid=0".to_string(),
            ],
        };

        let (source, target, username, domain, options) = smb_reconnect_fields(&entry);

        assert_eq!(source, "//nas/share");
        assert_eq!(target, "/mnt/share");
        assert_eq!(username, "alice");
        assert_eq!(domain, "OFFICE");
        assert!(options.starts_with("rw,nosuid,nodev,"));
        assert!(options.contains("vers=3.1.1"));
        assert!(options.contains("forceuid,forcegid"));
        assert!(!options.contains("ro,"));
        assert!(!options.contains("username="));
    }

    #[test]
    fn smb_form_validation_points_to_the_first_invalid_field() {
        assert_eq!(
            smb_form_error("", "/mnt/share", "", "", "").map(|error| error.0),
            Some(0)
        );
        assert_eq!(
            smb_form_error("nas/share", "/mnt/share", "", "", "").map(|error| error.0),
            Some(0)
        );
        assert_eq!(
            smb_form_error("//nas/share", "relative/path", "", "", "").map(|error| error.0),
            Some(1)
        );
        assert_eq!(
            smb_form_error("//nas/share", "/mnt/share", "", "secret", "").map(|error| error.0),
            Some(2)
        );
        assert!(smb_form_error("smb://nas/share", "/mnt/share", "alice", "", "").is_none());
        assert!(smb_form_error("//nas/share", "/mnt/share", "", "", "").is_none());
    }
}
