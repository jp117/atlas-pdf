//! Atlas fork: copies installed by Atlas PDF's Setup (Velopack, `packaging/windows/package-atlas.ps1`)
//! update themselves from the latest GitHub release.
//!
//! [`startup`] runs first in `main`: Setup and the updater start the app with hook arguments,
//! which are answered here before the process exits. An installed copy then checks for a newer
//! release, downloads it in the background (`pdfcraft_ui_egui::updates::Updater`), and
//! [`install_on_exit`] hands it to Velopack's updater as the app closes. Development builds, the
//! portable zip and `install-atlas.ps1` copies aren't installed by Setup: none of this applies there.

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
static FIRST_RUN: AtomicBool = AtomicBool::new(false);

/// Answer Setup's hooks (they exit), then look for the installation this copy belongs to.
pub fn startup() {
    VelopackApp::build()
        // A downloaded update installs when the app closes, never as it starts: a second launch
        // that only hands its files to the running window must not replace that window's files.
        .set_auto_apply_on_startup(false)
        .on_after_install_fast_callback(|_| registered(crate::atlas_register::register()))
        // An update may move or rename things: write every value again.
        .on_after_update_fast_callback(|_| registered(crate::atlas_register::register()))
        .on_before_uninstall_fast_callback(|_| crate::atlas_register::unregister())
        .on_first_run(|_| FIRST_RUN.store(true, Ordering::SeqCst))
        .run();
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

fn registered(result: Result<(), String>) {
    if let Err(e) = result {
        log::error!("couldn't offer Atlas PDF for PDFs: {e}");
    }
}

/// Let an installed copy's window update it. On the first start, open Settings ▸ Default apps at
/// Atlas PDF, since Windows leaves choosing the app for PDFs to the user.
pub fn attach(app: &mut PdfCraftApp) {
    if let Some(installed) = INSTALLED.get() {
        app.set_updater(Arc::clone(installed) as Arc<dyn Updater>);
    }
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
        let limit = asset.Size.min(MAX_PACKAGE);
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

    #[test]
    fn feed_urls_join_with_one_slash() {
        for base in ["https://example.com/feed", "https://example.com/feed/"] {
            assert_eq!(Feed { base: base.into() }.url("releases.win.json"), "https://example.com/feed/releases.win.json");
        }
    }
}
