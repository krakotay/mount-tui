use super::*;

pub(super) fn draw_ui<B: ratatui::backend::Backend>(
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

pub(super) fn render_header(state: &AppState) -> Paragraph<'static> {
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

pub(super) fn draw_footer(f: &mut ratatui::Frame<'_>, state: &AppState, area: Rect) {
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

    let status_style = if status_is_error(&status) {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(status, status_style))),
        status_row[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(root_text, root_style))).alignment(Alignment::Right),
        status_row[1],
    );
}

pub(super) fn status_is_error(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    ["failed", "failure", "error", "einval"]
        .iter()
        .any(|marker| status.contains(marker))
}

pub(super) fn render_table(state: &AppState, _area: Rect) -> Table<'static> {
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
            let owner = ownership_label(e.ownership);
            let owner_style = match e.ownership {
                Ownership::CurrentUser => Style::default().fg(Color::Green),
                Ownership::Other(_) | Ownership::Mixed => Style::default().fg(Color::Yellow),
                Ownership::Unknown | Ownership::Unmounted => Style::default().fg(Color::DarkGray),
            };
            Row::new(vec![
                Cell::from(name),
                Cell::from(kind),
                Cell::from(size),
                Cell::from(mount),
                Cell::from(fstype),
                Cell::from(owner).style(owner_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(20),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Percentage(30),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
    ];

    Table::new(rows, widths)
        .header(
            Row::new(vec![
                Cell::from("Name"),
                Cell::from("Kind"),
                Cell::from("Size"),
                Cell::from("Mount"),
                Cell::from("FS"),
                Cell::from("Owner"),
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

pub(super) fn render_details(state: &AppState) -> Paragraph<'static> {
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
        if !matches!(entry.ownership, Ownership::Unmounted) {
            let style = match entry.ownership {
                Ownership::CurrentUser => Style::default().fg(Color::Green),
                Ownership::Other(_) | Ownership::Mixed => Style::default().fg(Color::Yellow),
                Ownership::Unknown | Ownership::Unmounted => Style::default().fg(Color::DarkGray),
            };
            lines.push(Line::from(Span::styled(
                format!("Owner: {}", ownership_label(entry.ownership)),
                style,
            )));
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

pub(super) fn build_entries(
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
        // Keep udev's best-effort filesystem signature visible even when the
        // device is not mounted. A mounted filesystem remains authoritative.
        let mut fstype = dev.fstype.clone();
        let mut source = dev.path.clone();
        let mut options = Vec::new();
        let mut owner_uids = Vec::new();

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
                    owner_uids.push(mount_owner_uid(m));
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
            ownership: ownership_from_uids(&owner_uids, effective_user_ids().0),
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
                ownership: ownership_from_uids(&[mount_owner_uid(mount)], effective_user_ids().0),
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
                ownership: ownership_from_uids(&[mount_owner_uid(m)], effective_user_ids().0),
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

pub(super) fn build_mount_map(mounts: &[MountEntry]) -> HashMap<String, Vec<&MountEntry>> {
    let mut map: HashMap<String, Vec<&MountEntry>> = HashMap::new();
    for m in mounts {
        map.entry(m.source.clone()).or_default().push(m);
        if let Some(canon) = canonicalize_dev(&m.source) {
            map.entry(canon).or_default().push(m);
        }
    }
    map
}

pub(super) fn canonicalize_dev(path: &str) -> Option<String> {
    if !path.starts_with("/dev/") {
        return None;
    }
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

pub(super) fn device_match_keys(dev: &BlockDevice) -> Vec<String> {
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

pub(super) fn is_real_device(name: &str) -> bool {
    !VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p))
}

pub(super) fn format_size(bytes: u64) -> String {
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

pub(super) fn select_prev(state: &mut AppState, n: usize) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = state.selected.saturating_sub(n);
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

pub(super) fn select_next(state: &mut AppState, n: usize) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = (state.selected + n).min(state.entries.len() - 1);
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

pub(super) fn select_first(state: &mut AppState) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = 0;
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

pub(super) fn select_last(state: &mut AppState) {
    if state.entries.is_empty() {
        return;
    }
    state.selected = state.entries.len() - 1;
    state.table_state.select(Some(state.selected));
    update_info_extra_on_selection(state);
}

pub(super) fn toggle_span(label: &str, enabled: bool) -> Span<'static> {
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

pub(super) fn footer_action_span(key: &str, label: &str, enabled: bool) -> Span<'static> {
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

pub(super) fn update_info_extra_on_selection(state: &mut AppState) {
    if state.info_extra_visible {
        if let Some(entry) = state.entries.get(state.selected) {
            state.info_extra = device_info_lines(entry);
        } else {
            state.info_extra.clear();
        }
    }
}

pub(super) fn selected_actions(state: &AppState) -> (bool, bool, bool) {
    if let Some(entry) = state.entries.get(state.selected) {
        let mounted = !entry.mount_points.is_empty();
        (
            !mounted,
            mounted,
            mounted && entry.ownership != Ownership::CurrentUser,
        )
    } else {
        (false, false, false)
    }
}

pub(super) fn mount_owner_uid(mount: &MountEntry) -> Option<u32> {
    if mount_tui::uses_mount_ownership(&mount.fstype) {
        // Synthetic-ownership filesystems default to root when no uid option
        // is present. Avoid stat on a remote SMB share, which may block if its
        // server is unavailable.
        return mount_option_value(&mount.options, &["uid"])
            .and_then(|uid| uid.parse().ok())
            .or(Some(0));
    }
    rustix::fs::stat(&mount.target).ok().map(|stat| stat.st_uid)
}

pub(super) fn mount_points_needing_access(
    mounts: &[MountEntry],
    mount_points: &[String],
    current_uid: u32,
) -> Vec<String> {
    mount_points
        .iter()
        .filter(|target| {
            mounts
                .iter()
                .find(|mount| mount.target == target.as_str())
                .and_then(mount_owner_uid)
                != Some(current_uid)
        })
        .cloned()
        .collect()
}

pub(super) fn ownership_from_uids(owner_uids: &[Option<u32>], current_uid: u32) -> Ownership {
    if owner_uids.is_empty() {
        return Ownership::Unmounted;
    }
    if owner_uids.iter().any(Option::is_none) {
        return Ownership::Unknown;
    }
    let mut owners = owner_uids.iter().flatten();
    let first = *owners.next().expect("non-empty owners checked above");
    if owners.any(|uid| *uid != first) {
        Ownership::Mixed
    } else if first == current_uid {
        Ownership::CurrentUser
    } else {
        Ownership::Other(first)
    }
}

pub(super) fn ownership_label(ownership: Ownership) -> String {
    match ownership {
        Ownership::Unmounted => "-".to_string(),
        Ownership::CurrentUser => format!("you ({})", effective_user_ids().0),
        Ownership::Other(uid) => format!("uid:{uid}"),
        Ownership::Mixed => "mixed".to_string(),
        Ownership::Unknown => "unknown".to_string(),
    }
}

pub(super) fn render_modal(f: &mut ratatui::Frame<'_>, state: &AppState, area: Rect) {
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
        Modal::UnmountError {
            target,
            error,
            processes,
        } => {
            let mut body = vec![
                Line::from(Span::styled(
                    format!("Could not unmount {target}"),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(error.clone()),
                Line::from(""),
            ];
            if processes.is_empty() {
                body.push(Line::from(
                    "No userspace PID was found; a nested mount or kernel user may keep it busy.",
                ));
            } else {
                body.push(Line::from("Processes using this mount:"));
                body.extend(processes.iter().map(|process| {
                    Line::from(format!("  PID {:>7}  {}", process.pid, process.name))
                }));
            }
            body.push(Line::from(""));
            body.push(Line::from("Enter/Esc: close"));
            let widget = Paragraph::new(body)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(
                    Block::default()
                        .title("Unmount failed")
                        .borders(Borders::ALL),
                );
            f.render_widget(Clear, rect);
            f.render_widget(widget, rect);
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
            cursor,
        } => {
            let fstype_line = if is_ntfs_driver(fstype) {
                ntfs_driver_line(fstype, *field == 2)
            } else {
                editable_form_line("Fstype", fstype, *field == 2, *cursor)
            };
            let lines = vec![
                editable_form_line("Source", source, *field == 0, *cursor),
                editable_form_line("Target", target, *field == 1, *cursor),
                fstype_line,
                editable_form_line("Options", opts, *field == 3, *cursor),
                Line::from(if is_ntfs_driver(fstype) {
                    "↑/↓/Tab: field  Space or ←/→: driver  Enter: next/mount  Esc: cancel"
                } else {
                    "↑/↓: field  ←/→/Home/End: cursor  Del/Backspace: edit  Enter: next/mount"
                }),
            ];
            let widget = Paragraph::new(lines)
                .alignment(Alignment::Left)
                .block(Block::default().title("Mount").borders(Borders::ALL));
            f.render_widget(Clear, rect);
            f.render_widget(widget, rect);
        }
        Modal::ConfirmMountRetry {
            source,
            target,
            fstype,
            opts,
            error,
        } => {
            let retries = mount_retry_options(fstype, opts);
            let has_force = retries.iter().any(|retry| retry.force);
            let heading = if has_force {
                "DANGER: force mount fallback available"
            } else {
                "Mount failed"
            };
            let mut body = vec![
                Line::from(Span::styled(
                    heading,
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(error.clone()),
                Line::from(""),
                Line::from(format!("Retry {source} on {target}?")),
                Line::from(format!("Filesystem: {fstype}   Failed options: {opts}")),
                Line::from(retry_prompt(&retries)),
            ];
            if has_force {
                body.push(Line::from(Span::styled(
                    "DANGER: force can damage the filesystem and is not recommended.",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
            }
            let widget = Paragraph::new(body)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(
                    Block::default()
                        .title(if has_force {
                            "DANGER: force mount fallback"
                        } else {
                            "Mount fallback"
                        })
                        .borders(Borders::ALL),
                );
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
            cursor,
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
                    *cursor,
                    source_invalid,
                ),
                input_form_line(
                    "Mount target *",
                    target,
                    "/media/user/share",
                    *field == 1,
                    *cursor,
                    target_invalid,
                ),
                input_form_line(
                    "Username",
                    username,
                    "blank = guest",
                    *field == 2,
                    *cursor,
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
                    *cursor,
                    false,
                ),
                input_form_line("Domain", domain, "optional", *field == 4, *cursor, false),
                input_form_line(
                    "Mount options",
                    opts,
                    "comma-separated, optional",
                    *field == 5,
                    *cursor,
                    false,
                ),
                Line::from("* required   ↑/↓/Tab: field   ←/→/Home/End: cursor"),
                Line::from("Del/Backspace/Ctrl+U: edit   Enter: next/connect   Esc: cancel"),
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

pub(super) fn ntfs_driver_line(driver: &str, active: bool) -> Line<'static> {
    let marker = if active { "> " } else { "  " };
    let selected = Style::default()
        .fg(Color::LightGreen)
        .add_modifier(Modifier::BOLD);
    let available = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let ntfs_3g = if driver == "ntfs-3g" {
        Span::styled("[x] ntfs-3g (compatible)", selected)
    } else {
        Span::styled("[ ] ntfs-3g (compatible)", available)
    };
    let ntfs3 = if driver == "ntfs3" {
        Span::styled("[x] ntfs3 (kernel)", selected)
    } else {
        Span::styled("[ ] ntfs3 (kernel)", available)
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(Color::Yellow)),
        Span::styled("NTFS driver: ", Style::default().fg(Color::DarkGray)),
        ntfs_3g,
        Span::raw("   "),
        ntfs3,
    ])
}

pub(super) fn editable_form_line(
    label: &str,
    value: &str,
    active: bool,
    cursor: usize,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{label}: "),
        Style::default().fg(Color::DarkGray),
    )];
    if !active {
        spans.push(Span::styled(
            value.to_string(),
            Style::default().fg(Color::White),
        ));
        return Line::from(spans);
    }

    let byte_cursor = char_to_byte_index(value, cursor);
    let (before, after) = value.split_at(byte_cursor);
    spans.push(Span::styled(
        before.to_string(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        "│",
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        after.to_string(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

pub(super) fn input_form_line(
    label: &str,
    value: &str,
    placeholder: &str,
    active: bool,
    cursor: usize,
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
    } else {
        Style::default().fg(Color::White)
    };
    let mut spans = vec![Span::styled(format!("{marker} {label}: "), label_style)];
    if active {
        let byte_cursor = char_to_byte_index(value, cursor);
        let (before, after) = value.split_at(byte_cursor);
        spans.push(Span::styled(before.to_string(), value_style));
        spans.push(Span::styled(
            "│",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ));
        if value.is_empty() {
            spans.push(Span::styled(
                format!("<{placeholder}>"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
        } else {
            spans.push(Span::styled(after.to_string(), value_style));
        }
    } else if value.is_empty() {
        spans.push(Span::styled(
            format!("<{placeholder}>"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    } else {
        spans.push(Span::styled(value.to_string(), value_style));
    }
    if invalid {
        spans.push(Span::styled(
            "  !",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}
