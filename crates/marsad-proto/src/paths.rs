//! Paths between a Windows GUI machine and the WSL distros it hosts.
//!
//! A WSL host's files are reachable from Windows: `/home/me/a b.txt` in distro `Ubuntu-24.04`
//! is `\\wsl.localhost\Ubuntu-24.04\home\me\a b.txt`, and `/mnt/c/Users/me` is `C:\Users\me`.
//! [`wsl_to_windows`] is the one translation (pure, no I/O); [`windows_to_wsl`] is its inverse
//! for paths a native Windows picker returns; [`resolve_wsl`] adds the filesystem check that
//! tells "the distro is not running" from "the file is not readable".

use std::fmt;
use std::path::PathBuf;

/// The UNC server WSL serves distro files from (`\\wsl$\` is the older spelling).
pub const WSL_SERVER: &str = "wsl.localhost";

/// Why a path could not be translated or reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// Not an absolute Linux path (or a Windows path where a Linux one was needed).
    NotAbsolute(String),
    /// The distro name is empty or contains characters Windows cannot carry in a UNC name.
    BadDistro(String),
    /// A path segment that cannot be written in a Windows path (a backslash, a NUL byte).
    BadSegment(String),
    /// A Windows path that is not inside the distro (or under `C:\` …) it should map to.
    OutsideDistro(String),
    /// Windows cannot see the distro's files: it is not running.
    DistroNotRunning(String),
    /// The file exists nowhere Windows can read, or reading it failed.
    NotReadable { path: String, reason: String },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::NotAbsolute(p) => write!(f, "`{p}` is not an absolute path"),
            PathError::BadDistro(d) => write!(f, "`{d}` is not a usable WSL distro name"),
            PathError::BadSegment(p) => {
                write!(f, "`{p}` cannot be written as a Windows path (backslash or NUL in a name)")
            }
            PathError::OutsideDistro(p) => write!(f, "`{p}` is not inside the WSL distro"),
            PathError::DistroNotRunning(d) => write!(
                f,
                "The WSL distro `{d}` is not running, so Windows cannot see its files. Start it (for example `wsl -d {d}`) and try again."
            ),
            PathError::NotReadable { path, reason } => {
                write!(f, "Windows cannot read `{path}`: {reason}")
            }
        }
    }
}

impl std::error::Error for PathError {}

fn valid_distro(d: &str) -> bool {
    !d.is_empty()
        && d.trim() == d
        && !d
            .chars()
            .any(|c| c.is_control() || r#"\/:*?"<>|"#.contains(c))
}

/// A path that is already in Windows form: a drive (`C:\x`, `c:/x`) or UNC (`\\server\share`).
pub fn is_windows_path(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with("\\\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
}

/// Split an absolute Linux path into its segments, resolving `.`, empty segments and `..`
/// lexically (`..` never climbs above `/`).
fn segments(path: &str) -> Result<Vec<&str>, PathError> {
    if !path.starts_with('/') {
        return Err(PathError::NotAbsolute(path.to_string()));
    }
    let mut out: Vec<&str> = Vec::new();
    for s in path.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s if s.contains('\\') || s.contains('\0') => {
                return Err(PathError::BadSegment(path.to_string()))
            }
            s => out.push(s),
        }
    }
    Ok(out)
}

/// The Windows form of a path inside a WSL distro, for the GUI machine to open:
/// `/mnt/<d>/…` is `<D>:\…`; anything else is `\\wsl.localhost\<distro>\…`. A path that is
/// already a Windows path is returned unchanged. Anything not absolute is rejected.
pub fn wsl_to_windows(distro: &str, path: &str) -> Result<String, PathError> {
    if is_windows_path(path) {
        return Ok(path.to_string());
    }
    let segs = segments(path)?;
    if let Some((drive, rest)) = segs.split_first() {
        if *drive == "mnt" {
            if let Some((d, rest)) = rest.split_first() {
                if d.len() == 1 && d.as_bytes()[0].is_ascii_alphabetic() {
                    let letter = d.to_ascii_uppercase();
                    return Ok(format!("{letter}:\\{}", rest.join("\\")));
                }
            }
        }
    }
    if !valid_distro(distro) {
        return Err(PathError::BadDistro(distro.to_string()));
    }
    Ok(format!("\\\\{WSL_SERVER}\\{distro}\\{}", segs.join("\\")))
}

/// The Linux path of a Windows path a picker returned: `\\wsl.localhost\<distro>\x` (or
/// `\\wsl$\<distro>\x`) of the given distro is `/x`; `C:\x` is `/mnt/c/x`; a Linux path is
/// returned as is. A path in another distro, or on another UNC share, is an error.
pub fn windows_to_wsl(distro: &str, path: &str) -> Result<String, PathError> {
    if path.starts_with('/') {
        return Ok(path.to_string());
    }
    if !is_windows_path(path) {
        return Err(PathError::NotAbsolute(path.to_string()));
    }
    let parts = |s: &str| -> Vec<String> {
        s.split(['\\', '/'])
            .filter(|p| !p.is_empty() && *p != ".")
            .map(str::to_string)
            .collect()
    };
    if let Some(unc) = path.strip_prefix("\\\\") {
        let mut it = parts(unc).into_iter();
        let server = it.next().unwrap_or_default();
        let name = it.next().unwrap_or_default();
        let ours = server.eq_ignore_ascii_case(WSL_SERVER) || server.eq_ignore_ascii_case("wsl$");
        if !ours || !name.eq_ignore_ascii_case(distro) {
            return Err(PathError::OutsideDistro(path.to_string()));
        }
        let rest: Vec<String> = it.collect();
        return Ok(format!("/{}", rest.join("/")));
    }
    let letter = path.as_bytes()[0].to_ascii_lowercase() as char;
    let rest = parts(&path[3..]);
    Ok(if rest.is_empty() {
        format!("/mnt/{letter}")
    } else {
        format!("/mnt/{letter}/{}", rest.join("/"))
    })
}

/// Decide what a failed read of a translated path means: `root_visible` says whether the
/// distro's root (`\\wsl.localhost\<distro>\`) could be read at all.
pub fn classify_failure(
    distro: &str,
    windows_path: &str,
    on_distro: bool,
    root_visible: bool,
    reason: &str,
) -> PathError {
    if on_distro && !root_visible {
        PathError::DistroNotRunning(distro.to_string())
    } else {
        PathError::NotReadable {
            path: windows_path.to_string(),
            reason: reason.to_string(),
        }
    }
}

/// Translate `linux_path` of `distro` and check that this machine can read it (the single
/// entry point of `desktop.open` / `desktop.reveal` / the picker for WSL hosts).
pub fn resolve_wsl(distro: &str, linux_path: &str) -> Result<PathBuf, PathError> {
    let win = wsl_to_windows(distro, linux_path)?;
    match std::fs::metadata(&win) {
        Ok(_) => Ok(PathBuf::from(win)),
        Err(e) => {
            let on_distro = win.starts_with("\\\\");
            let root = format!("\\\\{WSL_SERVER}\\{distro}\\");
            let root_visible = !on_distro || std::fs::metadata(&root).is_ok();
            Err(classify_failure(
                distro,
                &win,
                on_distro,
                root_visible,
                &e.to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "Ubuntu-24.04";

    #[test]
    fn distro_paths_become_unc() {
        assert_eq!(
            wsl_to_windows(D, "/home/me/a.txt").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\home\me\a.txt"
        );
        assert_eq!(
            wsl_to_windows(D, "/").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\"
        );
        assert_eq!(
            wsl_to_windows("my.distro_x", "/etc").unwrap(),
            r"\\wsl.localhost\my.distro_x\etc"
        );
    }

    #[test]
    fn spaces_unicode_and_trailing_slashes_survive() {
        assert_eq!(
            wsl_to_windows(D, "/home/me/My Docs/résumé ✓.pdf").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\home\me\My Docs\résumé ✓.pdf"
        );
        assert_eq!(
            wsl_to_windows(D, "/home/me/dir///").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\home\me\dir"
        );
        assert_eq!(
            wsl_to_windows(D, "/home//me/./x/../y").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\home\me\y"
        );
        assert_eq!(
            wsl_to_windows(D, "/../../etc").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\etc"
        );
    }

    #[test]
    fn mnt_drives_map_to_drive_letters() {
        assert_eq!(
            wsl_to_windows(D, "/mnt/c/Users/me/x y.txt").unwrap(),
            r"C:\Users\me\x y.txt"
        );
        assert_eq!(wsl_to_windows(D, "/mnt/d").unwrap(), r"D:\");
        assert_eq!(wsl_to_windows(D, "/mnt/d/").unwrap(), r"D:\");
        // Not a drive: more than one letter, or not a letter.
        assert_eq!(
            wsl_to_windows(D, "/mnt/wsl/x").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\mnt\wsl\x"
        );
        assert_eq!(
            wsl_to_windows(D, "/mnt/1/x").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\mnt\1\x"
        );
        assert_eq!(
            wsl_to_windows(D, "/mnt").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\mnt"
        );
    }

    #[test]
    fn windows_paths_pass_through() {
        for p in [
            r"C:\Users\me",
            "c:/x/y",
            r"\\server\share\f",
            r"\\wsl.localhost\Debian\etc",
        ] {
            assert_eq!(wsl_to_windows(D, p).unwrap(), p);
        }
    }

    #[test]
    fn rejects_what_is_not_absolute_or_representable() {
        for p in ["", "relative/x", "./x", "~/x", "x"] {
            assert!(
                matches!(wsl_to_windows(D, p), Err(PathError::NotAbsolute(_))),
                "{p}"
            );
        }
        assert!(matches!(
            wsl_to_windows(D, "/a\\b"),
            Err(PathError::BadSegment(_))
        ));
        assert!(matches!(
            wsl_to_windows(D, "/a\0b"),
            Err(PathError::BadSegment(_))
        ));
        for d in ["", " ", "a/b", r"a\b", "a:b", "a\nb", "x "] {
            assert!(
                matches!(wsl_to_windows(d, "/etc"), Err(PathError::BadDistro(_))),
                "{d:?}"
            );
        }
        // A drive path needs no distro.
        assert_eq!(wsl_to_windows("", "/mnt/c/x").unwrap(), r"C:\x");
    }

    #[test]
    fn inverse_maps_back() {
        assert_eq!(
            windows_to_wsl(D, r"\\wsl.localhost\Ubuntu-24.04\home\me").unwrap(),
            "/home/me"
        );
        assert_eq!(
            windows_to_wsl(D, r"\\wsl$\ubuntu-24.04\home\me\").unwrap(),
            "/home/me"
        );
        assert_eq!(
            windows_to_wsl(D, r"\\wsl.localhost\Ubuntu-24.04\").unwrap(),
            "/"
        );
        assert_eq!(
            windows_to_wsl(D, r"\\wsl.localhost\Ubuntu-24.04").unwrap(),
            "/"
        );
        assert_eq!(
            windows_to_wsl(D, r"C:\Users\me\My Docs").unwrap(),
            "/mnt/c/Users/me/My Docs"
        );
        assert_eq!(windows_to_wsl(D, r"d:\").unwrap(), "/mnt/d");
        assert_eq!(
            windows_to_wsl(D, "/already/linux").unwrap(),
            "/already/linux"
        );
        assert!(matches!(
            windows_to_wsl(D, r"\\wsl.localhost\Debian\etc"),
            Err(PathError::OutsideDistro(_))
        ));
        assert!(matches!(
            windows_to_wsl(D, r"\\fileserver\share\x"),
            Err(PathError::OutsideDistro(_))
        ));
        assert!(matches!(
            windows_to_wsl(D, "rel"),
            Err(PathError::NotAbsolute(_))
        ));
    }

    #[test]
    fn round_trips() {
        for p in ["/home/me/x y", "/", "/mnt/c/a/b", "/mnt/e", "/var/lib/ü"] {
            let w = wsl_to_windows(D, p).unwrap();
            assert_eq!(windows_to_wsl(D, &w).unwrap(), p, "{p}");
        }
    }

    #[test]
    fn failures_are_told_apart() {
        let w = r"\\wsl.localhost\X\a";
        assert!(
            matches!(classify_failure("X", w, true, false, "gone"), PathError::DistroNotRunning(d) if d == "X")
        );
        assert!(matches!(
            classify_failure("X", w, true, true, "denied"),
            PathError::NotReadable { .. }
        ));
        assert!(matches!(
            classify_failure("X", r"C:\a", false, false, "denied"),
            PathError::NotReadable { .. }
        ));
        assert!(PathError::DistroNotRunning("X".into())
            .to_string()
            .contains("not running"));
    }
}
