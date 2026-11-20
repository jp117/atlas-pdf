//! The launcher entry the Linux AppImage writes for itself (`apps/pdfcraft/src/atlas_install.rs`):
//! Velopack builds the AppImage but adds nothing to the desktop, so without this Atlas PDF has no
//! menu entry and isn't offered for PDFs. Pure text; the app writes the file.

use crate::brand::{APP_ID, APP_NAME};

/// The `.desktop` file for an AppImage at `appimage`, or `None` for a path an entry can't hold
/// (not UTF-8, or with a control character such as a newline, which would end the line). `Exec`
/// takes URIs (`%U`), as file managers offer only such an app as the default for a type; the app
/// turns `file://` arguments back into paths (`path_from_arg` in `apps/pdfcraft/src/main.rs`).
pub fn for_appimage(appimage: &std::path::Path) -> Option<String> {
    let path = appimage.to_str()?;
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={APP_NAME}\n\
         GenericName=PDF Editor\n\
         Comment=Read, mark up and print construction drawings\n\
         Exec={} %U\n\
         TryExec={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         StartupNotify=true\n\
         StartupWMClass={APP_ID}\n\
         Categories=Office;Viewer;\n\
         MimeType=application/pdf;\n\
         Keywords=pdf;takeoff;drawings;viewer;\n",
        value(&quoted(path)),
        value(path),
    ))
}

/// An Exec argument in double quotes, with the four characters the spec reserves inside them
/// escaped, and `%` doubled so it isn't read as a field code.
fn quoted(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A string value as the file holds it: a backslash is written `\\` (newlines were refused). For
/// Exec this applies after [`quoted`], as readers undo it first.
fn value(s: &str) -> String {
    s.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn line<'a>(entry: &'a str, key: &str) -> &'a str {
        entry.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).unwrap_or_default()
    }

    #[test]
    fn the_entry_runs_the_appimage_with_the_files_given() {
        let entry = for_appimage(Path::new("/home/sam/Applications/AtlasSwitch.AtlasPDF.AppImage")).unwrap_or_default();
        assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
        assert_eq!(line(&entry, "Exec"), "\"/home/sam/Applications/AtlasSwitch.AtlasPDF.AppImage\" %U");
        assert_eq!(line(&entry, "TryExec"), "/home/sam/Applications/AtlasSwitch.AtlasPDF.AppImage");
        assert_eq!(line(&entry, "Icon"), APP_ID);
        assert_eq!(line(&entry, "StartupWMClass"), APP_ID);
        assert_eq!(line(&entry, "MimeType"), "application/pdf;");
        assert_eq!(line(&entry, "Name"), "Atlas PDF");
    }

    #[test]
    fn reserved_characters_in_the_path_are_escaped() {
        let entry = for_appimage(Path::new("/home/sam/my \"apps\"/$x `y` 100%/a\\b.AppImage")).unwrap_or_default();
        // The quoting escapes (\" \$ \` \\) are inside a string value, whose own escaping doubles
        // each backslash: \\" in the file, four for a literal backslash (Desktop Entry
        // Specification, "The Exec key"). And %% for %.
        assert_eq!(line(&entry, "Exec"), r#""/home/sam/my \\"apps\\"/\\$x \\`y\\` 100%%/a\\\\b.AppImage" %U"#);
        // TryExec isn't quoted and has no field codes: only the backslash changes.
        assert_eq!(line(&entry, "TryExec"), r#"/home/sam/my "apps"/$x `y` 100%/a\\b.AppImage"#);
    }

    #[test]
    fn a_path_that_would_break_the_file_gets_no_entry() {
        assert_eq!(for_appimage(Path::new("/tmp/a\nExec=evil")), None);
        assert_eq!(for_appimage(Path::new("/tmp/a\rb")), None);
        assert_eq!(for_appimage(Path::new("")), None);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            assert_eq!(for_appimage(Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff.AppImage"))), None);
        }
    }
}
