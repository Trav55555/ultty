//! Narrow the Linux Vulkan loader to the detected hardware GPU's ICD.
//!
//! When `mesa-vulkan-drivers` is installed alongside a hardware GPU, the
//! system Vulkan loader (`libvulkan.so`) opens every ICD manifest during
//! adapter enumeration and keeps Mesa's software rasterizer (`libvulkan_lvp.so`
//! and its ~40 MiB `libLLVM.so`) mapped for the lifetime of the process even
//! when the hardware GPU is chosen.
//!
//! Calling [`narrow_to_detected_gpu`] once at startup—while the process is
//! still single-threaded, before any event loop, worker thread, or Vulkan
//! instance exists—sets `VK_DRIVER_FILES` to only the manifests that match the
//! accessible hardware render GPUs. If any loader environment variable is
//! already set, if no accessible hardware GPU is present, if an unrecognized
//! GPU is found, or if no matching ICD manifest exists on disk, the environment
//! is left untouched so the loader falls back to its full default search.

use std::sync::atomic::{AtomicBool, Ordering};

static NARROWED_ENV: AtomicBool = AtomicBool::new(false);

/// Whether Ultty itself set `VK_DRIVER_FILES` during startup.
///
/// Child processes spawned by Ultty strip `VK_DRIVER_FILES` when this is true
/// so internal ICD narrowing does not leak into user shells.
pub fn narrowed_env() -> bool {
    NARROWED_ENV.load(Ordering::Acquire)
}

/// Detect accessible hardware GPUs and, if matching Vulkan ICD manifests exist
/// on disk, narrow `VK_DRIVER_FILES` to them.
///
/// Must be called while the process is single-threaded (before spawning any
/// threads or creating a `wgpu::Instance`). Returns the value written to
/// `VK_DRIVER_FILES`, or `None` when the loader is left at its default search.
#[cfg(target_os = "linux")]
pub fn narrow_to_detected_gpu() -> Option<String> {
    let selected = linux::select_system_icds()?;
    std::env::set_var("VK_DRIVER_FILES", &selected);
    NARROWED_ENV.store(true, Ordering::Release);
    tracing::debug!("Narrowed VK_DRIVER_FILES to detected GPU ICDs: {selected}");
    Some(selected)
}

/// No-op on non-Linux platforms (Metal / DX12 do not use Linux Vulkan ICDs).
#[cfg(not(target_os = "linux"))]
pub fn narrow_to_detected_gpu() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::fs::{self, OpenOptions};
    use std::path::{Path, PathBuf};

    const LOADER_OVERRIDE_VARS: &[&str] = &[
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
        "VK_ADD_DRIVER_FILES",
        "VK_LOADER_DRIVERS_SELECT",
        "VK_LOADER_DRIVERS_DISABLE",
    ];

    const VENDOR_AMD: u16 = 0x1002;
    const VENDOR_NVIDIA: u16 = 0x10de;
    const VENDOR_VIRTIO: u16 = 0x1af4;
    const VENDOR_INTEL: u16 = 0x8086;

    const VIRTUAL_DRM_DRIVERS: &[&str] = &["vgem", "vkms"];

    /// Choose the `:`-separated `VK_DRIVER_FILES` string for this host, or
    /// `None` to leave the Vulkan loader at its full default search.
    pub(super) fn select_system_icds() -> Option<String> {
        select_icds(
            Path::new("/sys/class/drm"),
            Path::new("/dev/dri"),
            Path::new("/dev/nvidiactl"),
            &|key| std::env::var_os(key),
            &can_open_rw,
        )
    }

    fn can_open_rw(path: &Path) -> bool {
        OpenOptions::new().read(true).write(true).open(path).is_ok()
    }

    fn select_icds(
        sys_drm: &Path,
        dev_dri: &Path,
        dev_nvidiactl: &Path,
        env_var: &dyn Fn(&str) -> Option<OsString>,
        node_accessible: &dyn Fn(&Path) -> bool,
    ) -> Option<String> {
        if LOADER_OVERRIDE_VARS
            .iter()
            .any(|key| env_var(key).is_some())
        {
            return None;
        }

        let prefixes = detect_icd_prefixes(sys_drm, dev_dri, dev_nvidiactl, node_accessible)?;
        if prefixes.is_empty() {
            return None;
        }

        let dirs = icd_search_dirs(env_var);
        let manifests = collect_matching_manifests(&dirs, &prefixes);
        if manifests.is_empty() {
            return None;
        }

        let joined = manifests
            .into_iter()
            .filter_map(|p| p.into_os_string().into_string().ok())
            .collect::<Vec<_>>()
            .join(":");
        (!joined.is_empty()).then_some(joined)
    }

    /// Scan render-capable DRM nodes (`renderD*`) and `/dev/nvidiactl` to
    /// determine the ICD filename prefixes for all accessible hardware GPUs.
    ///
    /// Returns `None` if an accessible non-virtual render GPU has an unknown
    /// vendor (so we fall back to all drivers rather than hiding it).
    fn detect_icd_prefixes(
        sys_drm: &Path,
        dev_dri: &Path,
        dev_nvidiactl: &Path,
        node_accessible: &dyn Fn(&Path) -> bool,
    ) -> Option<BTreeSet<&'static str>> {
        let mut prefixes = BTreeSet::new();
        let mut saw_accessible_render_node = false;

        if let Ok(entries) = fs::read_dir(sys_drm) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name_str) = name.to_str() else {
                    continue;
                };
                if !is_render_node_name(name_str) {
                    continue;
                }

                let device_dir = entry.path().join("device");
                let driver = driver_name(&device_dir);
                if driver
                    .as_deref()
                    .is_some_and(|d| VIRTUAL_DRM_DRIVERS.contains(&d))
                {
                    continue;
                }

                let dev_node = dev_dri.join(name_str);
                let accessible = node_accessible(&dev_node)
                    || (driver.as_deref() == Some("nvidia") && node_accessible(dev_nvidiactl));
                if !accessible {
                    continue;
                }
                saw_accessible_render_node = true;

                let vendor = read_hex_u16(&device_dir.join("vendor"))?;
                let matched = vendor_prefixes(vendor, driver.as_deref())?;
                prefixes.extend(matched);
            }
        }

        if !saw_accessible_render_node && node_accessible(dev_nvidiactl) {
            prefixes.insert("nvidia");
        }

        Some(prefixes)
    }

    fn is_render_node_name(name: &str) -> bool {
        name.strip_prefix("renderD")
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    }

    fn driver_name(device_dir: &Path) -> Option<String> {
        let target = fs::read_link(device_dir.join("driver")).ok()?;
        target
            .file_name()
            .and_then(|n| n.to_str())
            .map(ToOwned::to_owned)
    }

    fn read_hex_u16(path: &Path) -> Option<u16> {
        let text = fs::read_to_string(path).ok()?;
        let trimmed = text.trim();
        let hex = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        u16::from_str_radix(hex, 16).ok()
    }

    fn vendor_prefixes(vendor: u16, driver: Option<&str>) -> Option<&'static [&'static str]> {
        match vendor {
            VENDOR_NVIDIA => match driver {
                Some("nvidia") => Some(&["nvidia"]),
                Some("nouveau") => Some(&["nouveau"]),
                _ => Some(&["nvidia", "nouveau"]),
            },
            VENDOR_AMD => Some(&["radeon", "amd"]),
            VENDOR_INTEL => Some(&["intel"]),
            VENDOR_VIRTIO => Some(&["virtio"]),
            _ => None,
        }
    }

    /// Standard Khronos Vulkan loader ICD search directories on Linux.
    fn icd_search_dirs(env_var: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(xdg_config) = env_var("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(xdg_config).join("vulkan/icd.d"));
        } else if let Some(home) = env_var("HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home).join(".config/vulkan/icd.d"));
        }
        dirs.push(PathBuf::from("/etc/vulkan/icd.d"));

        let data_dirs = env_var("XDG_DATA_DIRS")
            .and_then(|v| v.into_string().ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        for dir in data_dirs.split(':').filter(|s| !s.is_empty()) {
            dirs.push(PathBuf::from(dir).join("vulkan/icd.d"));
        }
        dirs
    }

    fn collect_matching_manifests(dirs: &[PathBuf], prefixes: &BTreeSet<&str>) -> Vec<PathBuf> {
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();

        for dir in dirs {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();

            for path in paths {
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !filename_matches(name, prefixes) {
                    continue;
                }
                if !manifest_is_valid(&path) {
                    continue;
                }
                if seen.insert(path.clone()) {
                    result.push(path);
                }
            }
        }
        result
    }

    fn filename_matches(name: &str, prefixes: &BTreeSet<&str>) -> bool {
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".json") || lower.starts_with("lvp") {
            return false;
        }
        prefixes.iter().any(|prefix| lower.starts_with(prefix))
    }

    fn manifest_is_valid(path: &Path) -> bool {
        let Ok(text) = fs::read_to_string(path) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        let Some(library_path) = json
            .get("ICD")
            .and_then(|icd| icd.get("library_path"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            return false;
        };
        let lib = Path::new(library_path);
        !lib.is_absolute() || lib.exists()
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::fs::symlink;
        use std::path::{Path, PathBuf};

        use tempfile::TempDir;

        use super::select_icds;

        fn write_icd(dir: &Path, name: &str, library_path: &str) -> PathBuf {
            fs::create_dir_all(dir).unwrap();
            let path = dir.join(name);
            let content = format!(
                r#"{{"file_format_version":"1.0.1","ICD":{{"library_path":"{library_path}","api_version":"1.4.0"}}}}"#
            );
            fs::write(&path, content).unwrap();
            path
        }

        fn add_render_node(sys_drm: &Path, name: &str, vendor: Option<&str>, driver: Option<&str>) {
            let device = sys_drm.join(name).join("device");
            fs::create_dir_all(&device).unwrap();
            if let Some(v) = vendor {
                fs::write(device.join("vendor"), format!("{v}\n")).unwrap();
            }
            if let Some(d) = driver {
                let target = sys_drm.join("_drivers").join(d);
                fs::create_dir_all(&target).unwrap();
                symlink(&target, device.join("driver")).unwrap();
            }
        }

        fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
            let map: HashMap<String, OsString> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
                .collect();
            move |key| map.get(key).cloned()
        }

        #[test]
        fn test_nvidia_gpu_narrows_to_nvidia_icd_and_skips_lavapipe() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            add_render_node(&sys_drm, "renderD128", Some("0x10de"), Some("nvidia"));
            let nv = write_icd(&icd_dir, "nvidia_icd.json", "libGLX_nvidia.so.0");
            write_icd(&icd_dir, "lvp_icd.json", "libvulkan_lvp.so");
            write_icd(&icd_dir, "radeon_icd.json", "libvulkan_radeon.so");
            write_icd(&icd_dir, "nouveau_icd.json", "libvulkan_nouveau.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| true);
            assert_eq!(selected.as_deref(), Some(nv.to_str().unwrap()));
        }

        #[test]
        fn test_falls_back_when_matching_icd_missing() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            add_render_node(&sys_drm, "renderD128", Some("0x10de"), Some("nvidia"));
            write_icd(&icd_dir, "lvp_icd.json", "libvulkan_lvp.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| true);
            assert_eq!(selected, None);
        }

        #[test]
        fn test_falls_back_when_no_hardware_gpu_present() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            fs::create_dir_all(&sys_drm).unwrap();
            write_icd(&icd_dir, "lvp_icd.json", "libvulkan_lvp.so");
            write_icd(&icd_dir, "radeon_icd.json", "libvulkan_radeon.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| false);
            assert_eq!(selected, None);
        }

        #[test]
        fn test_respects_user_vulkan_loader_env_vars() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            add_render_node(&sys_drm, "renderD128", Some("0x1002"), Some("amdgpu"));
            write_icd(&icd_dir, "radeon_icd.json", "libvulkan_radeon.so");

            for var in [
                "VK_DRIVER_FILES",
                "VK_ICD_FILENAMES",
                "VK_ADD_DRIVER_FILES",
                "VK_LOADER_DRIVERS_SELECT",
                "VK_LOADER_DRIVERS_DISABLE",
            ] {
                let env = env_from(&[
                    ("XDG_DATA_DIRS", share.to_str().unwrap()),
                    (var, "/custom/icd.json"),
                ]);
                let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| true);
                assert_eq!(selected, None, "must respect existing {var}");
            }
        }

        #[test]
        fn test_hybrid_intel_and_nvidia_includes_both_and_skips_lavapipe() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            add_render_node(&sys_drm, "renderD128", Some("0x8086"), Some("i915"));
            add_render_node(&sys_drm, "renderD129", Some("0x10de"), Some("nvidia"));
            let intel = write_icd(&icd_dir, "intel_icd.json", "libvulkan_intel.so");
            let nv = write_icd(&icd_dir, "nvidia_icd.json", "libGLX_nvidia.so.0");
            write_icd(&icd_dir, "lvp_icd.json", "libvulkan_lvp.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| true).unwrap();
            assert_eq!(selected, format!("{}:{}", intel.display(), nv.display()));
        }

        #[test]
        fn test_falls_back_on_unknown_gpu_vendor() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            add_render_node(&sys_drm, "renderD128", Some("0x13b5"), Some("panfrost"));
            write_icd(&icd_dir, "radeon_icd.json", "libvulkan_radeon.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| true);
            assert_eq!(selected, None);
        }

        #[test]
        fn test_ignores_virtual_vgem_and_missing_absolute_library_path() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            add_render_node(&sys_drm, "renderD128", None, Some("vgem"));
            add_render_node(&sys_drm, "renderD129", Some("0x1002"), Some("amdgpu"));
            write_icd(&icd_dir, "amd_broken_icd.json", "/nonexistent/libamd.so");
            let radeon = write_icd(&icd_dir, "radeon_icd.json", "libvulkan_radeon.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|_| true);
            assert_eq!(selected.as_deref(), Some(radeon.to_str().unwrap()));
        }

        #[test]
        fn test_nvidia_without_drm_render_node_uses_nvidiactl() {
            let tmp = TempDir::new().unwrap();
            let sys_drm = tmp.path().join("sys/class/drm");
            let dev_dri = tmp.path().join("dev/dri");
            let nvidiactl = tmp.path().join("dev/nvidiactl");
            let share = tmp.path().join("share");
            let icd_dir = share.join("vulkan/icd.d");

            fs::create_dir_all(&sys_drm).unwrap();
            let nv = write_icd(&icd_dir, "nvidia_icd.json", "libGLX_nvidia.so.0");
            write_icd(&icd_dir, "lvp_icd.json", "libvulkan_lvp.so");

            let env = env_from(&[("XDG_DATA_DIRS", share.to_str().unwrap())]);
            let selected = select_icds(&sys_drm, &dev_dri, &nvidiactl, &env, &|p| p == nvidiactl);
            assert_eq!(selected.as_deref(), Some(nv.to_str().unwrap()));
        }
    }
}
