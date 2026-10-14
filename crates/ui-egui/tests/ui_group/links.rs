//! Project links (Atlas fork): the ArtCraft brand and community links are gone (the brand is not
//! licensed to forks); the Help menu and the About dialog credit PdfCraft, the project the app is
//! built on.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::links;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn no_artcraft_brand_or_discord_on_the_home_screen() {
    let h = harness(|_| {});
    for label in ["Discord", "Join our Discord", "Join the ArtCraft community", "ArtCraft"] {
        assert!(h.query_by_label(label).is_none(), "{label} is still shown");
    }
}

#[test]
fn about_dialog_credits_pdfcraft_without_the_brand() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let mut h = harness(move |app| {
        app.open_bytes("one.pdf", None, pdf.to_vec()).unwrap();
        app.dialog = Some(Dialog::About);
    });
    assert_eq!(h.query_all_by_label("ArtCraft").count(), 0, "no mark or wordmark");
    h.get_by_label("PdfCraft on GitHub").click();
    h.run_steps(2);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(links::GITHUB));
}

#[test]
fn help_commands_open_each_link() {
    for l in links::LINKS {
        let mut h = harness(|_| {});
        assert!(h.state_mut().execute(l.command), "{}", l.command);
        assert_eq!(h.state().last_opened_url.as_deref(), Some(l.url));
        assert_eq!(pdfcraft_engine::commands::command(l.command).unwrap().menu, Some("Help"));
    }
    for gone in ["help.discord", "help.app_page", "help.website"] {
        assert!(pdfcraft_engine::commands::command(gone).is_none(), "{gone} is still registered");
    }
}
