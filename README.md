# mount-tui

[Русская версия](README.ru.md)

`mount-tui` is an interactive Linux terminal interface for inspecting, mounting,
and unmounting local block devices and SMB/CIFS shares.

## Features

- Lists disks, partitions, mount points, filesystem types, sizes, and device metadata.
- Shows whether every mounted directory belongs to the invoking desktop user.
- Detects filesystems on unmounted devices from udev metadata without probing or
  blocking on the device itself.
- Mounts local filesystems with editable targets and options.
- Uses desktop-friendly `/media/<user>/<label-or-device>` mount targets by default.
- Offers an explicit NTFS driver selector: compatible `ntfs-3g` (default) or
  the in-kernel `ntfs3` driver.
- Connects and reconnects SMB/CIFS shares using an embedded credential form.
- Passes SMB passwords through a temporary mode `0600` credentials file rather
  than exposing them in process arguments.
- Grants the desktop user access through suitable UID/GID mount options or by
  changing only the mount root owner on Unix-native filesystems. The action is
  disabled when ownership is already correct.
- Supports filtering, terminal-native mouse text selection/copying, optional
  pseudo-filesystems, and detailed device information.

## Requirements

- Linux
- A terminal with UTF-8 and color support
- Root privileges for mount, unmount, and ownership operations
- Rust 1.85 or newer to build from source (edition 2024)

Optional runtime helpers:

- `ntfs-3g` for the default, most compatible NTFS mode
- `cifs-utils` (`mount.cifs`) for SMB/CIFS shares

Examples for Debian/Ubuntu:

```bash
sudo apt install ntfs-3g cifs-utils
```

Fedora:

```bash
sudo dnf install ntfs-3g cifs-utils
```

Arch Linux:

```bash
sudo pacman -S ntfs-3g cifs-utils
```

## Build and run

```bash
cargo build --release
sudo ./target/release/mount-tui
```

With [`just`](https://github.com/casey/just), build and install the preferred
format in one command:

```bash
just install       # release binary in /usr/local/bin
just install-deb   # build and install a DEB package
just install-rpm   # build and install an RPM package
```

Set `PREFIX` to change the binary installation prefix, for example
`PREFIX=/usr just install`. Packaging tools are installed through Cargo on
first use if they are not already available.

The application may be started without root. When a privileged operation is
requested, press `R` in the privilege dialog to restart it through `sudo`.
Starting it from a regular user's `sudo` session preserves `SUDO_UID` and
`SUDO_GID`, allowing access to be granted to that user instead of root.

## Controls

| Key | Action |
| --- | --- |
| `↑` / `↓`, `PageUp` / `PageDown`, `Home` / `End` | Navigate devices |
| `f` | Filter the device list |
| `r` | Refresh mounts and devices |
| `m` | Mount the selected local device |
| `n` | Connect a new SMB/CIFS share |
| `u` | Unmount the selected target |
| `a` | Grant the invoking desktop user access |
| `i` | Show or hide extended device information |
| `d`, `t`, `s`, `p` | Toggle disks, partitions, SMB, or pseudo-filesystems |
| `q` | Quit |

Forms support `↑` / `↓`, `Tab` / `Shift+Tab`, `Home` / `End`, and `Ctrl+U` to
clear the active field. In the local mount form, NTFS drivers are displayed as
an explicit `[x]` / `[ ]` selector and can be changed with `Space`, `←`, or `→`.
Text fields have a visible cursor and support in-place editing with `←` / `→`,
`Home` / `End`, `Backspace`, and `Delete`. If a read-write local mount fails,
the application offers an interactive retry with the same options in read-only
mode.

## SMB/CIFS

Enter a share as `//server/share`; `smb://server/share` is also accepted. An
empty username selects guest access. Required and invalid values are highlighted
before a mount command is started.

By default, SMB mounts use the invoking user's UID/GID, `file_mode=0664`, and
`dir_mode=0775`. The access action reconnects an existing SMB mount using the
embedded credential form. Server-side ACLs still take precedence over client
mount options.

## NTFS drivers

For an NTFS volume, `ntfs-3g` is selected by default for compatibility. Select
`ntfs3` in the mount form to use the newer in-kernel driver. `ntfs-3g` is a
userspace FUSE driver and must be installed separately; the program invokes its
system mount helper when selected.

## License

Licensed under the [Apache License 2.0](LICENSE).
