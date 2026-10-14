//! The Layers panel for construction drawings: layers grouped by trade (from their NCS names) or
//! by the drawing they came from, same-named layers merged into one row, a search box, "Only"
//! to isolate a group, and "Electrical focus" to drop the architectural, structural, mechanical
//! and plumbing backgrounds in one click. Sheet containers from combined files ("Page 1", …)
//! stay on through every preset.

use crate::theme::Tokens;
use egui::{Rect, RichText, Sense, vec2};
use pdfcraft_atlas::layers::{LayerAction, LayerGroup, LayerId, LayerInput, LayerModel, Visibility};
use pdfcraft_engine::DocId;
use pdfcraft_render::{DocInfo, LayerOp};

/// The panel's search text and grouping, kept per document in egui's memory.
#[derive(Clone, Default)]
struct ViewState {
    query: String,
    by_source: bool,
}

/// Draw the panel; layer visibility changes the user asked for are pushed to `changes`.
pub(crate) fn show(ui: &mut egui::Ui, t: &Tokens, id: DocId, info: &DocInfo, changes: &mut Vec<(LayerId, bool)>) {
    if info.layers.is_empty() {
        ui.label(RichText::new("This document has no layers.").color(t.text_faint));
        return;
    }
    let input: Vec<LayerInput<'_>> = info.layers.iter().map(|l| LayerInput { id: l.id, name: &l.name, visible: l.visible }).collect();
    let model = LayerModel::new(&input);
    let memory = egui::Id::new(("atlas-layers", id.0));
    let mut state: ViewState = ui.data(|d| d.get_temp(memory)).unwrap_or_default();
    let mut action: Option<Owned> = None;

    ui.add(egui::TextEdit::singleline(&mut state.query).hint_text("Search layers").desired_width(f32::INFINITY));
    ui.horizontal_wrapped(|ui| {
        if ui.button("Electrical focus").on_hover_text("Hide architectural, structural, civil, mechanical and plumbing layers").clicked() {
            action = Some(Owned::ElectricalFocus);
        }
        if ui.button("Show all").clicked() {
            action = Some(Owned::ShowAll);
        }
    });
    ui.horizontal(|ui| {
        crate::widgets::radio_value(ui, &mut state.by_source, false, "By trade");
        crate::widgets::radio_value(ui, &mut state.by_source, true, "By drawing");
    });
    let merged = model.layers.len();
    if merged < info.layers.len() {
        ui.label(RichText::new(format!("{} layers, {merged} after merging same-named ones", info.layers.len())).small().color(t.text_faint));
    }
    ui.separator();

    if state.query.trim().is_empty() {
        let groups = if state.by_source { model.by_source() } else { model.by_discipline() };
        for (n, group) in groups.iter().enumerate() {
            // The trade Atlas quotes from starts open; the rest start closed.
            let open = n == 0 && !state.by_source;
            group_ui(ui, t, &model, group, (memory, state.by_source, n), open, &mut action);
        }
    } else {
        let hits = model.search(&state.query);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} matching", hits.len())).color(t.text_muted));
            if ui.add_enabled(!hits.is_empty(), egui::Button::new("Only these")).clicked() {
                action = Some(Owned::Isolate(hits.clone()));
            }
            if ui.add_enabled(!hits.is_empty(), egui::Button::new("Hide these")).clicked() {
                action = Some(Owned::Set(hits.clone(), false));
            }
        });
        for &i in &hits {
            row(ui, t, &model, i, true, &mut action);
        }
    }

    ui.data_mut(|d| d.insert_temp(memory, state));
    if let Some(action) = action {
        changes.extend(model.changes(action.borrowed()));
    }
}

/// [`LayerAction`] with owned layer lists, so it can outlive the frame's borrows.
enum Owned {
    Set(Vec<usize>, bool),
    Isolate(Vec<usize>),
    ElectricalFocus,
    ShowAll,
}

impl Owned {
    fn borrowed(&self) -> LayerAction<'_> {
        match self {
            Owned::Set(layers, visible) => LayerAction::Set { layers, visible: *visible },
            Owned::Isolate(layers) => LayerAction::Isolate { layers },
            Owned::ElectricalFocus => LayerAction::ElectricalFocus,
            Owned::ShowAll => LayerAction::ShowAll,
        }
    }
}

fn group_ui(
    ui: &mut egui::Ui,
    t: &Tokens,
    model: &LayerModel,
    group: &LayerGroup,
    key: (egui::Id, bool, usize),
    default_open: bool,
    action: &mut Option<Owned>,
) {
    let id = egui::Id::new(("atlas-layer-group", key.0, key.1, &group.title, key.2));
    let visibility = model.visibility_of(&group.layers);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
        .show_header(ui, |ui| {
            if eye(ui, t, visibility).on_hover_text("Show or hide the whole group").clicked() {
                *action = Some(Owned::Set(group.layers.clone(), visibility != Visibility::On));
            }
            ui.label(RichText::new(&group.title).strong());
            ui.label(RichText::new(group.layers.len().to_string()).color(t.text_faint));
            if ui.small_button("Only").on_hover_text("Show only this group").clicked() {
                *action = Some(Owned::Isolate(group.layers.clone()));
            }
        })
        .body(|ui| {
            for &i in &group.layers {
                row(ui, t, model, i, false, action);
            }
        });
}

fn row(ui: &mut egui::Ui, t: &Tokens, model: &LayerModel, i: usize, show_trade: bool, action: &mut Option<Owned>) {
    let Some(layer) = model.layers.get(i) else { return };
    let visibility = layer.visibility();
    ui.horizontal(|ui| {
        if eye(ui, t, visibility).clicked() {
            *action = Some(Owned::Set(vec![i], visibility != Visibility::On));
        }
        let fg = if visibility == Visibility::Off { t.text_faint } else { t.text };
        let name = ui.label(RichText::new(&layer.parsed.base).color(fg));
        let mut detail = Vec::new();
        if let Some(xref) = &layer.parsed.xref {
            detail.push(xref.clone());
        }
        if show_trade {
            detail.push(layer.parsed.discipline.label().to_owned());
        }
        if layer.members.len() > 1 {
            detail.push(format!("×{}", layer.members.len()));
        }
        if !detail.is_empty() {
            ui.label(RichText::new(detail.join(" · ")).small().color(t.text_faint));
        }
        name.on_hover_text(&layer.name);
    });
}

/// An eye toggle: open when on, crossed out when off, faint when mixed.
fn eye(ui: &mut egui::Ui, t: &Tokens, visibility: Visibility) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    let (icon, tint) = match visibility {
        Visibility::On => ("eye", t.icon),
        Visibility::Mixed => ("eye", t.text_faint),
        Visibility::Off => ("eye-off", t.text_faint),
    };
    crate::icons::paint(ui, Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), icon, 16.0, tint);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, visibility != Visibility::Off, "Layer visibility"));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Apply the panel's changes to the document's view in one batch (one re-render).
pub(crate) fn apply(app: &mut crate::PdfCraftApp, index: usize, id: DocId, changes: &[(LayerId, bool)]) {
    if changes.is_empty() {
        return;
    }
    let ops: Vec<(LayerOp, LayerId)> = changes.iter().map(|(layer, on)| (if *on { LayerOp::On } else { LayerOp::Off }, *layer)).collect();
    if app.session.set_layer_state(id, &ops, false)
        && let Some(view) = app.views.get_mut(index)
    {
        view.invalidate_content();
    }
}
