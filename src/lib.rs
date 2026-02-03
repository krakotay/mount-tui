//! mount_tui — библиотека для работы с монтированными файловыми системами.
//! Предоставляет список монтированных файловых систем и простые вызовы mount/umount.

pub mod mountmgr;

pub use mountmgr::{BlockDevice, MountEntry, MountError, MountManager};
