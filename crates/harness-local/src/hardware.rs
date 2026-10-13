//! Best-effort detection of the machine's compute resources.
//!
//! The setup flow uses this to recommend models that will actually run well —
//! and to auto-pick a quantization that fits — rather than letting the user
//! download a 20 GB model onto an 8 GB laptop.
//!
//! Detection is **macOS-first**: Apple Silicon shares one unified memory pool
//! between CPU and GPU, so RAM *is* the budget. The shape is deliberately
//! platform-agnostic ([`detect`] dispatches per-OS) so Windows/Linux + discrete
//! VRAM detection slot in later without changing callers.

use serde::Serialize;

/// The kind of accelerator the local runtime can offload to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Accelerator {
    /// Apple Metal (Apple Silicon, unified memory).
    Metal,
    /// NVIDIA CUDA (discrete VRAM). Reserved for a later platform.
    Cuda,
    /// CPU only.
    Cpu,
}

/// A snapshot of the machine's memory + accelerator, used to size model picks.
#[derive(Debug, Clone, Serialize)]
pub struct HardwareProfile {
    /// Total physical RAM in bytes.
    pub ram_bytes: u64,
    /// Dedicated VRAM in bytes when distinct from RAM (a discrete GPU). `None`
    /// on unified-memory machines (Apple Silicon), where RAM is the pool.
    pub vram_bytes: Option<u64>,
    /// What we can offload model layers to.
    pub accelerator: Accelerator,
    /// A human label like "Apple M2 Pro" or "CPU".
    pub chip_label: String,
    /// Bytes we'll actually plan against: the GPU/unified pool minus headroom
    /// reserved for the OS and the app, so a "fits" verdict leaves room to work.
    pub usable_budget: u64,
}

const GIB: u64 = 1024 * 1024 * 1024;

/// Headroom policy: keep 25% of the pool (at least 3 GB) free for the OS + app.
fn budget_from_pool(pool: u64) -> u64 {
    let reserve = (pool / 4).max(3 * GIB);
    pool.saturating_sub(reserve)
}

/// Detect this machine's profile. Always returns something (a conservative
/// profile if detection fails) so the UI never has to handle an error.
pub fn detect() -> HardwareProfile {
    #[cfg(target_os = "macos")]
    {
        macos::detect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        fallback()
    }
}

/// A conservative default when we can't probe the platform yet (non-macOS, for
/// now). Assumes a modest CPU-only machine so we never over-recommend.
#[cfg(not(target_os = "macos"))]
fn fallback() -> HardwareProfile {
    let ram = 8 * GIB;
    HardwareProfile {
        ram_bytes: ram,
        vram_bytes: None,
        accelerator: Accelerator::Cpu,
        chip_label: "CPU".to_string(),
        usable_budget: budget_from_pool(ram),
    }
}

/// How hard the OS is squeezing memory right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryPressure {
    Normal,
    /// The OS is compressing and swapping; loading a model makes it worse.
    Warn,
    /// The OS is about to kill apps; loading a model would hurt the user.
    Critical,
    /// The platform gives no reading.
    Unknown,
}

impl MemoryPressure {
    /// The macOS `kern.memorystatus_vm_pressure_level` value (1/2/4).
    fn from_level(level: u64) -> Self {
        match level {
            1 => Self::Normal,
            2 => Self::Warn,
            4 => Self::Critical,
            _ => Self::Unknown,
        }
    }
}

/// The OS's current memory-pressure verdict.
pub fn memory_pressure() -> MemoryPressure {
    #[cfg(target_os = "macos")]
    {
        macos::sysctl_u64("kern.memorystatus_vm_pressure_level")
            .map(MemoryPressure::from_level)
            .unwrap_or(MemoryPressure::Unknown)
    }
    #[cfg(not(target_os = "macos"))]
    {
        MemoryPressure::Unknown
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub fn detect() -> HardwareProfile {
        let ram = sysctl_u64("hw.memsize").unwrap_or(8 * GIB);
        let chip = sysctl_string("machdep.cpu.brand_string")
            .unwrap_or_else(|| "Apple Silicon".to_string());
        // On Apple Silicon the GPU shares system memory (unified), so RAM is the
        // model budget and Metal is the accelerator. Intel Macs fall back to CPU.
        let apple_silicon = cfg!(target_arch = "aarch64");
        HardwareProfile {
            ram_bytes: ram,
            vram_bytes: None,
            accelerator: if apple_silicon {
                Accelerator::Metal
            } else {
                Accelerator::Cpu
            },
            chip_label: chip,
            usable_budget: budget_from_pool(ram),
        }
    }

    fn sysctl_string(key: &str) -> Option<String> {
        let out = std::process::Command::new("sysctl")
            .args(["-n", key])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    pub(super) fn sysctl_u64(key: &str) -> Option<u64> {
        sysctl_string(key)?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_reserves_headroom() {
        // 16 GB pool → reserve 25% (4 GB) → 12 GB usable.
        assert_eq!(budget_from_pool(16 * GIB), 12 * GIB);
        // Small pool → reserve floors at 3 GB.
        assert_eq!(budget_from_pool(8 * GIB), 5 * GIB);
        // Never underflows.
        assert_eq!(budget_from_pool(GIB), 0);
    }

    #[test]
    fn pressure_levels_map_from_the_kernel_values() {
        assert_eq!(MemoryPressure::from_level(1), MemoryPressure::Normal);
        assert_eq!(MemoryPressure::from_level(2), MemoryPressure::Warn);
        assert_eq!(MemoryPressure::from_level(4), MemoryPressure::Critical);
        assert_eq!(MemoryPressure::from_level(0), MemoryPressure::Unknown);
    }

    #[test]
    fn detect_returns_sane_profile() {
        let p = detect();
        assert!(p.ram_bytes > 0);
        assert!(p.usable_budget < p.ram_bytes);
        assert!(!p.chip_label.is_empty());
    }
}
