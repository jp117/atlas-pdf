//! Installed copies update themselves: a check at start, a download in the background, and the
//! install when the app closes (or at once, with Restart now).
//!
//! The desktop app supplies the [`Updater`] (Velopack, for copies its Setup installed), so this
//! crate has no network or install code. Without one (a development build, the portable zip, the
//! web build) Help ▸ Check for updates keeps PdfCraft's behaviour: ask, then offer the page.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::{Align, Layout};

use crate::{PdfCraftApp, theme, updates::RELEASES_PAGE, widgets};

/// Finds, downloads and installs a newer release. Every method that blocks runs on its own thread.
pub trait Updater: Send + Sync {
    /// The newer release's version, or `None` when this copy is up to date. Blocking.
    fn check(&self) -> Result<Option<String>, String>;
    /// Download the release the last [`Self::check`] found, so it installs when the app closes.
    /// Blocking.
    fn download(&self) -> Result<(), String>;
    /// Whether the app opens again once the update has installed.
    fn reopen_after_install(&self, reopen: bool);
}

/// Where an update is.
#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    Checking(Receiver<Result<Option<String>, String>>),
    UpToDate,
    /// Downloading this version.
    Downloading(String, Receiver<Result<(), String>>),
    /// This version installs when the app closes.
    Ready(String),
    Failed(String),
}

#[derive(Default)]
pub(crate) struct UpdateState {
    updater: Option<Arc<dyn Updater>>,
    stage: Stage,
    /// The check at start has run.
    started: bool,
    /// The user asked (Help ▸ Check for updates): the dialog is showing.
    open: bool,
}

impl PdfCraftApp {
    /// Update this copy through `updater`: check at start, download in the background, install
    /// at close. The desktop app sets it for installed copies only.
    pub fn set_updater(&mut self, updater: Arc<dyn Updater>) {
        self.atlas.updates.updater = Some(updater);
    }
}

/// Help ▸ Check for updates. False when there is no updater, so the caller asks the upstream way.
pub(crate) fn check(app: &mut PdfCraftApp) -> bool {
    if app.atlas.updates.updater.is_none() {
        return false;
    }
    app.atlas.updates.open = true;
    app.atlas.updates.started = true;
    if matches!(app.atlas.updates.stage, Stage::Idle | Stage::UpToDate | Stage::Failed(_)) {
        start_check(app);
    }
    true
}

/// Each frame: the check at start, then pick up a finished check or download.
pub(crate) fn poll(app: &mut PdfCraftApp) {
    if app.atlas.updates.updater.is_none() {
        return;
    }
    if !app.atlas.updates.started {
        app.atlas.updates.started = true;
        start_check(app);
    }
    let checked = match &app.atlas.updates.stage {
        Stage::Checking(rx) => received(rx),
        _ => None,
    };
    match checked {
        None => {}
        Some(Ok(Some(version))) => start_download(app, version),
        Some(Ok(None)) => app.atlas.updates.stage = Stage::UpToDate,
        Some(Err(e)) => fail(app, e),
    }
    let downloaded = match &app.atlas.updates.stage {
        Stage::Downloading(version, rx) => received(rx).map(|r| r.map(|()| version.clone())),
        _ => None,
    };
    match downloaded {
        None => {}
        Some(Ok(version)) => {
            // Said once, quietly: the dialog says it too when it is open.
            if !app.atlas.updates.open {
                app.notify(format!("Atlas PDF {} is ready. It installs when you close Atlas PDF.", bare(&version)));
            }
            app.atlas.updates.stage = Stage::Ready(version);
        }
        Some(Err(e)) => fail(app, e),
    }
}

/// A finished job's answer; `None` while it runs.
fn received<T>(rx: &Receiver<Result<T, String>>) -> Option<Result<T, String>> {
    match rx.try_recv() {
        Ok(r) => Some(r),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err("the update stopped unexpectedly".into())),
    }
}

/// The check at start fails quietly (offline, no release yet): only the log and the dialog say so.
fn fail(app: &mut PdfCraftApp, error: String) {
    log::warn!("update: {error}");
    app.atlas.updates.stage = Stage::Failed(error);
}

/// Run `job` with the updater on its own thread; its answer arrives on the receiver.
fn spawn<T: Send + 'static>(
    app: &PdfCraftApp,
    job: impl FnOnce(&dyn Updater) -> Result<T, String> + Send + 'static,
) -> Option<Receiver<Result<T, String>>> {
    let updater = app.atlas.updates.updater.clone()?;
    let ctx = app.ctx.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // The receiver may be gone (the app quit): nothing to report to then.
        let _ = tx.send(job(updater.as_ref()));
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
    Some(rx)
}

fn start_check(app: &mut PdfCraftApp) {
    if let Some(rx) = spawn(app, |u| u.check()) {
        app.atlas.updates.stage = Stage::Checking(rx);
    }
}

fn start_download(app: &mut PdfCraftApp, version: String) {
    if let Some(rx) = spawn(app, |u| u.download()) {
        app.atlas.updates.stage = Stage::Downloading(version, rx);
    }
}

/// `v0.8.0` as `0.8.0`.
fn bare(version: &str) -> &str {
    version.trim_start_matches(['v', 'V'])
}

/// The Updates dialog, for copies with an updater.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.atlas.updates.open {
        return;
    }
    let t = theme::Tokens::get(ctx);
    let current = env!("CARGO_PKG_VERSION");
    let mut close = false;
    let mut restart = false;
    let modal = widgets::show_modal(ctx, "atlas-updates", |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("cloud", 22.0, t.accent));
            ui.label(egui::RichText::new("Check for updates").font(theme::semibold(16.0)));
        });
        ui.add_space(8.0);
        let ready = match &app.atlas.updates.stage {
            Stage::Idle | Stage::Checking(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking for a newer version…");
                });
                false
            }
            Stage::Downloading(v, _) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Downloading Atlas PDF {}…", bare(v)));
                });
                false
            }
            Stage::Ready(v) => {
                ui.label(egui::RichText::new(format!("Atlas PDF {} is ready.", bare(v))).strong());
                ui.label(
                    egui::RichText::new(format!(
                        "You have version {current}. The new version installs when you close Atlas PDF, or restart now to install it."
                    ))
                    .color(t.text_muted),
                );
                true
            }
            Stage::UpToDate => {
                ui.label(format!("Atlas PDF {current} is up to date."));
                false
            }
            Stage::Failed(e) => {
                ui.label(format!("Couldn't update: {e}"));
                ui.label(egui::RichText::new(format!("You have version {current}. All releases are listed at {RELEASES_PAGE}.")).color(t.text_muted));
                false
            }
        };
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ready {
                restart = widgets::pill_button(ui, "Restart now", true).clicked();
                close = widgets::pill_button(ui, "Later", false).clicked() || restart;
            } else if widgets::pill_button(ui, "Close", true).clicked() {
                close = true;
            }
        });
    });
    if modal.should_close() {
        close = true;
        restart = false;
    }
    if close {
        app.atlas.updates.open = false;
    }
    if restart {
        if let Some(updater) = &app.atlas.updates.updater {
            updater.reopen_after_install(true);
        }
        // The usual quit: unsaved documents are asked about first.
        app.request_quit(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    struct Fake {
        check: Result<Option<String>, String>,
        checks: AtomicUsize,
        downloads: AtomicUsize,
        reopen: AtomicBool,
        download: Mutex<Result<(), String>>,
    }

    impl Fake {
        fn new(check: Result<Option<String>, String>) -> Arc<Self> {
            Arc::new(Fake {
                check,
                checks: AtomicUsize::new(0),
                downloads: AtomicUsize::new(0),
                reopen: AtomicBool::new(false),
                download: Mutex::new(Ok(())),
            })
        }
    }

    impl Updater for Fake {
        fn check(&self) -> Result<Option<String>, String> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            self.check.clone()
        }
        fn download(&self) -> Result<(), String> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            self.download.lock().map_err(|e| e.to_string())?.clone()
        }
        fn reopen_after_install(&self, reopen: bool) {
            self.reopen.store(reopen, Ordering::SeqCst);
        }
    }

    /// Poll until the background jobs have settled.
    fn settle(app: &mut PdfCraftApp) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // Through the upstream hook, as each frame does.
            app.poll_updates();
            if !matches!(app.atlas.updates.stage, Stage::Checking(_) | Stage::Downloading(..)) || Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_newer_release_downloads_at_start_and_says_so_once() {
        let fake = Fake::new(Ok(Some("v99.0.0".into())));
        let mut app = PdfCraftApp::default();
        app.set_updater(fake.clone());
        settle(&mut app);
        assert!(matches!(&app.atlas.updates.stage, Stage::Ready(v) if v == "v99.0.0"));
        assert_eq!((fake.checks.load(Ordering::SeqCst), fake.downloads.load(Ordering::SeqCst)), (1, 1));
        assert_eq!(app.toast.as_ref().map(|(m, _)| m.as_str()), Some("Atlas PDF 99.0.0 is ready. It installs when you close Atlas PDF."));
        // Later frames neither check nor download again.
        app.toast = None;
        settle(&mut app);
        assert_eq!((fake.checks.load(Ordering::SeqCst), fake.downloads.load(Ordering::SeqCst)), (1, 1));
        assert!(app.toast.is_none());
    }

    #[test]
    fn up_to_date_and_failed_checks_at_start_stay_quiet() {
        for answer in [Ok(None), Err("offline".to_string())] {
            let fake = Fake::new(answer);
            let mut app = PdfCraftApp::default();
            app.set_updater(fake.clone());
            settle(&mut app);
            assert!(matches!(app.atlas.updates.stage, Stage::UpToDate | Stage::Failed(_)));
            assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);
            assert!(app.toast.is_none() && !app.atlas.updates.open, "nothing interrupts the user");
        }
    }

    #[test]
    fn a_failed_download_is_reported_in_the_dialog_not_as_ready() {
        let fake = Fake::new(Ok(Some("v99.0.0".into())));
        *fake.download.lock().unwrap() = Err("disk full".into());
        let mut app = PdfCraftApp::default();
        app.set_updater(fake.clone());
        settle(&mut app);
        assert!(matches!(&app.atlas.updates.stage, Stage::Failed(e) if e == "disk full"));
        assert!(app.toast.is_none());
        // Asking again retries.
        *fake.download.lock().unwrap() = Ok(());
        assert!(check(&mut app));
        settle(&mut app);
        assert!(matches!(app.atlas.updates.stage, Stage::Ready(_)));
        assert_eq!(fake.checks.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn help_check_for_updates_opens_the_dialog_only_with_an_updater() {
        let mut app = PdfCraftApp::default();
        assert!(!check(&mut app), "no updater: the upstream check runs instead");
        assert!(!app.atlas.updates.open);
        poll(&mut app);
        assert!(matches!(app.atlas.updates.stage, Stage::Idle));

        let fake = Fake::new(Ok(None));
        app.set_updater(fake.clone());
        // Through the upstream hook, as the menu command does.
        app.check_for_updates();
        assert!(app.atlas.updates.open);
        settle(&mut app);
        assert!(matches!(app.atlas.updates.stage, Stage::UpToDate));
        // The user's check counts as the one at start.
        assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_download_finished_with_the_dialog_open_does_not_also_notify() {
        let fake = Fake::new(Ok(Some("v99.0.0".into())));
        let mut app = PdfCraftApp::default();
        app.set_updater(fake);
        assert!(check(&mut app));
        settle(&mut app);
        assert!(matches!(app.atlas.updates.stage, Stage::Ready(_)));
        assert!(app.toast.is_none());
    }
}
