use std::path::PathBuf;

#[cfg(windows)]
pub fn default_install_directory() -> Result<PathBuf, String> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    };

    const DRIVE_FIXED_TYPE: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(format!(
            "Windows не смог перечислить диски: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    for index in 0..26 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = OsStr::new(&root).encode_wide().chain(Some(0)).collect();
        if unsafe { GetDriveTypeW(wide.as_ptr()) } != DRIVE_FIXED_TYPE {
            continue;
        }

        let mut available = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        let result =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
        if result == 0 {
            failures.push(format!("{letter}: {}", std::io::Error::last_os_error()));
            continue;
        }
        candidates.push((letter, available));
    }

    if candidates.is_empty() {
        let detail = if failures.is_empty() {
            "не найдено доступных локальных фиксированных дисков".to_string()
        } else {
            failures.join("; ")
        };
        return Err(format!(
            "Не удалось определить диск для установки: {detail}. Выберите папку вручную."
        ));
    }

    let selected = select_largest_drive(&mut candidates).ok_or_else(|| {
        "Не удалось определить диск для установки. Выберите папку вручную.".to_string()
    })?;
    Ok(PathBuf::from(format!("{}:\\Games\\1stMay", selected)))
}

fn select_largest_drive(candidates: &mut [(char, u64)]) -> Option<char> {
    use std::cmp::Ordering;
    candidates.sort_by(
        |(left_letter, left_space), (right_letter, right_space)| match right_space.cmp(left_space) {
            Ordering::Equal => left_letter.cmp(right_letter),
            ordering => ordering,
        },
    );
    candidates.first().map(|(letter, _)| *letter)
}

#[cfg(not(windows))]
pub fn default_install_directory() -> Result<PathBuf, String> {
    Err(
        "Автоматический выбор диска поддерживается только в Windows. Выберите папку вручную."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::select_largest_drive;

    #[test]
    fn largest_drive_selection_breaks_ties_by_letter() {
        let mut candidates = [('D', 100), ('C', 100), ('E', 80)];
        assert_eq!(select_largest_drive(&mut candidates), Some('C'));
    }
}
