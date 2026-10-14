//! Construction-drawing layer grouping.
//!
//! CAD exports carry one optional content group per CAD layer. Most follow the US National CAD
//! Standard (NCS): a one- or two-letter discipline designator, a dash, then a major group, as in
//! `E-POWR-RISE` or `EP-LITE`. Attached xrefs prefix their layers with the xref name
//! (`ARCH-BG|A-WALL`) and bound xrefs with `$n$` (`1ST FLOOR BG$0$BG`). Files combined page by
//! page can wrap each sheet in a layer called `Page 1`, `Page 2`, …; those are containers, and
//! turning one off blanks the whole sheet, so presets and isolate keep them on.
//!
//! [`LayerModel`] merges groups that share a name (CAD files combined into one PDF repeat `0`,
//! `Defpoints`, `TEXT`, …), groups them by discipline or by source drawing, and turns panel
//! actions into the minimal list of `(id, visible)` changes for the viewer to apply in one batch.

use std::collections::{HashMap, HashSet};

/// An optional content group's object id: (object number, generation).
pub type LayerId = (u32, u16);

/// One optional content group as the viewer currently shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerInput<'a> {
    pub id: LayerId,
    pub name: &'a str,
    pub visible: bool,
}

/// The trade a layer belongs to, from its NCS designator or, failing that, its words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Discipline {
    Electrical,
    FireProtection,
    Telecom,
    Mechanical,
    Plumbing,
    Structural,
    Architectural,
    Interiors,
    Civil,
    Landscape,
    Equipment,
    General,
    Other,
}

impl Discipline {
    /// Display order: electrical first, since that is what Atlas quotes from.
    pub const ALL: [Discipline; 13] = [
        Discipline::Electrical,
        Discipline::FireProtection,
        Discipline::Telecom,
        Discipline::Mechanical,
        Discipline::Plumbing,
        Discipline::Structural,
        Discipline::Architectural,
        Discipline::Interiors,
        Discipline::Civil,
        Discipline::Landscape,
        Discipline::Equipment,
        Discipline::General,
        Discipline::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Discipline::Electrical => "Electrical",
            Discipline::FireProtection => "Fire protection",
            Discipline::Telecom => "Telecom & security",
            Discipline::Mechanical => "Mechanical",
            Discipline::Plumbing => "Plumbing",
            Discipline::Structural => "Structural",
            Discipline::Architectural => "Architectural",
            Discipline::Interiors => "Interiors",
            Discipline::Civil => "Civil & site",
            Discipline::Landscape => "Landscape",
            Discipline::Equipment => "Equipment",
            Discipline::General => "General & annotation",
            Discipline::Other => "Other / CAD",
        }
    }

    /// Background trades that "Electrical focus" hides.
    pub fn is_background(self) -> bool {
        matches!(
            self,
            Discipline::Mechanical
                | Discipline::Plumbing
                | Discipline::Structural
                | Discipline::Architectural
                | Discipline::Interiors
                | Discipline::Civil
                | Discipline::Landscape
        )
    }
}

/// What a layer name says about the layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedName {
    /// The xref (attached or bound) the layer came from, if any.
    pub xref: Option<String>,
    /// The layer name without the xref prefix.
    pub base: String,
    pub discipline: Discipline,
    /// `Page N`: a whole-sheet container from a combined file.
    pub page_container: bool,
}

/// Parse a layer name. Total: any string gives an answer, unknown names land in
/// [`Discipline::Other`].
pub fn parse_name(name: &str) -> ParsedName {
    let trimmed = name.trim();
    let (xref, base) = split_xref(trimmed);
    let page_container = xref.is_none() && is_page_container(base);
    let discipline = if page_container {
        Discipline::Other
    } else {
        designator_discipline(base).or_else(|| keyword_discipline(base)).or_else(|| xref.and_then(keyword_discipline)).unwrap_or(Discipline::Other)
    };
    ParsedName { xref: xref.map(str::to_owned), base: base.to_owned(), discipline, page_container }
}

/// `XREF|LAYER` (attached) or `XREF$0$LAYER` (bound). Nested attached xrefs keep their full
/// path (`A|B|LAYER` → `A|B`).
fn split_xref(name: &str) -> (Option<&str>, &str) {
    if let Some((xref, base)) = name.rsplit_once('|')
        && !xref.is_empty()
        && !base.is_empty()
    {
        return (Some(xref), base);
    }
    for (i, _) in name.match_indices('$') {
        // `$` is ASCII, so i + 1 is a char boundary.
        let Some(after) = name.get(i + 1..) else { continue };
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || after.as_bytes().get(digits) != Some(&b'$') {
            continue;
        }
        if let (Some(xref), Some(base)) = (name.get(..i), after.get(digits + 1..))
            && !xref.is_empty()
            && !base.is_empty()
        {
            return (Some(xref), base);
        }
    }
    (None, name)
}

/// `Page 12`, `page_3`, `PAGE7`.
fn is_page_container(name: &str) -> bool {
    let Some(prefix) = name.get(..4) else { return false };
    if !prefix.eq_ignore_ascii_case("page") {
        return false;
    }
    let Some(rest) = name.get(4..) else { return false };
    let rest = rest.strip_prefix([' ', '_', '-']).unwrap_or(rest);
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

/// The NCS designator before the first `-` or `_`: one or two ASCII letters.
fn designator_discipline(name: &str) -> Option<Discipline> {
    let end = name.find(['-', '_'])?;
    let token = name.get(..end)?;
    let mut chars = token.chars();
    let first = chars.next()?;
    let second = chars.next();
    if chars.next().is_some() || !first.is_ascii_alphabetic() || second.is_some_and(|c| !c.is_ascii_alphabetic()) {
        return None;
    }
    match first.to_ascii_uppercase() {
        // W is distributed energy (PV, generation): part of the power system.
        'E' | 'W' => Some(Discipline::Electrical),
        'F' => Some(Discipline::FireProtection),
        'T' => Some(Discipline::Telecom),
        'M' => Some(Discipline::Mechanical),
        'P' => Some(Discipline::Plumbing),
        'S' => Some(Discipline::Structural),
        'A' => Some(Discipline::Architectural),
        'I' => Some(Discipline::Interiors),
        // C civil, V survey and mapping, B geotechnical.
        'C' | 'V' | 'B' => Some(Discipline::Civil),
        'L' => Some(Discipline::Landscape),
        'Q' => Some(Discipline::Equipment),
        'G' => Some(Discipline::General),
        _ => None,
    }
}

/// Keywords by discipline, checked in this order. A keyword of four or more letters matches a
/// word that starts with it; shorter ones must match a whole word (so `FA` is not `FACADE`).
#[rustfmt::skip]
const KEYWORDS: &[(Discipline, &[&str])] = &[
    (
        Discipline::Electrical,
        &[
            "ELEC", "ELECTRICAL", "POWR", "POWER", "LITE", "LIGHT", "LTG", "CIRCUIT", "CKT", "PANEL", "PNL", "SWBD", "SWGR",
            "SWITCHBOARD", "SWITCHGEAR", "XFMR", "TRANSFORMER", "CONDUIT", "FEEDER", "RECEP", "GRND", "GROUND", "DIMMER",
        ],
    ),
    (Discipline::FireProtection, &["FIRE", "SPRINK", "SPKR", "SPRK", "FA", "FP"]),
    (Discipline::Telecom, &["TELE", "COMM", "DATA", "CCTV", "SECUR", "TEL", "AV"]),
    (Discipline::Mechanical, &["MECH", "HVAC", "DUCT", "DIFF", "VAV", "AHU", "RTU", "EXHAUST"]),
    (Discipline::Plumbing, &["PLUMB", "PLMB", "SANR", "SANI", "DOMW", "WATER"]),
    (
        Discipline::Structural,
        &["STRU", "STRC", "COLS", "COLUMN", "BEAM", "FNDN", "FOUND", "FTNG", "FOOTING", "CONC", "REBAR", "STEEL", "SLAB", "JOIST"],
    ),
    (Discipline::Architectural, &["ARCH", "WALL", "DOOR", "FLOR", "FLOOR", "WINDOW", "GLAZ", "STAIR", "CEIL", "ROOF", "ROOM", "PARTITION"]),
    (Discipline::Interiors, &["FURN", "CASEWORK", "FINISH"]),
    (Discipline::Civil, &["SITE", "CURB", "PAVE", "TOPO", "ROAD", "GRADE", "PARKING", "SIDEWALK"]),
    (Discipline::Landscape, &["LAND", "PLANT", "TREE", "IRRIG"]),
    (Discipline::Equipment, &["EQPM", "EQUIP"]),
    (
        Discipline::General,
        &[
            "TEXT", "TITLE", "TBLOCK", "TTLB", "BORDER", "ANNO", "DIM", "DIMS", "DIMENSION", "NOTE", "GRID", "VPORT", "VIEWPORT",
            "SHEET", "LOGO", "SYMB",
        ],
    ),
];

fn keyword_discipline(name: &str) -> Option<Discipline> {
    let upper = name.to_ascii_uppercase();
    let words: Vec<&str> = upper.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    KEYWORDS.iter().find_map(|(discipline, keys)| {
        let hit = keys.iter().any(|key| words.iter().any(|w| *w == *key || (key.len() >= 4 && w.starts_with(key))));
        hit.then_some(*discipline)
    })
}

/// Whether a merged layer is fully on, fully off, or mixed (duplicates that disagree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    On,
    Off,
    Mixed,
}

/// Every optional content group that shares one (case-insensitive) name.
#[derive(Clone, Debug, PartialEq)]
pub struct MergedLayer {
    /// The trimmed, upper-cased name: stable across files and sessions, used for saved state.
    pub key: String,
    /// The first spelling seen.
    pub name: String,
    pub parsed: ParsedName,
    /// Each group's id and current visibility, in document order, without duplicate ids.
    pub members: Vec<(LayerId, bool)>,
}

impl MergedLayer {
    pub fn visibility(&self) -> Visibility {
        let on = self.members.iter().filter(|(_, v)| *v).count();
        if on == 0 {
            Visibility::Off
        } else if on == self.members.len() {
            Visibility::On
        } else {
            Visibility::Mixed
        }
    }
}

/// A heading in the Layers panel and the merged layers under it (indexes into
/// [`LayerModel::layers`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerGroup {
    pub title: String,
    pub layers: Vec<usize>,
}

/// Something the user did in the Layers panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerAction<'a> {
    /// Show or hide these merged layers (one row's eye, a group's eye, or the search results).
    Set { layers: &'a [usize], visible: bool },
    /// Show only these merged layers (and the page containers); hide everything else.
    Isolate { layers: &'a [usize] },
    /// Hide architectural, interiors, structural, civil, landscape, mechanical and plumbing
    /// layers; show the rest.
    ElectricalFocus,
    /// Show every layer.
    ShowAll,
}

/// The layers of one document, merged by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerModel {
    pub layers: Vec<MergedLayer>,
}

const PAGES_TITLE: &str = "Combined-file pages";
const THIS_DRAWING: &str = "This drawing";

impl LayerModel {
    pub fn new(input: &[LayerInput<'_>]) -> Self {
        let mut layers: Vec<MergedLayer> = Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        let mut seen: HashSet<LayerId> = HashSet::new();
        for layer in input {
            // A reference listed twice in /OCGs is one group; the first entry wins.
            if !seen.insert(layer.id) {
                continue;
            }
            let key = layer.name.trim().to_uppercase();
            match by_key.get(&key).and_then(|&i| layers.get_mut(i)) {
                Some(merged) => merged.members.push((layer.id, layer.visible)),
                None => {
                    by_key.insert(key.clone(), layers.len());
                    layers.push(MergedLayer {
                        key,
                        name: layer.name.trim().to_owned(),
                        parsed: parse_name(layer.name),
                        members: vec![(layer.id, layer.visible)],
                    });
                }
            }
        }
        Self { layers }
    }

    /// Groups in [`Discipline::ALL`] order, then the page containers; empty groups are left out.
    pub fn by_discipline(&self) -> Vec<LayerGroup> {
        let mut groups: Vec<LayerGroup> = Discipline::ALL
            .iter()
            .map(|d| LayerGroup { title: d.label().to_owned(), layers: self.sorted(|l| !l.parsed.page_container && l.parsed.discipline == *d) })
            .collect();
        groups.push(LayerGroup { title: PAGES_TITLE.to_owned(), layers: self.sorted(|l| l.parsed.page_container) });
        groups.retain(|g| !g.layers.is_empty());
        groups
    }

    /// The document's own layers first, then one group per xref (alphabetical), then the page
    /// containers.
    pub fn by_source(&self) -> Vec<LayerGroup> {
        let mut xrefs: Vec<&str> = self.layers.iter().filter_map(|l| l.parsed.xref.as_deref()).collect();
        xrefs.sort_by_key(|x| x.to_uppercase());
        xrefs.dedup();
        let mut groups =
            vec![LayerGroup { title: THIS_DRAWING.to_owned(), layers: self.sorted(|l| !l.parsed.page_container && l.parsed.xref.is_none()) }];
        for xref in xrefs {
            groups.push(LayerGroup { title: xref.to_owned(), layers: self.sorted(|l| l.parsed.xref.as_deref() == Some(xref)) });
        }
        groups.push(LayerGroup { title: PAGES_TITLE.to_owned(), layers: self.sorted(|l| l.parsed.page_container) });
        groups.retain(|g| !g.layers.is_empty());
        groups
    }

    /// Indexes of the layers whose name contains `query` (case-insensitive), in name order.
    /// An empty query matches everything.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_uppercase();
        self.sorted(|l| l.key.contains(&needle))
    }

    fn sorted(&self, keep: impl Fn(&MergedLayer) -> bool) -> Vec<usize> {
        let mut out: Vec<usize> = self.layers.iter().enumerate().filter(|(_, l)| keep(l)).map(|(i, _)| i).collect();
        out.sort_by(|&a, &b| {
            let (la, lb) = (self.layers.get(a), self.layers.get(b));
            let ka = la.map(|l| (l.parsed.page_number(), l.key.as_str()));
            let kb = lb.map(|l| (l.parsed.page_number(), l.key.as_str()));
            ka.cmp(&kb)
        });
        out
    }

    /// The visibility of a set of merged layers taken together (a group heading's eye).
    pub fn visibility_of(&self, layers: &[usize]) -> Visibility {
        let mut any_on = false;
        let mut any_off = false;
        for merged in layers.iter().filter_map(|&i| self.layers.get(i)) {
            match merged.visibility() {
                Visibility::On => any_on = true,
                Visibility::Off => any_off = true,
                Visibility::Mixed => return Visibility::Mixed,
            }
        }
        match (any_on, any_off) {
            (true, true) => Visibility::Mixed,
            (false, true) => Visibility::Off,
            _ => Visibility::On,
        }
    }

    /// The `(id, visible)` changes an action needs: only groups whose state actually changes.
    /// Out-of-range indexes are ignored.
    pub fn changes(&self, action: LayerAction<'_>) -> Vec<(LayerId, bool)> {
        let wanted: Vec<Option<bool>> = match action {
            LayerAction::Set { layers, visible } => {
                let mut wanted = vec![None; self.layers.len()];
                for &i in layers {
                    if let Some(slot) = wanted.get_mut(i) {
                        *slot = Some(visible);
                    }
                }
                wanted
            }
            LayerAction::Isolate { layers } => {
                let keep: HashSet<usize> = layers.iter().copied().collect();
                self.layers.iter().enumerate().map(|(i, l)| Some(l.parsed.page_container || keep.contains(&i))).collect()
            }
            LayerAction::ElectricalFocus => {
                self.layers.iter().map(|l| Some(l.parsed.page_container || !l.parsed.discipline.is_background())).collect()
            }
            LayerAction::ShowAll => vec![Some(true); self.layers.len()],
        };
        self.diff(&wanted)
    }

    fn diff(&self, wanted: &[Option<bool>]) -> Vec<(LayerId, bool)> {
        self.layers
            .iter()
            .zip(wanted)
            .filter_map(|(l, w)| w.map(|v| (l, v)))
            .flat_map(|(l, v)| l.members.iter().filter(move |(_, now)| *now != v).map(move |(id, _)| (*id, v)))
            .collect()
    }

    /// The state to remember for this file: each fully-on or fully-off merged layer by key.
    /// Mixed layers are left out so the document's own defaults apply to them next time.
    pub fn snapshot(&self) -> Vec<(String, bool)> {
        self.layers
            .iter()
            .filter_map(|l| match l.visibility() {
                Visibility::On => Some((l.key.clone(), true)),
                Visibility::Off => Some((l.key.clone(), false)),
                Visibility::Mixed => None,
            })
            .collect()
    }

    /// The changes that bring the document back to a [`snapshot`](Self::snapshot). Keys the
    /// document no longer has are ignored; layers the snapshot doesn't mention keep their state.
    pub fn restore(&self, saved: &[(String, bool)]) -> Vec<(LayerId, bool)> {
        let saved: HashMap<&str, bool> = saved.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let wanted: Vec<Option<bool>> = self.layers.iter().map(|l| saved.get(l.key.as_str()).copied()).collect();
        self.diff(&wanted)
    }
}

impl ParsedName {
    /// The sheet number of a page container, for sorting `Page 2` before `Page 10`.
    fn page_number(&self) -> Option<u64> {
        if !self.page_container {
            return None;
        }
        let digits: String = self.base.chars().filter(char::is_ascii_digit).take(18).collect();
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(names: &[&'a str]) -> Vec<LayerInput<'a>> {
        names.iter().enumerate().map(|(i, name)| LayerInput { id: (u32::try_from(i).unwrap() + 1, 0), name, visible: true }).collect()
    }

    #[test]
    fn ncs_designators_pick_the_discipline() {
        assert_eq!(parse_name("E-POWR-RISE").discipline, Discipline::Electrical);
        assert_eq!(parse_name("EP-LITE").discipline, Discipline::Electrical);
        assert_eq!(parse_name("A-WALL-PATT-1").discipline, Discipline::Architectural);
        assert_eq!(parse_name("S-COLS").discipline, Discipline::Structural);
        assert_eq!(parse_name("P-SANR-FIXT").discipline, Discipline::Plumbing);
        assert_eq!(parse_name("I-WALL").discipline, Discipline::Interiors);
        assert_eq!(parse_name("e_names").discipline, Discipline::Electrical);
    }

    #[test]
    fn keywords_cover_names_without_a_designator() {
        assert_eq!(parse_name("-RAL ELECTRICAL").discipline, Discipline::Electrical);
        assert_eq!(parse_name("MECHANICAL EQUIP").discipline, Discipline::Mechanical);
        assert_eq!(parse_name("CIRCUIT").discipline, Discipline::Electrical);
        assert_eq!(parse_name("00-Flor").discipline, Discipline::Architectural);
        assert_eq!(parse_name("CONCRETE PAD").discipline, Discipline::Structural);
        assert_eq!(parse_name("TBLOCK").discipline, Discipline::General);
        // Short keywords match whole words only.
        assert_eq!(parse_name("FACADE").discipline, Discipline::Other);
        assert_eq!(parse_name("FA").discipline, Discipline::FireProtection);
    }

    #[test]
    fn cad_system_layers_are_other() {
        for name in ["0", "Defpoints", "Visible (ANSI)", "DEFAULT_2", "7_ALL_ROUNDS", "", "   ", "ÉLÉ-ÇTRIQUE"] {
            assert_eq!(parse_name(name).discipline, Discipline::Other, "{name:?}");
        }
    }

    #[test]
    fn xref_prefixes_are_split_off() {
        let attached = parse_name("ARCH-BG|A-WALL");
        assert_eq!(attached.xref.as_deref(), Some("ARCH-BG"));
        assert_eq!(attached.base, "A-WALL");
        assert_eq!(attached.discipline, Discipline::Architectural);

        let bound = parse_name("1ST FLOOR BG$0$BG");
        assert_eq!(bound.xref.as_deref(), Some("1ST FLOOR BG"));
        assert_eq!(bound.base, "BG");
        // Nothing in "BG"; the xref name says floor plan.
        assert_eq!(bound.discipline, Discipline::Architectural);

        assert_eq!(parse_name("A|B|E-POWR").xref.as_deref(), Some("A|B"));
        // Not xrefs: empty sides, `$` without digits.
        for name in ["|A-WALL", "A-WALL|", "$0$X", "COST$ $X", "A$$B", "A$1"] {
            assert_eq!(parse_name(name).xref, None, "{name:?}");
        }
    }

    #[test]
    fn page_containers_are_detected() {
        for name in ["Page 1", "page 121", "PAGE_7", "Page-3", "Page12"] {
            assert!(parse_name(name).page_container, "{name:?}");
        }
        for name in ["Page", "Page ", "Pages 1", "Page 1a", "XREF|Page 1", "Pag"] {
            assert!(!parse_name(name).page_container, "{name:?}");
        }
    }

    #[test]
    fn same_names_merge_and_duplicate_ids_collapse() {
        let mut layers = input(&["0", "TEXT", "0", "text "]);
        layers.push(LayerInput { id: (1, 0), name: "0", visible: false });
        let model = LayerModel::new(&layers);
        assert_eq!(model.layers.len(), 2);
        assert_eq!(model.layers[0].members, vec![((1, 0), true), ((3, 0), true)]);
        assert_eq!(model.layers[1].name, "TEXT");
        assert_eq!(model.layers[1].members.len(), 2);
    }

    #[test]
    fn groups_follow_discipline_order_with_pages_last() {
        let model = LayerModel::new(&input(&["Page 10", "A-WALL", "E-POWR", "Page 2", "0", "E-LITE"]));
        let groups = model.by_discipline();
        let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Electrical", "Architectural", "Other / CAD", "Combined-file pages"]);
        let names = |g: &LayerGroup| g.layers.iter().map(|&i| model.layers[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&groups[0]), ["E-LITE", "E-POWR"]);
        assert_eq!(names(&groups[3]), ["Page 2", "Page 10"]);
    }

    #[test]
    fn source_groups_put_the_drawing_first() {
        let model = LayerModel::new(&input(&["zeta|A-WALL", "E-POWR", "Alpha$0$S-COLS", "Page 1"]));
        let titles: Vec<String> = model.by_source().into_iter().map(|g| g.title).collect();
        assert_eq!(titles, ["This drawing", "Alpha", "zeta", "Combined-file pages"]);
    }

    #[test]
    fn electrical_focus_hides_backgrounds_but_never_page_containers() {
        let model = LayerModel::new(&input(&["E-POWR", "A-WALL", "Page 1", "M-HVAC", "0"]));
        let mut changes = model.changes(LayerAction::ElectricalFocus);
        changes.sort();
        assert_eq!(changes, vec![((2, 0), false), ((4, 0), false)]);
    }

    #[test]
    fn isolate_keeps_targets_and_page_containers() {
        let mut layers = input(&["E-POWR", "A-WALL", "Page 1"]);
        for l in &mut layers {
            l.visible = false;
        }
        let model = LayerModel::new(&layers);
        let mut changes = model.changes(LayerAction::Isolate { layers: &[0] });
        changes.sort();
        assert_eq!(changes, vec![((1, 0), true), ((3, 0), true)]);
    }

    #[test]
    fn set_touches_every_member_that_differs_and_ignores_bad_indexes() {
        let layers = vec![LayerInput { id: (1, 0), name: "0", visible: true }, LayerInput { id: (2, 0), name: "0", visible: false }];
        let model = LayerModel::new(&layers);
        assert_eq!(model.layers[0].visibility(), Visibility::Mixed);
        assert_eq!(model.changes(LayerAction::Set { layers: &[0, 99], visible: true }), vec![((2, 0), true)]);
        assert_eq!(model.changes(LayerAction::Set { layers: &[usize::MAX], visible: false }), vec![]);
        assert_eq!(model.changes(LayerAction::ShowAll), vec![((2, 0), true)]);
    }

    #[test]
    fn snapshot_and_restore_round_trip_by_name() {
        let layers = vec![
            LayerInput { id: (1, 0), name: "E-POWR", visible: true },
            LayerInput { id: (2, 0), name: "A-WALL", visible: false },
            LayerInput { id: (3, 0), name: "0", visible: true },
            LayerInput { id: (4, 0), name: "0", visible: false },
        ];
        let model = LayerModel::new(&layers);
        let saved = model.snapshot();
        assert_eq!(saved, vec![("E-POWR".to_owned(), true), ("A-WALL".to_owned(), false)]);

        // Reopened with different ids and every layer on: the saved names win, the rest stay.
        let reopened = LayerModel::new(&[
            LayerInput { id: (7, 0), name: "a-wall", visible: true },
            LayerInput { id: (8, 0), name: "E-POWR", visible: true },
            LayerInput { id: (9, 0), name: "NEW", visible: true },
        ]);
        assert_eq!(reopened.restore(&saved), vec![((7, 0), false)]);
        assert_eq!(reopened.restore(&[("GONE".to_owned(), false)]), vec![]);
    }

    #[test]
    fn search_is_case_insensitive_and_empty_matches_all() {
        let model = LayerModel::new(&input(&["E-POWR-RISE", "A-WALL", "e-powr-panl"]));
        assert_eq!(model.search("powr").len(), 2);
        assert_eq!(model.search("  ").len(), 3);
        assert_eq!(model.search("nothing"), Vec::<usize>::new());
    }

    #[test]
    fn group_visibility_rolls_up() {
        let layers = vec![LayerInput { id: (1, 0), name: "A", visible: true }, LayerInput { id: (2, 0), name: "B", visible: false }];
        let model = LayerModel::new(&layers);
        assert_eq!(model.visibility_of(&[0]), Visibility::On);
        assert_eq!(model.visibility_of(&[1]), Visibility::Off);
        assert_eq!(model.visibility_of(&[0, 1]), Visibility::Mixed);
        assert_eq!(model.visibility_of(&[]), Visibility::On);
    }
}
