//! Atlas fork: copies installed by Atlas PDF's Setup on Windows, and the Linux AppImage (both
//! Velopack: `packaging/windows/package-atlas.ps1`, `packaging/linux/package-atlas.sh`), update
//! themselves from the latest GitHub release.
//!
//! [`startup`] runs first in `main`: on Windows, Setup and the updater start the app with hook
//! arguments, which are answered here before the process exits. An installed copy then checks for
//! a newer release, downloads it in the background (`pdfcraft_ui_egui::updates::Updater`), and
//! [`install_on_exit`] hands it to Velopack's updater as the app closes, which replaces the
//! installed files (on Linux, the AppImage itself). Development builds, the portable zip and
//! `install-atlas.ps1` copies aren't installed by Velopack: none of this applies there. On Linux
//! the AppImage also writes its own launcher entry, which Velopack doesn't.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::updates::Updater;
use velopack::sources::UpdateSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions, VelopackApp, VelopackAsset, VelopackAssetFeed};

/// Where releases are published: each release's `releases.win.json` and full package. GitHub
/// redirects `latest/download` to the newest published release, with no API rate limit.
const FEED: &str = "https://github.com/jp117/atlas-pdf/releases/latest/download";

/// Replaces [`FEED`], for trying an update against a local folder served over HTTP.
const FEED_VARIABLE: &str = "ATLASPDF_UPDATE_URL";

/// The largest package downloaded, whatever the feed says.
const MAX_PACKAGE: u64 = 2 << 30;

/// The installed copy, once [`startup`] has found one.
static INSTALLED: OnceLock<Arc<Installed>> = OnceLock::new();

/// The first start after Setup installed the app.
#[cfg(target_os = "windows")]
static FIRST_RUN: AtomicBool = AtomicBool::new(false);

/// Answer Setup's hooks (they exit), then look for the installation this copy belongs to.
pub fn startup() {
    // A downloaded update installs when the app closes, never as it starts: a second launch that
    // only hands its files to the running window must not replace that window's files.
    setup_hooks(VelopackApp::build().set_auto_apply_on_startup(false)).run();
    #[cfg(target_os = "linux")]
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        // Off the main thread: it reads and may write files, and the window shouldn't wait.
        let spawned = std::thread::Builder::new().name("launcher-entry".into()).spawn(move || launcher_entry(Path::new(&appimage)));
        if let Err(e) = spawned {
            log::warn!("couldn't add Atlas PDF to the applications menu: {e}");
        }
    }
    let source = Feed { base: std::env::var(FEED_VARIABLE).unwrap_or_else(|_| FEED.to_string()) };
    // Full packages only: deltas would have to come from older releases than `latest`.
    let options = UpdateOptions { MaximumDeltasBeforeFallback: -1, ..Default::default() };
    match UpdateManager::new(source, Some(options), None) {
        Ok(manager) => {
            log::info!("installed by Setup: version {}", manager.get_current_version_as_string());
            let _ = INSTALLED.set(Arc::new(Installed { manager, found: Mutex::new(None), reopen: AtomicBool::new(false) }));
        }
        // A development build, the portable zip or an `install-atlas.ps1` copy.
        Err(e) => log::info!("not installed by Setup, so no automatic updates: {e}"),
    }
}

/// Setup's install, update and uninstall hooks, which register Atlas PDF for PDFs. Windows only: the
/// AppImage has no installer, and writes its menu entry as it starts instead.
fn setup_hooks(app: VelopackApp<'_>) -> VelopackApp<'_> {
    #[cfg(target_os = "windows")]
    let app = app
        .on_after_install_fast_callback(|_| registered(crate::atlas_register::register()))
        // An update may move or rename things: write every value again.
        .on_after_update_fast_callback(|_| registered(crate::atlas_register::register()))
        .on_before_uninstall_fast_callback(|_| crate::atlas_register::unregister())
        .on_first_run(|_| FIRST_RUN.store(true, Ordering::SeqCst));
    app
}

#[cfg(target_os = "windows")]
fn registered(result: Result<(), String>) {
    if let Err(e) = result {
        log::error!("couldn't offer Atlas PDF for PDFs: {e}");
    }
}

/// The AppImage's launcher entry and icon in the user's data folder, written again whenever they
/// differ (the AppImage moved, or an update changed them), so the applications menu lists Atlas PDF
/// and "Open with" offers it for PDFs. Deleting the AppImage hides the entry (its `TryExec`).
#[cfg(target_os = "linux")]
fn launcher_entry(appimage: &Path) {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(".local/share")));
    let Some(data) = data else { return };
    match write_launcher_entry(appimage, &data) {
        // Refresh the cache "Open with" reads; desktops without the tool rescan by themselves.
        Ok(true) => {
            if let Err(e) = std::process::Command::new("update-desktop-database").arg(data.join("applications")).status() {
                log::info!("update-desktop-database: {e}");
            }
        }
        Ok(false) => {}
        Err(e) => log::warn!("couldn't add Atlas PDF to the applications menu: {e}"),
    }
}

/// [`launcher_entry`] under the data folder `data`. Whether the entry changed.
#[cfg(target_os = "linux")]
fn write_launcher_entry(appimage: &Path, data: &Path) -> std::io::Result<bool> {
    use pdfcraft_atlas::brand::APP_ID;
    let entry = pdfcraft_atlas::desktop_entry::for_appimage(appimage)
        .ok_or_else(|| std::io::Error::other(format!("the AppImage's path {} can't be written in one", appimage.display())))?;
    let icon = data.join("icons/hicolor/256x256/apps").join(format!("{APP_ID}.png"));
    if let Err(e) = write_if_changed(&icon, crate::APP_ICON_PNG) {
        log::warn!("couldn't install the menu icon {}: {e}", icon.display());
    }
    write_if_changed(&data.join("applications").join(format!("{APP_ID}.desktop")), entry.as_bytes())
}

/// Replace `path` with `bytes` unless it holds them already, through a temporary file beside it so
/// a desktop scanning the folder never reads half a file. Whether it wrote.
#[cfg(target_os = "linux")]
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let temporary = dir.join(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    std::fs::write(&temporary, bytes)?;
    if let Err(e) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    Ok(true)
}

/// Let an installed copy's window update it. On Windows, on the first start, open Settings ▸
/// Default apps at Atlas PDF, since Windows leaves choosing the app for PDFs to the user.
pub fn attach(app: &mut PdfCraftApp) {
    if let Some(installed) = INSTALLED.get() {
        app.set_updater(Arc::clone(installed) as Arc<dyn Updater>);
    }
    #[cfg(target_os = "windows")]
    if FIRST_RUN.load(Ordering::SeqCst) {
        match std::process::Command::new("explorer.exe").arg("ms-settings:defaultapps?registeredAppUser=Atlas%20PDF").spawn() {
            Ok(_) => {
                app.notify("Atlas PDF is installed. To open PDFs in it by double-click, set .pdf to Atlas PDF in the Settings page that opened.")
            }
            Err(e) => log::warn!("couldn't open Default apps: {e}"),
        }
    }
}

/// As the app closes: hand a downloaded update to Velopack's updater, which waits for this process
/// to exit, installs it, and reopens the app if the user chose Restart now.
pub fn install_on_exit() {
    let Some(installed) = INSTALLED.get() else { return };
    let Some(pending) = installed.manager.get_update_pending_restart() else { return };
    let reopen = installed.reopen.load(Ordering::SeqCst);
    log::info!("installing {} as the app closes (reopen: {reopen})", pending.Version);
    if let Err(e) = installed.manager.wait_exit_then_apply_updates(&pending, true, reopen, Vec::<String>::new()) {
        log::error!("couldn't start the update to {}: {e}", pending.Version);
    }
}

struct Installed {
    manager: UpdateManager,
    /// What the last check found, for the download.
    found: Mutex<Option<UpdateInfo>>,
    reopen: AtomicBool,
}

impl Updater for Installed {
    fn check(&self) -> Result<Option<String>, String> {
        let found = match self.manager.check_for_updates().map_err(|e| e.to_string())? {
            UpdateCheck::UpdateAvailable(info) => Some(*info),
            UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => None,
        };
        let version = found.as_ref().map(|info| info.TargetFullRelease.Version.clone());
        *self.found.lock().map_err(|_| "the update state was lost; restart Atlas PDF")? = found;
        Ok(version)
    }

    fn download(&self) -> Result<(), String> {
        let found = self.found.lock().map_err(|_| "the update state was lost; restart Atlas PDF")?.clone();
        let info = found.ok_or("no newer version was found to download")?;
        self.manager.download_updates(&info, None).map_err(|e| e.to_string())
    }

    fn reopen_after_install(&self, reopen: bool) {
        self.reopen.store(reopen, Ordering::SeqCst);
    }
}

/// The release feed, fetched with the app's own HTTPS client (the operating system's certificate
/// authorities, so a company proxy that inspects TLS still works). Velopack checks each
/// package's size and SHA-256 against the feed after the download.
struct Feed {
    base: String,
}

impl Feed {
    fn url(&self, file: &str) -> String {
        format!("{}/{file}", self.base.trim_end_matches('/'))
    }
}

impl UpdateSource for Feed {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &velopack::bundle::Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, velopack::Error> {
        let name = format!("releases.{channel}.json");
        if !plain_file_name(&name) {
            return Err(velopack::Error::Other(format!("unexpected update channel {channel:?}")));
        }
        let mut response = get(&self.url(&name), Duration::from_secs(20))?;
        let body = response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(network)?;
        parse_feed(&body)
    }

    fn download_release_entry(&self, asset: &VelopackAsset, local_file: &Path, _progress: Option<Sender<i16>>) -> Result<(), velopack::Error> {
        // `parse_feed` already dropped other names; checked again because the asset is Velopack's.
        if !plain_file_name(&asset.FileName) {
            return Err(velopack::Error::Other(format!("unexpected package name {:?}", asset.FileName)));
        }
        // ureq refuses a body that reaches its limit, so allow one byte more than the feed's size:
        // a longer body still fails here, and Velopack checks the size and SHA-256 afterwards.
        let limit = asset.Size.min(MAX_PACKAGE).saturating_add(1);
        let mut response = get(&self.url(&asset.FileName), Duration::from_secs(60 * 60))?;
        let mut body = response.body_mut().with_config().limit(limit).reader();
        let mut file = std::fs::File::create(local_file)?;
        std::io::copy(&mut body, &mut file)?;
        file.flush()?;
        Ok(())
    }
}

fn get(url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, velopack::Error> {
    let agent = crate::updates::agent(timeout).map_err(velopack::Error::Other)?;
    agent.get(url).header("User-Agent", crate::updates::USER_AGENT).call().map_err(network)
}

fn network(e: ureq::Error) -> velopack::Error {
    match e {
        ureq::Error::StatusCode(404) => velopack::Error::Other("no Atlas PDF release is published yet".into()),
        e => velopack::Error::Other(format!("couldn't reach the update server ({e})")),
    }
}

/// The feed, keeping only packages with plain file names: a name becomes both a URL under the feed
/// and a file in Velopack's packages folder.
fn parse_feed(body: &str) -> Result<VelopackAssetFeed, velopack::Error> {
    let mut feed: VelopackAssetFeed = serde_json::from_str(body)?;
    feed.Assets.retain(|a| plain_file_name(&a.FileName));
    Ok(feed)
}

/// Letters, digits, `.`, `-` and `_`, not starting with a dot: no separators, no `..`, no URL syntax.
fn plain_file_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_package_names_are_fetched() {
        for ok in ["AtlasPDF-0.7.1-full.nupkg", "releases.win.json", "a_b-c.1"] {
            assert!(plain_file_name(ok), "{ok}");
        }
        for bad in ["", "..", "../x.nupkg", "..\\x", "a/b", "a\\b", "x?y=1", "x#y", "%2e%2e", ".hidden", "C:x", "a b", "é.nupkg"] {
            assert!(!plain_file_name(bad), "{bad}");
        }
        assert!(!plain_file_name(&"a".repeat(201)));
    }

    #[test]
    fn the_feed_drops_packages_with_other_names() {
        let feed = parse_feed(
            r#"{"Assets":[
                {"PackageId":"AtlasPDF","Version":"0.8.0","Type":"Full","FileName":"AtlasPDF-0.8.0-full.nupkg","SHA1":"x","SHA256":"y","Size":10},
                {"PackageId":"AtlasPDF","Version":"9.9.9","Type":"Full","FileName":"../../evil.nupkg","SHA1":"x","SHA256":"y","Size":10}
            ]}"#,
        )
        .unwrap();
        let names: Vec<_> = feed.Assets.iter().map(|a| a.FileName.as_str()).collect();
        assert_eq!(names, ["AtlasPDF-0.8.0-full.nupkg"]);
        assert!(parse_feed("<html>").is_err());
    }

    /// Serve one HTTP response holding `body` on a local port, and return that address.
    fn serve_once(body: &'static [u8]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut request);
                let head = format!(
                    "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn package(size: u64) -> VelopackAsset {
        VelopackAsset { FileName: "AtlasSwitch.AtlasPDF-0.7.2-full.nupkg".into(), Size: size, ..Default::default() }
    }

    #[test]
    fn a_package_of_exactly_its_feed_size_downloads() {
        let file = std::env::temp_dir().join(format!("atlas-feed-test-{}.partial", std::process::id()));
        Feed { base: serve_once(b"hello") }.download_release_entry(&package(5), &file, None).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"hello");
        // A server sending more than the feed promised is cut off.
        assert!(Feed { base: serve_once(b"hello, world") }.download_release_entry(&package(5), &file, None).is_err());
        let _ = std::fs::remove_file(&file);
    }

    /// Velopack installs into `%LOCALAPPDATA%\<packId>`, and uninstalling deletes that folder: it
    /// must not be the app's own folder there (crash recovery, the clipboard cache, single instance).
    #[test]
    fn setup_installs_beside_the_data_folder_not_into_it() {
        let script = include_str!("../../../packaging/windows/package-atlas.ps1");
        let id = script.split("--packId ").nth(1).and_then(|rest| rest.split_whitespace().next()).unwrap();
        assert!(!id.is_empty() && !id.eq_ignore_ascii_case(pdfcraft_atlas::brand::DATA_NAME), "{id}");
    }

    #[test]
    fn feed_urls_join_with_one_slash() {
        for base in ["https://example.com/feed", "https://example.com/feed/"] {
            assert_eq!(Feed { base: base.into() }.url("releases.win.json"), "https://example.com/feed/releases.win.json");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_appimage_writes_its_menu_entry_once_and_again_when_moved() {
        let data = std::env::temp_dir().join(format!("atlas-launcher-entry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let entry = data.join("applications/AtlasSwitch.AtlasPDF.desktop");
        let icon = data.join("icons/hicolor/256x256/apps/AtlasSwitch.AtlasPDF.png");

        assert!(write_launcher_entry(Path::new("/opt/a/AtlasSwitch.AtlasPDF.AppImage"), &data).unwrap());
        assert!(std::fs::read_to_string(&entry).unwrap().contains("Exec=\"/opt/a/AtlasSwitch.AtlasPDF.AppImage\" %U\n"));
        assert_eq!(std::fs::read(&icon).unwrap(), crate::APP_ICON_PNG);
        // Nothing changed: nothing is written, so the desktop isn't asked to rescan.
        assert!(!write_launcher_entry(Path::new("/opt/a/AtlasSwitch.AtlasPDF.AppImage"), &data).unwrap());
        // Moved: the entry follows it.
        assert!(write_launcher_entry(Path::new("/opt/b/AtlasSwitch.AtlasPDF.AppImage"), &data).unwrap());
        assert!(std::fs::read_to_string(&entry).unwrap().contains("/opt/b/"));
        // A path no entry can hold leaves the last one alone.
        assert!(write_launcher_entry(Path::new("/opt/c\nExec=x"), &data).is_err());
        assert!(std::fs::read_to_string(&entry).unwrap().contains("/opt/b/"));
        let leftovers: Vec<_> = std::fs::read_dir(data.join("applications")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temporary files left: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&data);
    }
}
