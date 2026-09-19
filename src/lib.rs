//! mount_tui — библиотека для работы с монтированными файловыми системами.
//! Предоставляет список монтированных файловых систем и простые вызовы mount/umount.

pub mod fstab;
pub mod mountmgr;
pub mod ssh;

pub use fstab::{FstabEntry, FstabRecord, read_fstab, remove_fstab_entry, replace_fstab_entry};
pub use mountmgr::{
    BlockDevice, MountEntry, MountError, MountManager, UserAccessMethod, is_smb_fstype,
    is_sshfs_fstype, ownership_options, uses_mount_ownership,
};
pub use ssh::{SshHost, parse_ssh_config, read_ssh_config};
