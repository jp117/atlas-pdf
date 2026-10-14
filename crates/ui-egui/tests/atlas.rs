//! Atlas fork: the takeoff tools in the window (the floating tool strip, the keys).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_atlas::takeoff::Status;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("drawing.pdf", None, PDF.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn the_tool_strip_has_a_button_per_takeoff_status() {
    for status in Status::ALL {
        let mut h = harness();
        h.get_by_label(status.label()).click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Takeoff(status), "{status:?}");
    }
}
