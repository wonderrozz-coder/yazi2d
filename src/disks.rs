//! Mounted filesystems, shown in the sidebar under "Disks".
//!
//! A dual-pane manager is often used to move data between volumes, so the disks
//! of the machine belong next to the bookmarks. The list is read from the
//! kernel's mount table on Linux (`/proc/self/mounts`), from `mount` on macOS
//! and from `wmic`/PowerShell drive enumeration on Windows, then filtered down
//! to entries a user would recognise:
//!
//! * pseudo filesystems (`proc`, `sysfs`, `tmpfs`, `devtmpfs`, `overlay`…) are
//!   dropped, because they have no user-visible content;
//! * `/boot`, `/snap`, `/var/lib/*` and similar system mounts are dropped;
//! * anything the kernel marks as a read-only squashfs (a snap package) is
//!   dropped as well.
//!
//! The label prefers the volume name over the device node: `/dev/nvme0n1p2 on
//! /` reads as "Root", a USB stick as "USB DISK".

use std::path::PathBuf;

/// One mounted volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    /// Where it is mounted; navigating there opens the volume.
    pub mount: PathBuf,
    /// Human label: volume name, partition label, or the device's last part.
    pub label: String,
    /// Filesystem type, shown to the right.
    pub kind: String,
    /// Free / total in bytes, when the platform reports them.
    pub free: Option<u64>,
    pub total: Option<u64>,
}

impl Disk {
    /// `"120G free of 500G"`, or empty when sizes are unknown.
    pub fn usage(&self) -> String {
        match (self.free, self.total) {
            (Some(free), Some(total)) if total > 0 => format!(
                "{} free of {}",
                crate::ui::human_size(free),
                crate::ui::human_size(total)
            ),
            _ => String::new(),
        }
    }
}

/// Every mounted volume worth showing, in mount-table order.
pub fn list() -> Vec<Disk> {
    let mut disks = platform_mounts();
    // A mount can appear twice (bind mounts); keep the first occurrence.
    disks.dedup_by(|a, b| a.mount == b.mount);
    for disk in &mut disks {
        if let Some((free, total)) = free_space(&disk.mount) {
            disk.free = Some(free);
            disk.total = Some(total);
        }
    }
    disks
}

/// Free and total bytes of the filesystem holding `path`.
///
/// Uses `statvfs` on Unix; Windows falls back to `None`, where the sidebar then
/// shows the mount point only.
#[cfg(unix)]
fn free_space(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // `statvfs` is stable and available on every Unix we target.
    let mut stats: libc_statvfs = unsafe { std::mem::zeroed() };
    let result = unsafe { statvfs(c_path.as_ptr(), &mut stats) };
    if result != 0 {
        return None;
    }
    let block = stats.f_frsize.max(1);
    let total = stats.f_blocks.saturating_mul(block);
    let free = stats.f_bavail.saturating_mul(block);
    Some((free, total))
}

#[cfg(not(unix))]
fn free_space(_path: &std::path::Path) -> Option<(u64, u64)> {
    None
}

/// `struct statvfs` with the layout every Linux/macOS target shares.
#[cfg(unix)]
#[repr(C)]
struct libc_statvfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
    f_spare: [i32; 6],
}

#[cfg(unix)]
unsafe extern "C" {
    fn statvfs(path: *const std::os::raw::c_char, buffer: *mut libc_statvfs) -> i32;
}

/// Mounts the kernel knows about, filtered to user-visible volumes.
fn platform_mounts() -> Vec<Disk> {
    #[cfg(target_os = "linux")]
    {
        linux_mounts()
    }
    #[cfg(target_os = "macos")]
    {
        macos_mounts()
    }
    #[cfg(target_os = "windows")]
    {
        windows_mounts()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn linux_mounts() -> Vec<Disk> {
    let Ok(table) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };

    let mut devices: Vec<(Disk, String)> = table
        .lines()
        .filter_map(|line| {
            // Fields are space separated; the first four are device, mount
            // point, type and options, all escaped with `\040` for spaces.
            let mut fields = line.split_whitespace();
            let device = unescape(fields.next()?);
            let mount = unescape(fields.next()?);
            let kind = fields.next()?.to_string();
            let options = fields.next().unwrap_or_default();

            if !interesting(&mount, &kind, options) {
                return None;
            }
            let label = mount_label(&mount, &device);
            Some((
                Disk {
                    label,
                    mount: PathBuf::from(mount),
                    kind,
                    free: None,
                    total: None,
                },
                // The device is what identifies one physical volume.
                device,
            ))
        })
        .collect();

    devices.sort_by_key(|(disk, _)| rank(&disk.mount));

    // One row per *device*: a btrfs subvolume (`/home`) is the same disk as its
    // root, and listing it twice would be a lie about the hardware. The first
    // mount in `rank` order wins, which is `/` for the root filesystem.
    let mut seen: Vec<String> = Vec::new();
    let mut disks = Vec::new();
    for (disk, device) in devices {
        let key = device.clone();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        disks.push(disk);
    }
    disks
}

/// Is this mount something a user would open in a file manager?
#[cfg(target_os = "linux")]
pub(crate) fn interesting(mount: &str, kind: &str, options: &str) -> bool {
    const PSEUDO: [&str; 22] = [
        "proc",
        "sysfs",
        "devtmpfs",
        "devpts",
        "tmpfs",
        "cgroup",
        "cgroup2",
        "debugfs",
        "tracefs",
        "securityfs",
        "pstore",
        "bpf",
        "autofs",
        "mqueue",
        "hugetlbfs",
        "configfs",
        "fusectl",
        "binfmt_misc",
        "efivarfs",
        "ramfs",
        "selinuxfs",
        "rpc_pipefs",
    ];
    if PSEUDO.contains(&kind) {
        return false;
    }
    // Overlay/squashfs are snap and container internals, not user volumes.
    if kind == "squashfs" || kind == "overlay" {
        return false;
    }
    if mount == "/boot" || mount == "/boot/efi" || mount.starts_with("/snap") {
        return false;
    }
    // System subtrees that happen to live on a real filesystem.
    if mount.starts_with("/var/") || mount.starts_with("/sys/") || mount.starts_with("/proc/") {
        return false;
    }
    // FUSE placeholders (gvfs, portal documents, flatpak) are portals, not
    // volumes; showing them would fill the list with empty directories.
    if kind.starts_with("fuse") {
        return false;
    }
    // Btrfs subvolumes share their device with the root volume, so they are the
    // same disk under a different name.
    if mount.starts_with("/run/user/") || mount.starts_with("/run/media/") && kind == "autofs" {
        return false;
    }
    // A read-only tmpfs or squashfs holds no user data.
    if options.split(',').any(|option| option == "ro") && kind == "tmpfs" {
        return false;
    }
    true
}

/// Sort key: root first, then the usual home mounts, then everything else.
#[cfg(target_os = "linux")]
fn rank(mount: &std::path::Path) -> (u8, String) {
    let path = mount.to_string_lossy();
    let priority = match path.as_ref() {
        "/" => 0,
        "/home" => 1,
        "/mnt" | "/media" => 2,
        _ if path.starts_with("/media") => 3,
        _ if path.starts_with("/mnt") => 4,
        _ if path.starts_with("/run/media") => 5,
        _ => 6,
    };
    (priority, path.to_string())
}

#[cfg(target_os = "linux")]
fn mount_label(mount: &str, device: &str) -> String {
    if mount == "/" {
        return "Root".to_string();
    }
    mount
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(|part| part.replace('_', " "))
        .unwrap_or_else(|| device.to_string())
}

/// Decode the kernel's octal escapes (`\040` is a space).
#[cfg(target_os = "linux")]
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let bytes = field.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &field[index + 1..index + 4];
            if let Ok(code) = u8::from_str_radix(digits, 8) {
                out.push(code as char);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

#[cfg(target_os = "macos")]
fn macos_mounts() -> Vec<Disk> {
    let Ok(output) = std::process::Command::new("mount").output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            // `/dev/disk3s1 on / (apfs, local, journaled)`
            let (device, rest) = line.split_once(" on ")?;
            let (mount, info) = rest.split_once(" (")?;
            let kind = info.split(',').next().unwrap_or_default().to_string();
            if mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data" {
                return None;
            }
            if mount == "/private/var/vm" {
                return None;
            }
            Some(Disk {
                label: if mount == "/" {
                    "Root".to_string()
                } else {
                    mount
                        .rsplit('/')
                        .find(|part| !part.is_empty())
                        .unwrap_or(mount)
                        .replace('_', " ")
                },
                mount: PathBuf::from(mount),
                kind,
                free: None,
                total: None,
            })
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_mounts() -> Vec<Disk> {
    // `wmic` is deprecated but present everywhere from 7 to 11; the fallback is
    // PowerShell for machines where it was removed.
    let output = std::process::Command::new("wmic")
        .args(["logicaldisk", "get", "DeviceID,VolumeName,FileSystem", "/format:csv"])
        .output()
        .or_else(|_| {
            std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    "Get-Volume | Select-Object DriveLetter,FriendlyName,FileSystem | ConvertTo-Csv -NoTypeInformation",
                ])
                .output()
        });
    let Ok(output) = output else {
        return Vec::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(|field| field.trim()).collect();
            // CSV: Node,DeviceID,FileSystem,VolumeName / or DriveLetter,...,FileSystem
            let letter = fields
                .iter()
                .find(|field| field.len() == 2 && field.ends_with(':'))?;
            let name = fields
                .iter()
                .rev()
                .find(|field| !field.is_empty() && !field.contains(':') && **field != "FileSystem")
                .map(|field| field.trim_matches('"').to_string())
                .filter(|name| !name.is_empty());
            Some(Disk {
                mount: PathBuf::from(format!("{letter}\\")),
                label: name.unwrap_or_else(|| letter.to_string()),
                kind: fields
                    .iter()
                    .find(|field| **field == "NTFS" || **field == "FAT32" || **field == "exFAT")
                    .unwrap_or(&"")
                    .to_string(),
                free: None,
                total: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_usage_line_is_readable() {
        let disk = Disk {
            mount: PathBuf::from("/"),
            label: "Root".to_string(),
            kind: "ext4".to_string(),
            free: Some(120 * 1024 * 1024 * 1024),
            total: Some(500 * 1024 * 1024 * 1024),
        };
        assert_eq!(disk.usage(), "120G free of 500G");

        let unknown = Disk {
            free: None,
            total: None,
            ..disk
        };
        assert!(unknown.usage().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pseudo_filesystems_and_system_mounts_are_hidden() {
        assert!(!interesting("/proc", "proc", "rw"));
        assert!(!interesting("/sys", "sysfs", "rw"));
        assert!(!interesting("/dev", "devtmpfs", "rw"));
        assert!(!interesting("/run", "tmpfs", "rw"));
        assert!(!interesting("/boot", "vfat", "rw"));
        assert!(!interesting("/snap/core/1", "squashfs", "ro"));
        assert!(!interesting("/var/lib/docker", "ext4", "rw"));
        assert!(interesting("/", "ext4", "rw"));
        assert!(interesting("/home", "btrfs", "rw"));
        assert!(interesting("/run/media/me/USB", "vfat", "rw"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_root_volume_is_labelled_and_ranked_first() {
        assert_eq!(mount_label("/", "/dev/nvme0n1p2"), "Root");
        assert_eq!(
            mount_label("/run/media/me/USB DISK", "/dev/sdb1"),
            "USB DISK"
        );
        assert!(rank(std::path::Path::new("/")) < rank(std::path::Path::new("/home/me")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_escapes_are_decoded() {
        assert_eq!(
            unescape("/run/media/me/My\\040Disk"),
            "/run/media/me/My Disk"
        );
        assert_eq!(unescape("/plain"), "/plain");
    }

    #[test]
    fn the_list_never_repeats_a_mount() {
        let disks = list();
        let mut mounts: Vec<PathBuf> = disks.iter().map(|disk| disk.mount.clone()).collect();
        let before = mounts.len();
        mounts.sort();
        mounts.dedup();
        assert_eq!(before, mounts.len(), "duplicate mounts in {disks:?}");
    }
}
