use super::*;

#[test]
fn ntfs_defaults_to_ntfs_3g_and_can_switch_to_kernel_driver() {
    assert_eq!(preferred_mount_fstype("ntfs"), "ntfs-3g");
    assert_eq!(preferred_mount_fstype("ntfs3"), "ntfs-3g");
    assert_eq!(toggled_ntfs_driver("ntfs-3g"), "ntfs3");
    assert_eq!(toggled_ntfs_driver("ntfs3"), "ntfs-3g");
}

#[test]
fn failed_statuses_are_rendered_as_errors() {
    assert!(status_is_error("mount failed: Nix(EINVAL)"));
    assert!(status_is_error("IO error: permission denied"));
    assert!(!status_is_error("Mounted"));
    assert!(!status_is_error("Refreshed"));
}

#[test]
fn unmounted_device_keeps_safely_detected_filesystem_in_main_list() {
    let device = BlockDevice {
        name: "sdb1".to_string(),
        path: "/dev/sdb1".to_string(),
        size_bytes: Some(1024),
        removable: true,
        is_partition: true,
        mapper_name: None,
        model: None,
        vendor: None,
        fstype: Some("ntfs".to_string()),
    };

    let entries = build_entries(&[], &[device], false, true, true, true, "");

    assert_eq!(entries.len(), 1);
    assert!(entries[0].mount_points.is_empty());
    assert_eq!(entries[0].fstype.as_deref(), Some("ntfs"));
}

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
        ownership: Ownership::Other(0),
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

#[test]
fn ownership_indicator_distinguishes_current_other_and_mixed_owners() {
    assert_eq!(ownership_from_uids(&[], 1000), Ownership::Unmounted);
    assert_eq!(
        ownership_from_uids(&[Some(1000)], 1000),
        Ownership::CurrentUser
    );
    assert_eq!(ownership_from_uids(&[Some(0)], 1000), Ownership::Other(0));
    assert_eq!(
        ownership_from_uids(&[Some(1000), Some(0)], 1000),
        Ownership::Mixed
    );
    assert_eq!(ownership_from_uids(&[None], 1000), Ownership::Unknown);
}

#[test]
fn local_mount_targets_use_the_desktop_media_directory() {
    assert_eq!(media_mount_target("alice", "sda2"), "/media/alice/sda2");
    assert_eq!(
        media_mount_target("Alice Smith", "Work files"),
        "/media/Alice_Smith/Work_files"
    );
}

#[test]
fn read_only_retry_replaces_rw_and_preserves_other_options() {
    assert_eq!(
        read_only_mount_options("rw,uid=1000,gid=1000,umask=022"),
        "ro,uid=1000,gid=1000,umask=022"
    );
    assert_eq!(read_only_mount_options("defaults"), "ro,defaults");
    assert!(mount_options_are_read_only("nodev,ro,errors=remount-ro"));
    assert!(!mount_options_are_read_only("rw,errors=remount-ro"));
}

#[test]
fn ntfs3_retry_flow_offers_the_requested_safe_and_dangerous_fallbacks() {
    let retries = mount_retry_options("ntfs3", "rw,uid=1000");
    assert_eq!(retries.len(), 2);
    assert_eq!(retries[0].options, "ro,uid=1000");
    assert!(!retries[0].force);
    assert_eq!(retries[1].options, "force,rw,uid=1000");
    assert!(retries[1].force);

    let after_rw_force = mount_retry_options("ntfs3", "force,rw,uid=1000");
    assert_eq!(after_rw_force[0].options, "ro,uid=1000");
    assert!(!after_rw_force[0].force);

    let after_ro = mount_retry_options("ntfs3", "ro,uid=1000");
    assert_eq!(after_ro[0].options, "force,ro,uid=1000");
    assert!(after_ro[0].force);
    assert!(mount_retry_options("ntfs3", "force,ro,uid=1000").is_empty());
}

#[test]
fn non_ntfs3_retry_flow_remains_rw_to_ro_only() {
    assert_eq!(
        mount_retry_options("ext4", "rw,nodev")[0].options,
        "ro,nodev"
    );
    assert!(mount_retry_options("ext4", "ro,nodev").is_empty());
}

#[test]
fn mount_form_editor_changes_text_at_the_cursor() {
    let mut value = "rw,uid=1000".to_string();
    let mut cursor = 2;

    insert_char_at(&mut value, &mut cursor, ',');
    assert_eq!(value, "rw,,uid=1000");
    remove_char_before(&mut value, &mut cursor);
    assert_eq!(value, "rw,uid=1000");
    remove_char_at(&mut value, cursor);
    assert_eq!(value, "rwuid=1000");

    let mut unicode = "том".to_string();
    let mut unicode_cursor = 1;
    insert_char_at(&mut unicode, &mut unicode_cursor, '-');
    assert_eq!(unicode, "т-ом");
    remove_char_before(&mut unicode, &mut unicode_cursor);
    assert_eq!(unicode, "том");
}

#[test]
fn shared_line_editor_supports_navigation_insertion_and_deletion() {
    let mut value = "//nas/shre".to_string();
    let mut cursor = value.chars().count();

    assert!(handle_line_editor_key(
        &mut value,
        &mut cursor,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
    ));
    handle_line_editor_key(
        &mut value,
        &mut cursor,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
    );
    assert!(handle_line_editor_key(
        &mut value,
        &mut cursor,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
    ));
    assert_eq!(value, "//nas/share");

    handle_line_editor_key(
        &mut value,
        &mut cursor,
        KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
    );
    handle_line_editor_key(
        &mut value,
        &mut cursor,
        KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
    );
    assert_eq!(value, "/nas/share");

    handle_line_editor_key(
        &mut value,
        &mut cursor,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    assert!(value.is_empty());
    assert_eq!(cursor, 0);
}

#[test]
fn busy_mount_errors_are_concise_and_recognized() {
    let error = mount_tui::MountError::System(rustix::io::Errno::BUSY);

    assert!(is_resource_busy(&error));
    let message = concise_mount_error(&error);
    assert!(message.to_ascii_lowercase().contains("busy"));
    assert!(!message.contains("System("));
    assert!(!message.contains("Os {"));
}

#[test]
fn proc_path_parsing_handles_spaces_deleted_files_and_boundaries() {
    let mapped =
        mapped_file_path("7f00-7f10 r--p 00000000 08:01 42 /media/alice/My Share/movie (deleted)")
            .unwrap();
    assert!(path_is_inside(mapped, Path::new("/media/alice/My Share")));
    assert!(!path_is_inside(
        Path::new("/media/alice/My Share2/movie"),
        Path::new("/media/alice/My Share"),
    ));
    assert!(mapped_file_path("7f00-7f10 r--p 00000000 00:00 0").is_none());
}

#[test]
fn proc_scan_reports_the_pid_holding_an_open_file() {
    let target = std::env::temp_dir().join(format!(
        "mount-tui-busy-process-test-{}",
        std::process::id()
    ));
    fs::create_dir_all(&target).unwrap();
    let open_file = fs::File::create(target.join("open-file")).unwrap();

    let processes = processes_using_mount(target.to_str().unwrap());

    assert!(
        processes
            .iter()
            .any(|process| process.pid == std::process::id())
    );
    drop(open_file);
    fs::remove_dir_all(target).unwrap();
}
