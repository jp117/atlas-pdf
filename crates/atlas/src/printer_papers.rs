//! Which paper a printer's driver takes, so the Print dialog can stay within it. The driver's list
//! decides what can be printed at size: the spooler refuses a job on a paper the driver doesn't
//! list. Only [`read`] touches the spooler; the rest is pure logic.

use pdfcraft_print::{LABEL_PAPER, PAPERS};

/// How far a driver's paper may be from a standard one, in micrometres: a quarter of a millimetre,
/// as the spooler matches a job's sheet.
const TOLERANCE: u32 = 250;

/// The most driver papers kept: drivers list a few hundred at most.
const MAX_SIZES: usize = 4096;

/// The index of US Letter in [`PAPERS`], the paper to prefer when a driver names no default.
const LETTER: usize = 0;

/// A paper's size in micrometres from points. The [`PAPERS`] sizes are small and finite.
fn micrometres(points: f64) -> u32 {
    (points * 25_400.0 / 72.0).round() as u32
}

/// `(short side, long side)`, so a paper matches either way round.
fn sides(size: (u32, u32)) -> (u32, u32) {
    (size.0.min(size.1), size.0.max(size.1))
}

/// Whether a driver's paper is the standard paper at `index`.
fn is_paper(size: (u32, u32), index: usize) -> bool {
    let Some((_, (width, height))) = PAPERS.get(index) else { return false };
    let (short, long) = sides(size);
    let (paper_short, paper_long) = sides((micrometres(*width), micrometres(*height)));
    short.abs_diff(paper_short) <= TOLERANCE && long.abs_diff(paper_long) <= TOLERANCE
}

/// The papers one driver lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverPapers {
    /// Each listed size in micrometres. At least one is a standard paper.
    sizes: Vec<(u32, u32)>,
    /// The size the driver uses by default, in micrometres.
    default: Option<(u32, u32)>,
}

impl DriverPapers {
    /// What a driver lists, in micrometres. `None` when none of it is a paper in [`PAPERS`] (a
    /// receipt printer, say): the list can't choose among the papers offered, so it says nothing.
    pub fn new(sizes: impl IntoIterator<Item = (u32, u32)>, default: Option<(u32, u32)>) -> Option<Self> {
        let sizes: Vec<(u32, u32)> = sizes.into_iter().filter(|(width, height)| *width > 0 && *height > 0).take(MAX_SIZES).collect();
        let papers = Self { sizes, default };
        (0..PAPERS.len()).any(|paper| papers.takes(paper)).then_some(papers)
    }

    /// Whether the driver lists the paper at this index of [`PAPERS`].
    pub fn takes(&self, paper: usize) -> bool {
        self.sizes.iter().any(|size| is_paper(*size, paper))
    }

    /// The paper to use when the one selected isn't listed: the driver's default, then US Letter,
    /// then the first it lists. Never label stock, unless that is all it takes.
    pub fn fallback(&self) -> Option<usize> {
        let default = self.default.and_then(|size| (0..PAPERS.len()).find(|paper| is_paper(size, *paper) && self.takes(*paper)));
        let listed = || (0..PAPERS.len()).filter(|paper| *paper != LABEL_PAPER).find(|paper| self.takes(*paper));
        default.or_else(|| self.takes(LETTER).then_some(LETTER)).or_else(listed).or_else(|| self.takes(LABEL_PAPER).then_some(LABEL_PAPER))
    }
}

/// What `printer`'s driver lists. `None` when it can't say, or on a system without this spooler.
/// It asks the driver, which can take seconds on a network printer, so read it off the GUI thread.
#[cfg(windows)]
pub fn read(printer: &str) -> Option<DriverPapers> {
    use winprint::printer::PrinterDevice;
    use winprint::ticket::{PrintCapabilities, PrintTicket, PrintTicketBuilder};

    let devices = PrinterDevice::all().ok()?;
    let device = devices.iter().find(|device| device.name() == printer)?;
    let capabilities = PrintCapabilities::fetch(device).ok()?;
    let sizes: Vec<(u32, u32)> = capabilities
        .page_media_sizes()
        .map(|media| {
            let size = media.size();
            (size.width_in_micron(), size.height_in_micron())
        })
        .collect();
    // Validating a ticket that asks for nothing gives the driver's defaults.
    let default = PrintTicketBuilder::new(device).ok().and_then(|mut builder| {
        builder.merge(PrintTicket::default()).ok()?;
        ticket_paper(builder.build().ok()?.get_xml())
    });
    DriverPapers::new(sizes, default)
}

/// The paper a validated print ticket holds, in micrometres.
#[cfg(windows)]
fn ticket_paper(xml: &[u8]) -> Option<(u32, u32)> {
    use winprint::ticket::document::reader::ParsableXmlDocument as _;
    use winprint::ticket::document::{NS_PSK, PrintTicketDocument, WithScoredProperties as _};

    let document = PrintTicketDocument::parse_from_bytes(xml).ok()?;
    let feature =
        document.features.iter().find(|feature| feature.name.local_name == "PageMediaSize" && feature.name.namespace_ref() == Some(NS_PSK))?;
    let option = feature.options.first()?;
    let side = |name: &str| option.get_scored_property(name, Some(NS_PSK))?.value_with(&document.parameter_inits)?.integer();
    Some((u32::try_from(side("MediaSizeWidth")?).ok()?, u32::try_from(side("MediaSizeHeight")?).ok()?))
}

/// What `printer`'s driver lists: nothing here, as this system's spooler doesn't say.
#[cfg(not(windows))]
pub fn read(_printer: &str) -> Option<DriverPapers> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER_SIZE: (u32, u32) = (215_900, 279_400);
    const LEGAL_SIZE: (u32, u32) = (215_900, 355_600);
    const A4_SIZE: (u32, u32) = (210_000, 297_000);
    const FOUR_BY_SIX: (u32, u32) = (101_600, 152_400);
    const ARCH_C_SIZE: (u32, u32) = (457_200, 609_600);

    fn paper(name: &str) -> usize {
        PAPERS.iter().position(|(paper, _)| paper.starts_with(name)).unwrap()
    }

    /// As HP LaserJet M607's driver lists them: nothing wider than Legal.
    #[test]
    fn an_office_printer_takes_its_own_sizes_and_no_drawing_sizes() {
        let hp = DriverPapers::new([LETTER_SIZE, LEGAL_SIZE, A4_SIZE, FOUR_BY_SIX], Some(LETTER_SIZE)).unwrap();
        for taken in ["US Letter", "US Legal", "A4"] {
            assert!(hp.takes(paper(taken)), "{taken}");
        }
        for refused in ["Tabloid", "ARCH C", "ARCH D", "A3"] {
            assert!(!hp.takes(paper(refused)), "{refused}");
        }
        assert_eq!(hp.fallback(), Some(paper("US Letter")));
    }

    #[test]
    fn a_paper_listed_the_other_way_round_is_the_same_paper() {
        let papers = DriverPapers::new([(609_600, 457_200)], None).unwrap();
        assert!(papers.takes(paper("ARCH C")));
    }

    /// The spooler matches to a quarter of a millimetre, so a list that passes here prints.
    #[test]
    fn a_paper_matches_to_a_quarter_of_a_millimetre_and_no_further() {
        assert!(DriverPapers::new([(457_450, 609_600)], None).unwrap().takes(paper("ARCH C")));
        assert_eq!(DriverPapers::new([(457_451, 609_600)], None), None, "not a standard paper");
    }

    #[test]
    fn a_driver_listing_no_standard_paper_says_nothing() {
        // An 80 mm receipt roll, a size nobody could read, and no sizes at all.
        assert_eq!(DriverPapers::new([(80_000, 297_000)], None), None);
        assert_eq!(DriverPapers::new([(0, 0)], None), None);
        assert_eq!(DriverPapers::new([], Some(LETTER_SIZE)), None);
    }

    #[test]
    fn a_paper_outside_the_list_of_papers_is_not_taken() {
        let papers = DriverPapers::new([LETTER_SIZE], None).unwrap();
        assert!(!papers.takes(PAPERS.len()));
        assert!(!papers.takes(usize::MAX));
    }

    #[test]
    fn the_fallback_is_the_drivers_default_when_it_is_a_paper_it_lists() {
        let roll = DriverPapers::new([LETTER_SIZE, ARCH_C_SIZE], Some(ARCH_C_SIZE)).unwrap();
        assert_eq!(roll.fallback(), Some(paper("ARCH C")));
    }

    #[test]
    fn the_fallback_is_letter_without_a_usable_default_and_then_the_first_listed() {
        // A default it doesn't list, and a default that is no standard paper.
        let both = [LEGAL_SIZE, LETTER_SIZE];
        assert_eq!(DriverPapers::new(both, Some(ARCH_C_SIZE)).unwrap().fallback(), Some(paper("US Letter")));
        assert_eq!(DriverPapers::new(both, Some((1, 1))).unwrap().fallback(), Some(paper("US Letter")));
        assert_eq!(DriverPapers::new(both, None).unwrap().fallback(), Some(paper("US Letter")));
        assert_eq!(DriverPapers::new([A4_SIZE, LEGAL_SIZE], None).unwrap().fallback(), Some(paper("US Legal")), "the order of PAPERS");
    }

    #[test]
    fn label_stock_is_the_fallback_only_when_it_is_all_a_printer_takes() {
        assert_eq!(DriverPapers::new([FOUR_BY_SIX, LEGAL_SIZE], None).unwrap().fallback(), Some(paper("US Legal")));
        assert_eq!(DriverPapers::new([FOUR_BY_SIX], None).unwrap().fallback(), Some(LABEL_PAPER));
    }

    #[test]
    fn a_printer_that_is_not_installed_says_nothing() {
        assert_eq!(read("No such printer, for this test"), None);
    }
}
