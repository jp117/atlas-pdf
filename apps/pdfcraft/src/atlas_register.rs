//! Atlas fork: offer Atlas PDF for PDFs, for the current user only (no administrator rights).
//!
//! Setup's install and update hooks call [`register`], its uninstall hook [`unregister`]
//! (`atlas_install`); `packaging/windows/install-atlas.ps1` runs the same hooks. This adds a
//! ProgID, OpenWithProgids, Capabilities and RegisteredApplications (Windows' Default Programs),
//! an App Paths entry and "Open with Atlas PDF" on every PDF. Windows doesn't let a program make
//! itself the default for a file type: the user chooses it in Settings ▸ Default apps.

use windows_registry::CURRENT_USER;

const NAME: &str = "Atlas PDF";
const PROG_ID: &str = "AtlasPDF.Document";
const CAPABILITIES: &str = r"Software\AtlasPDF\Capabilities";

/// The keys only Atlas PDF writes, removed whole by [`unregister`].
const OWN_KEYS: [&str; 5] = [
    r"Software\Classes\AtlasPDF.Document",
    r"Software\Classes\Applications\AtlasPDF.exe",
    r"Software\Classes\SystemFileAssociations\.pdf\shell\AtlasPDF.Open",
    r"Software\AtlasPDF",
    r"Software\Microsoft\Windows\CurrentVersion\App Paths\AtlasPDF.exe",
];

/// Values Atlas PDF adds to keys other apps share: removed one by one, never the key.
const SHARED_VALUES: [(&str, &str); 2] = [(r"Software\Classes\.pdf\OpenWithProgids", PROG_ID), (r"Software\RegisteredApplications", NAME)];

/// One string value under `HKEY_CURRENT_USER`: its key, its name (empty for the default value)
/// and its data.
type Value = (String, &'static str, String);

/// Every value [`register`] writes, for the app at `exe`.
fn values(exe: &str) -> Vec<Value> {
    let open = format!("\"{exe}\" \"%1\"");
    // The document icon is resource 2 of the executable (build.rs).
    let document_icon = format!("\"{exe}\",-2");
    let app_icon = format!("\"{exe}\",0");
    let classes = r"Software\Classes";
    let app = format!(r"{classes}\Applications\AtlasPDF.exe");
    let menu = format!(r"{classes}\SystemFileAssociations\.pdf\shell\AtlasPDF.Open");
    vec![
        (format!(r"{classes}\{PROG_ID}"), "", "PDF Document".into()),
        (format!(r"{classes}\{PROG_ID}\DefaultIcon"), "", document_icon.clone()),
        (format!(r"{classes}\{PROG_ID}\shell\open\command"), "", open.clone()),
        (SHARED_VALUES[0].0.into(), PROG_ID, String::new()),
        (app.clone(), "FriendlyAppName", NAME.into()),
        (format!(r"{app}\DefaultIcon"), "", document_icon),
        (format!(r"{app}\SupportedTypes"), ".pdf", String::new()),
        (format!(r"{app}\shell\open\command"), "", open.clone()),
        (CAPABILITIES.into(), "ApplicationName", NAME.into()),
        (CAPABILITIES.into(), "ApplicationDescription", "Atlas Switch's PDF workbench for construction drawings".into()),
        (CAPABILITIES.into(), "ApplicationIcon", app_icon.clone()),
        (format!(r"{CAPABILITIES}\FileAssociations"), ".pdf", PROG_ID.into()),
        (SHARED_VALUES[1].0.into(), NAME, CAPABILITIES.into()),
        (OWN_KEYS[4].into(), "", exe.into()),
        // On every PDF, whichever app is the default (Show more options on Windows 11).
        (menu.clone(), "", "Open with Atlas PDF".into()),
        (menu.clone(), "Icon", app_icon),
        (menu.clone(), "MultiSelectModel", "Document".into()),
        (format!(r"{menu}\command"), "", open),
    ]
}

/// Offer this executable for PDFs. Every value is rewritten, so an update that moved the app
/// points them at the new place.
pub fn register() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("the app's own path: {e}"))?;
    // A lossy path would name some other file in every command.
    let exe = exe.to_str().ok_or_else(|| format!("the app's path can't be written to the registry: {}", exe.display()))?;
    for (key, name, data) in values(exe) {
        // `create` opens a key that exists without touching other apps' values in it.
        CURRENT_USER.create(&key).and_then(|k| k.set_string(name, &data)).map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

/// Remove everything [`register`] wrote. The settings folder is the user's and stays.
pub fn unregister() {
    for key in OWN_KEYS {
        if let Err(e) = CURRENT_USER.remove_tree(key) {
            log::info!("uninstall: {key}: {e}");
        }
    }
    for (key, name) in SHARED_VALUES {
        if let Err(e) = CURRENT_USER.create(key).and_then(|k| k.remove_value(name)) {
            log::info!("uninstall: {key} {name}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfs_open_with_the_quoted_executable() {
        let exe = r"C:\Users\A B\AppData\Local\AtlasPDF\current\AtlasPDF.exe";
        let values = values(exe);
        let open = format!("\"{exe}\" \"%1\"");
        let commands: Vec<_> = values.iter().filter(|(k, _, _)| k.ends_with(r"\command")).collect();
        assert_eq!(commands.len(), 3);
        assert!(commands.iter().all(|(_, name, data)| name.is_empty() && *data == open), "{commands:?}");
        assert!(values.contains(&(r"Software\Classes\AtlasPDF.Document\DefaultIcon".into(), "", format!("\"{exe}\",-2"))));
    }

    #[test]
    fn every_value_is_removed_by_unregister() {
        // Each key either lies inside a key Atlas PDF owns, or is a shared key whose value is listed.
        for (key, name, _) in values("x.exe") {
            let owned = OWN_KEYS.iter().any(|own| key == *own || key.starts_with(&format!(r"{own}\")));
            let shared = SHARED_VALUES.contains(&(key.as_str(), name));
            assert!(owned ^ shared, "{key} {name:?}: owned {owned}, shared {shared}");
        }
    }

    #[test]
    fn default_programs_find_the_capabilities() {
        let values = values("x.exe");
        assert!(values.contains(&(r"Software\RegisteredApplications".into(), NAME, CAPABILITIES.into())));
        assert!(values.contains(&(format!(r"{CAPABILITIES}\FileAssociations"), ".pdf", PROG_ID.into())));
    }
}
