//! Identifies who is on the other end of the socket, so app-only methods
//! (approving sends, flipping the send gate) can refuse agents.

use std::path::PathBuf;

/// Executable path of a process id, via libproc.
pub fn exe_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n =
        unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

/// True when the executable is the Mermoz app bundle's main binary.
/// `MERMOZ_TRUSTED_EXE` names one extra exact path, for end-to-end tests.
pub fn is_app(exe: &std::path::Path) -> bool {
    if let Ok(t) = std::env::var("MERMOZ_TRUSTED_EXE")
        && exe == std::path::Path::new(&t)
    {
        return true;
    }
    is_app_bundle_path(&exe.to_string_lossy())
}

pub fn is_app_bundle_path(p: &str) -> bool {
    p.ends_with("/Mermoz.app/Contents/MacOS/Mermoz")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::bundle("/Applications/Mermoz.app/Contents/MacOS/Mermoz", true)]
    #[case::cli_inside_bundle("/Applications/Mermoz.app/Contents/Resources/mermoz", false)]
    #[case::lookalike("/tmp/NotMermoz.app/Contents/MacOS/Mermoz2", false)]
    #[case::random_binary("/usr/local/bin/mermoz", false)]
    fn bundle_path_detection(#[case] path: &str, #[case] want: bool) {
        assert_eq!(is_app_bundle_path(path), want);
    }

    #[rstest]
    fn own_exe_path_is_found() {
        assert!(exe_path(std::process::id() as i32).is_some());
    }
}
