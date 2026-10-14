//! The app's names, kept in one place.

/// What the app is called in windows and dialogs.
pub const APP_NAME: &str = "Atlas PDF";

/// The name of its per-user folders (settings, crash recovery, digital IDs, the clipboard cache)
/// and of its single-instance socket's folder: different from PdfCraft's, so Atlas PDF never
/// shares settings with an installed PdfCraft or hands it files to open.
pub const DATA_NAME: &str = "AtlasPDF";

/// [`DATA_NAME`] for the lower-case XDG folders on Linux and FreeBSD.
pub const DATA_NAME_LOWER: &str = "atlaspdf";
