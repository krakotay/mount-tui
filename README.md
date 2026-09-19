# mount-tui
<img width="1920" height="976" alt="image" src="https://github.com/user-attachments/assets/b151178e-d8c4-4fae-b882-7a207cd56fd4" />

[Русская версия](README.ru.md)

`mount-tui` is an interactive Linux terminal interface for inspecting, mounting,
and unmounting local block devices, SMB/CIFS shares, and SSH filesystems.

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
- Reads concrete hosts from the invoking user's `~/.ssh/config` and mounts
  remote directories through SSHFS. Manual `[user@]host` entry is also available.
- Shows a copyable raw `/etc/fstab` line for a selected mount, exports it to a
  new file, or appends it to `/etc/fstab` after duplicate checking.
- SSHFS mounts can be made permanent directly from the SSH form.
- Reads `/etc/fstab` into an optional, off-by-default view with validated raw
  editing and confirmed removal.
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
- `sshfs` for SSH filesystems

Examples for Debian/Ubuntu:

```bash
sudo apt install ntfs-3g cifs-utils sshfs
```

Fedora:

```bash
sudo dnf install ntfs-3g cifs-utils fuse-sshfs
```

Arch Linux:

```bash
sudo pacman -S ntfs-3g cifs-utils sshfs
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
| `h` | Choose an SSH host from `~/.ssh/config` or enter one manually |
| `u` | Unmount the selected target |
| `a` | Grant the invoking desktop user access |
| `x` | Show/export the raw fstab entry or add it to `/etc/fstab` |
| `b` | Toggle the `/etc/fstab` view (off by default) |
| `e` / `Delete` | Edit or remove the selected fstab entry |
| `i` | Show or hide extended device information |
| `d`, `t`, `s`, `p` | Toggle disks, partitions, SMB, or pseudo-filesystems |
| `q` | Quit |

Forms support `↑` / `↓`, `Tab` / `Shift+Tab`, `Home` / `End`, and `Ctrl+U` to
clear the active field. In the local mount form, NTFS drivers are displayed as
an explicit `[x]` / `[ ]` selector and can be changed with `Space`, `←`, or `→`.
Text fields have a visible cursor and support in-place editing with `←` / `→`,
`Home` / `End`, `Backspace`, and `Delete`. If a read-write local mount fails,
the application offers an interactive retry with the same options in read-only
mode. For the `ntfs3` driver, a failed read-write mount offers either read-only
or a clearly marked, not-recommended `DANGER` retry using `force`. If that forced
read-write attempt fails, read-only is offered; a failed ordinary read-only
attempt offers a final `DANGER` read-only retry using `force`.

## SMB/CIFS

Enter a share as `//server/share`; `smb://server/share` is also accepted. An
empty username selects guest access. Required and invalid values are highlighted
before a mount command is started.

By default, SMB mounts use the invoking user's UID/GID, `file_mode=0664`, and
`dir_mode=0775`. The access action reconnects an existing SMB mount using the
embedded credential form. Server-side ACLs still take precedence over client
mount options.

Authenticated SMB passwords are never copied into exported fstab text. Add a
root-readable `credentials=/path` option before using such an exported entry at
boot; guest SMB entries need no credentials file.

## SSHFS

Press `h` to choose a concrete (non-wildcard) `Host` alias read from the
invoking user's `~/.ssh/config`, or select manual entry. The form accepts the
remote path, local target, an optional password/key passphrase, SSHFS options,
and a permanent `/etc/fstab` toggle. Secrets are masked and passed to `sshfs`
only through stdin, never through process arguments. Leaving the secret blank
forces non-interactive SSH key/agent authentication, so an OpenSSH prompt can
never take over the TUI.

Resolved `User`, `HostName`, `Port`, and `IdentityFile` values are displayed or
carried into the mount defaults, while the SSH config itself is passed to SSH
so options such as `ProxyJump` continue to work. The permanent toggle is
disabled for password authentication or when there is no usable private key.
Enabling it validates that the private key needs no passphrase and that the
server accepts that exact key; a missing or wrong key leaves the toggle off.
This server check runs in the background with an animated progress dialog; a
failure dialog shows the SSH error and keeps `/etc/fstab` disabled.
Permanent entries include `_netdev` and `nofail`.
They also add `allow_other` and `default_permissions` together with the original
user's UID/GID, because boot-time fstab mounts are created by root.

When mount-tui runs through `sudo`, SSHFS itself is launched as the invoking
desktop user and the mountpoint is assigned to that user first. Consequently,
FUSE records the correct `user_id`, and the mounted directory is immediately
usable without replaying kernel-generated FUSE options.

## Export and permanent mounts

Press `x` on any listed filesystem to inspect its exact raw fstab line. `E`
exports it to a new mode-`0600` file without overwriting existing files. `P`
appends it to `/etc/fstab`, refusing an existing source/target pair. Local block
devices use `UUID=` when udev provides one. Network entries are marked
`_netdev`. This action creates a missing mount-point directory but does not run
`mount -a`.

Press `b` to show configured `/etc/fstab` entries alongside live devices; the
view is hidden by default. Select an entry and press `e` to edit its complete
raw line, or `Delete` to remove it after confirmation. Changes are validated,
written atomically, and preserve comments and unrelated lines. Before the first
edit or removal, mount-tui creates `/etc/fstab.mount-tui.bak` if it does not
already exist. When running without root, edit and remove open the privilege
dialog so mount-tui can be restarted through `sudo`.

## NTFS drivers

For an NTFS volume, `ntfs-3g` is selected by default for compatibility. Select
`ntfs3` in the mount form to use the newer in-kernel driver. `ntfs-3g` is a
userspace FUSE driver and must be installed separately; the program invokes its
system mount helper when selected.

## License

Licensed under the [Apache License 2.0](LICENSE).
