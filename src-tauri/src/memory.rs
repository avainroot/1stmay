use crate::config::TARGET_HEAP_MIB;
use serde::Serialize;

const MIB: u64 = 1024 * 1024;
const RESERVED_MIB: u64 = 1536;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    pub total_mib: u64,
    pub available_mib: u64,
    pub heap_mib: u64,
}

pub fn calculate_heap_mib(total_bytes: u64, available_bytes: u64) -> u64 {
    let physical_budget = total_bytes / MIB;
    let available_budget = available_bytes / MIB;
    let safe_physical = leave_memory_for_system(physical_budget);
    let safe_available = leave_memory_for_system(available_budget);
    TARGET_HEAP_MIB
        .min(safe_physical)
        .min(safe_available)
        .max(1)
}

fn leave_memory_for_system(memory_mib: u64) -> u64 {
    memory_mib.saturating_sub(RESERVED_MIB.min(memory_mib / 4))
}

#[cfg(windows)]
pub fn memory_status() -> Result<MemoryStatus, String> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(format!(
            "Не удалось определить доступную память Windows: {}",
            std::io::Error::last_os_error()
        ));
    }
    let heap_mib = calculate_heap_mib(status.ullTotalPhys, status.ullAvailPhys);
    Ok(MemoryStatus {
        total_mib: status.ullTotalPhys / MIB,
        available_mib: status.ullAvailPhys / MIB,
        heap_mib,
    })
}

pub fn available_heap_mib() -> Result<u64, String> {
    Ok(memory_status()?.heap_mib)
}

#[cfg(not(windows))]
pub fn memory_status() -> Result<MemoryStatus, String> {
    Err("Определение доступной памяти поддерживается только в Windows".into())
}

#[cfg(not(windows))]
pub fn available_heap_mib() -> Result<u64, String> {
    Err("Определение доступной памяти поддерживается только в Windows".into())
}

#[cfg(test)]
mod tests {
    use super::calculate_heap_mib;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn memory_allocation_targets_eight_gib_with_available_memory_bounds() {
        assert_eq!(calculate_heap_mib(16_384 * MIB, 8_000 * MIB), 6464);
        assert_eq!(calculate_heap_mib(32_768 * MIB, 24_000 * MIB), 8192);
        assert_eq!(calculate_heap_mib(8192 * MIB, 6000 * MIB), 4500);
    }

    #[test]
    fn uses_available_memory_below_the_previous_minimum_without_error() {
        assert_eq!(calculate_heap_mib(8192 * MIB, 2048 * MIB), 1536);
        assert_eq!(calculate_heap_mib(8192 * MIB, 3072 * MIB), 2304);
    }

    #[test]
    fn always_produces_a_valid_positive_heap_value() {
        assert_eq!(calculate_heap_mib(0, 0), 1);
        assert_eq!(calculate_heap_mib(8192 * MIB, 0), 1);
    }

    #[test]
    fn leaves_a_proportional_system_reserve_when_memory_is_low() {
        assert_eq!(calculate_heap_mib(4096 * MIB, 3072 * MIB), 2304);
        assert_eq!(calculate_heap_mib(8192 * MIB, 5632 * MIB), 4224);
    }
}
