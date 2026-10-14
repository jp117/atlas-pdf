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

#[test]
fn keys_re_mark_the_selected_box_in_box_mode_and_delete_removes_it() {
    use pdfcraft_atlas::takeoff::NewMark;
    use pdfcraft_engine::Edit;
    let mut h = harness();
    let id = h.state().views[0].id;
    let mark = NewMark {
        page: 0,
        rect: [50.0, 50.0, 150.0, 120.0],
        status: Status::Included,
        label: "LP-1".into(),
        comment: "spare".into(),
        author: "T".into(),
    };
    h.state_mut().session.apply(id, Edit::AddTakeoffMark(mark)).unwrap();
    let marks = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().takeoff_marks().unwrap().marks;
    let index = marks(&h)[0].index;

    // A green box tool is active and the box is selected: 3 makes it red, label and comment kept.
    h.state_mut().quick_tool = QuickTool::Takeoff(Status::Included);
    h.state_mut().views[0].comments.selected = Some((0, index));
    h.key_press(egui::Key::Num3);
    h.run_steps(3);
    let m = &marks(&h)[0];
    assert_eq!((m.status, m.label.as_str(), m.comment.as_str()), (Status::Excluded, "LP-1", "spare"));
    assert_eq!(h.state().quick_tool, QuickTool::Takeoff(Status::Included), "the tool stays");

    // With nothing selected, a key picks the tool instead.
    h.state_mut().views[0].comments.selected = None;
    h.key_press(egui::Key::Num2);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Takeoff(Status::IncludedWithComments));
    assert_eq!(marks(&h)[0].status, Status::Excluded);

    // Delete removes the selected box.
    h.state_mut().views[0].comments.selected = Some((0, index));
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(marks(&h).is_empty());
}

/// Comments belong to the quote, not to boxes: a yellow box goes on in one drag, like the others.
#[test]
fn a_yellow_box_is_added_in_one_drag_without_a_comment_prompt() {
    use egui::{Event, PointerButton, pos2};
    let mut h = harness();
    h.state_mut().quick_tool = QuickTool::Takeoff(Status::IncludedWithComments);
    h.run_steps(2);
    let id = h.state().views[0].id;
    let (from, to) = (pos2(640.0, 420.0), pos2(700.0, 470.0));
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
    h.event(Event::PointerMoved(from));
    h.run_steps(1);
    h.event(button(from, true));
    h.run_steps(1);
    for k in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.run_steps(1);
    }
    h.event(button(to, false));
    h.run_steps(4);
    let marks = h.state().session.get(id).unwrap().takeoff_marks().unwrap().marks;
    assert_eq!(marks.len(), 1, "the box is added straight away");
    assert_eq!((marks[0].status, marks[0].comment.as_str()), (Status::IncludedWithComments, ""));
    assert!(h.query_by_label("Comment").is_none(), "no comment prompt");
}
