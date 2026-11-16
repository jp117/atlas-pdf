use crate::util::hash128;
use crate::{InterpreterWarning, WarningSinkFn};
use hayro_syntax::object::{Array, Dict, MaybeRef, Name, Null, ObjRef, Object, Stream};
use kurbo::Affine;
use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

type CacheMap = HashMap<u128, Option<Box<dyn Any + Send + Sync>>>;
/// The objects read from a document, by key. PdfCraft patch (48): the second field counts the
/// ICC profiles read through it ([`Cache::icc_profile_reads`]). PdfCraft patch (80): the third
/// counts the `/Indexed` lookup tables read ([`Cache::indexed_lookup_reads`]).
#[derive(Clone)]
pub(crate) struct Cache(Arc<Mutex<CacheMap>>, Arc<AtomicUsize>, Arc<AtomicUsize>);
impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    pub(crate) fn new() -> Self {
        Self(
            Arc::new(Mutex::new(HashMap::new())),
            Arc::default(),
            Arc::default(),
        )
    }

    /// PdfCraft patch (48): count one ICC profile read, the colour space of an `/ICCBased`
    /// profile no entry held yet being built from it.
    pub(crate) fn count_icc_profile_read(&self) {
        self.1.fetch_add(1, Ordering::Relaxed);
    }

    /// PdfCraft patch (48): how many ICC profiles were read through this cache.
    pub(crate) fn icc_profile_reads(&self) -> usize {
        self.1.load(Ordering::Relaxed)
    }

    /// PdfCraft patch (80): count one `/Indexed` lookup table read (a stream decoded or a
    /// string copied).
    pub(crate) fn count_indexed_lookup_read(&self) {
        self.2.fetch_add(1, Ordering::Relaxed);
    }

    /// PdfCraft patch (80): how many `/Indexed` lookup tables were read through this cache.
    pub(crate) fn indexed_lookup_reads(&self) -> usize {
        self.2.load(Ordering::Relaxed)
    }

    pub(crate) fn get_or_insert_with<T: Clone + Send + Sync + 'static>(
        &self,
        id: u128,
        f: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let mut locked = self.0.lock().unwrap();

        // We can't use `get_or_insert_with` here, because if the closure makes another access to the
        // cache, we end up with a deadlock.
        match locked.entry(id) {
            Entry::Occupied(o) => o
                .get()
                .as_ref()
                .and_then(|val| val.downcast_ref::<T>().cloned()),
            Entry::Vacant(_) => {
                drop(locked);
                let val = f();
                self.0.lock().unwrap().insert(
                    id,
                    val.clone()
                        .map(|val| Box::new(val) as Box<dyn Any + Send + Sync>),
                );

                val
            }
        }
    }
}

/// PdfCraft patch: a byte-budgeted cache of decoded images, keyed by everything a decode reads
/// ([`DecodedImageKey`]). Image-heavy pages decode thousands of images; without this every tile
/// and every scale decodes them all again. Entries are shared (`Arc`), so a hit lends the pixels
/// without copying them; when an insert would pass the budget the images used longest ago go
/// first (Atlas fork: it dropped the whole map, so a page whose images together passed the budget
/// decoded all of them again on every render), which only costs re-decoding, and an image larger
/// than the whole budget is never kept.
/// The budget caps memory per cache, which several interpreter caches may share
/// ([`SharedImageCache`]). While one thread decodes an image, the others that ask for it wait
/// for that decode (at most [`PENDING_WAIT`]) instead of repeating it alongside, which for a large
/// scan multiplied its memory by the render threads. A poisoned lock decodes without the cache.
///
/// PdfCraft patch (91): a decode that fails is kept too, as the warnings it reported, so an
/// image drawn again and again that doesn't decode (damaged data, a JPX or JBIG2 stream its
/// decoder refuses) isn't decoded again at every draw: a later draw reports the same warnings
/// and gets nothing, as the decode did. A failure counts [`FAILED_DECODE_BYTES`] towards the
/// budget, so failures can't grow the map without bound either. A failure that may come from
/// what the page had left (its JBIG2 allowance, `ContentTruncated`) isn't kept: another page,
/// or the same page drawn again, starts with a fresh allowance. One that left out a JBIG2 mask
/// for a limit its stream reaches by itself is kept, with the image it takes along (patch 73,
/// [`note_stream_limited`]): decoding them again would only refuse them again, and an image of
/// any size under such a mask was decoded again at every draw.
#[derive(Clone)]
pub(crate) struct DecodedImageCache {
    inner: Arc<(Mutex<DecodedImageCacheInner>, Condvar)>,
}

/// PdfCraft patch: what a decoded image depends on. Two images share an entry only when all of
/// it is equal:
/// - where the stream's dictionary lies in the document's data (`dict_at`, `dict_len`): a
///   stream's data follows its dictionary, so this tells apart streams whose dictionaries are
///   byte for byte the same (two uncompressed images of one size), which `dict_hash` alone
///   would not; the data stays put for as long as the document (and so this cache) lives;
/// - the dictionary's bytes (`dict_hash`: `/Decode`, `/ColorSpace`, `/SMask`, `/Mask`,
///   `/BitsPerComponent`, filters, …) and its object (`obj`: the decryption key depends on it);
/// - whether it is decoded as a mask, a stencil mask, raster or mask output, interpolation, and
///   the decode target size where the decode reads it (JPEG 2000 only, see
///   `ImageXObject::decode_key`);
/// - the default colour space drawn in place of the image's device space, if any (the cache
///   key of its object, PdfCraft patch 38): the same image can be drawn by resources with a
///   `/DefaultRGB` and by resources without one.
///
/// A transfer function (graphics state `/TR`) also changes the pixels and has no cheap key, so
/// an image drawn under one is not cached; neither is an inline image, whose dictionary lives in
/// a content stream and whose colour space can be a name resolved through the resources.
/// Previews decode exactly as final renders do (they only skip drawing tiny images), so the two
/// share entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DecodedImageKey {
    pub(crate) mask_output: bool,
    pub(crate) dict_at: usize,
    pub(crate) dict_len: usize,
    pub(crate) dict_hash: u128,
    pub(crate) obj: Option<(i32, i32)>,
    pub(crate) is_mask: bool,
    pub(crate) is_stencil_mask: bool,
    pub(crate) interpolate: bool,
    pub(crate) default_space: Option<u128>,
    pub(crate) target: Option<(u32, u32)>,
    /// PdfCraft patch (82): the size a soft mask far larger than where it is drawn was averaged
    /// down to (`ImageXObject::decoded_soft_mask`), `None` for a mask kept at its own size.
    pub(crate) shrunk_to: Option<(u32, u32)>,
}

/// PdfCraft patch (91): what the budget charges for a failed decode, about what its key and
/// entry take in the map.
const FAILED_DECODE_BYTES: usize = 128;

/// PdfCraft patch (91): with the warnings of a failure kept, that it left out a JBIG2 mask for a
/// limit its stream reaches by itself ([`note_stream_limited`]); no warning's flag.
const STREAM_LIMITED: u8 = 128;

std::thread_local! {
    static LEFT_OUT_BY_STREAM_LIMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// PdfCraft patch (73): note, wherever a decode reports `ContentTruncated` for an image or a kept
/// failure replays it, whether that left out a JBIG2 mask for a limit its stream reaches by itself
/// (`hayro_syntax::jbig2_limited_by_stream`): its failure, and the failure of the image it takes
/// along, is then the stream's to keep, since decoding it again refuses it again.
pub(crate) fn note_stream_limited(limited: bool) {
    LEFT_OUT_BY_STREAM_LIMIT.with(|left_out| left_out.set(limited));
}

fn stream_limited() -> bool {
    LEFT_OUT_BY_STREAM_LIMIT.with(std::cell::Cell::get)
}

/// PdfCraft patch (91): what a [`DecodedImageCache`] keeps for a key.
enum DecodedEntry {
    /// The decoded image, shared with the threads drawing it.
    Decoded(Arc<dyn Any + Send + Sync>),
    /// The decode failed, reporting these warnings ([`warning_bit`] flags).
    Failed(u8),
}

/// PdfCraft patch (91): `warning` as a flag in a set of warnings.
fn warning_bit(warning: InterpreterWarning) -> u8 {
    match warning {
        InterpreterWarning::UnsupportedFont => 1,
        InterpreterWarning::ImageDecodeFailure => 2,
        InterpreterWarning::ContentTruncated => 4,
        InterpreterWarning::ColorFallback => 8,
        InterpreterWarning::Type3WidthsMissing => 16,
        InterpreterWarning::MalformedIndexedImage => 32,
    }
}

/// PdfCraft patch (91): the warnings in a set of [`warning_bit`] flags.
fn warnings_in(bits: u8) -> impl Iterator<Item = InterpreterWarning> {
    [
        InterpreterWarning::UnsupportedFont,
        InterpreterWarning::ImageDecodeFailure,
        InterpreterWarning::ContentTruncated,
        InterpreterWarning::ColorFallback,
        InterpreterWarning::Type3WidthsMissing,
        InterpreterWarning::MalformedIndexedImage,
    ]
    .into_iter()
    .filter(move |warning| bits & warning_bit(*warning) != 0)
}

/// Atlas fork: a cached image, what it holds and when it was last asked for.
struct KeptImage {
    entry: DecodedEntry,
    size: usize,
    used: u64,
}

struct DecodedImageCacheInner {
    map: HashMap<DecodedImageKey, KeptImage>,
    bytes: usize,
    budget: usize,
    /// Atlas fork: counts the asks, to tell which image was used longest ago.
    clock: u64,
    /// Images some thread is decoding now.
    pending: HashSet<DecodedImageKey>,
    /// Images decoded too large for the budget: every thread decodes these on its own, rather
    /// than wait for a decode it can't use.
    oversized: HashSet<DecodedImageKey>,
    /// How many decodes ran through the cache (misses), for tests.
    decodes: usize,
    /// PdfCraft patch (82): how many of `decodes` were of masks (keys with `mask_output`).
    mask_decodes: usize,
}

/// PdfCraft patch: the decoded-image budget of an interpreter cache of its own.
const DEFAULT_DECODED_IMAGE_BYTES: usize = 64 << 20;

/// PdfCraft patch: the longest a thread waits for another's decode of the same image before it
/// decodes the image itself. Decodes take milliseconds to a few seconds; this bounds the wait if
/// one never finishes. Every wake-up counts as a whole [`PENDING_POLL`], so other decodes ending
/// meanwhile can only shorten the wait.
const PENDING_WAIT: Duration = Duration::from_secs(5);

/// PdfCraft patch: how often a waiting thread looks again, in case a wake-up was missed.
const PENDING_POLL: Duration = Duration::from_millis(50);

impl DecodedImageCache {
    /// A cache keeping at most [`DEFAULT_DECODED_IMAGE_BYTES`] of decoded pixels.
    pub(crate) fn new() -> Self {
        Self::with_budget(DEFAULT_DECODED_IMAGE_BYTES)
    }

    /// A cache keeping at most `budget` bytes of decoded pixels.
    pub(crate) fn with_budget(budget: usize) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(DecodedImageCacheInner {
                    map: HashMap::new(),
                    bytes: 0,
                    budget,
                    clock: 0,
                    pending: HashSet::new(),
                    oversized: HashSet::new(),
                    decodes: 0,
                    mask_decodes: 0,
                }),
                Condvar::new(),
            )),
        }
    }

    /// PdfCraft patch (32): how many decodes the cache was asked to keep (its misses), for
    /// diagnostics and tests. 0 when the lock is poisoned.
    pub(crate) fn decodes(&self) -> usize {
        self.inner.0.lock().map_or(0, |inner| inner.decodes)
    }

    /// PdfCraft patch (82): how many of [`Self::decodes`] were of soft masks and `/Mask` images,
    /// for tests. 0 when the lock is poisoned.
    pub(crate) fn mask_decodes(&self) -> usize {
        self.inner.0.lock().map_or(0, |inner| inner.mask_decodes)
    }

    /// The decoded image for `key`, from the cache or from `f`, whose result holds `size_of`
    /// bytes; `f` decodes reporting its warnings to the sink it is handed. `None` as the key
    /// decodes without the cache. PdfCraft patch (91): `sink` gets the warnings of the decode,
    /// or those of the failed decode kept for `key`.
    pub(crate) fn get_or_insert_with<T: Send + Sync + 'static>(
        &self,
        key: Option<DecodedImageKey>,
        size_of: impl FnOnce(&T) -> usize,
        sink: &WarningSinkFn,
        f: impl FnOnce(&WarningSinkFn) -> Option<T>,
    ) -> Option<Arc<T>> {
        let Some(key) = key else {
            return f(sink).map(Arc::new);
        };
        let (lock, decoded) = &*self.inner;
        // A thread panicked while holding the lock: decode without the cache.
        let Ok(mut inner) = lock.lock() else {
            return f(sink).map(Arc::new);
        };
        let mut waited = Duration::ZERO;
        loop {
            inner.clock = inner.clock.saturating_add(1);
            let clock = inner.clock;
            // A hit shares what is there; a kept failure (PdfCraft patch 91) reports its
            // warnings again without decoding.
            if let Some(kept) = inner.map.get_mut(&key) {
                kept.used = clock;
                let hit = match &kept.entry {
                    DecodedEntry::Decoded(val) => Ok(val.clone()),
                    DecodedEntry::Failed(bits) => Err(*bits),
                };
                // The warnings go out without the lock: a sink may draw or log.
                drop(inner);
                return match hit {
                    Ok(val) => val.downcast::<T>().ok(),
                    Err(bits) => {
                        note_stream_limited(bits & STREAM_LIMITED != 0);
                        warnings_in(bits).for_each(|warning| sink(warning));
                        None
                    }
                };
            }
            // Wait while another thread decodes this image. A single-threaded build (the web)
            // has no other thread, and its condition variable can't wait.
            if cfg!(target_arch = "wasm32")
                || !inner.pending.contains(&key)
                || inner.oversized.contains(&key)
                || waited >= PENDING_WAIT
            {
                break;
            }
            inner = match decoded.wait_timeout(inner, PENDING_POLL) {
                Ok((inner, _)) => inner,
                Err(_) => return f(sink).map(Arc::new),
            };
            waited = waited.saturating_add(PENDING_POLL);
        }
        // Claim the decode, unless another thread still holds it (the wait ran out) or the
        // image is known not to fit: then this thread decodes on its own. (Built lazily: a
        // claim dropped unclaimed would release another thread's, under the lock held here.)
        let claim =
            (!inner.oversized.contains(&key) && inner.pending.insert(key)).then(|| PendingDecode {
                cache: &self.inner,
                key,
            });
        inner.decodes = inner.decodes.saturating_add(1);
        if key.mask_output {
            inner.mask_decodes = inner.mask_decodes.saturating_add(1);
        }
        drop(inner);
        // Decode without holding the lock (decoding must never deadlock the cache), noting the
        // warnings it reports (PdfCraft patch 91).
        let reported = Arc::new(AtomicU8::new(0));
        let recording: WarningSinkFn = {
            let (reported, sink) = (reported.clone(), sink.clone());
            Arc::new(move |warning| {
                reported.fetch_or(warning_bit(warning), Ordering::Relaxed);
                sink(warning);
            })
        };
        let val = f(&recording).map(Arc::new);
        let reported = reported.load(Ordering::Relaxed);
        let Ok(mut inner) = lock.lock() else {
            return val;
        };
        // Another thread may have decoded it meanwhile: keep the first copy.
        match inner.map.get(&key).map(|kept| &kept.entry) {
            Some(DecodedEntry::Decoded(existing)) => {
                if let Ok(existing) = existing.clone().downcast::<T>() {
                    return Some(existing);
                }
            }
            Some(DecodedEntry::Failed(_)) if val.is_none() => return None,
            _ => {}
        }
        let truncated = reported & warning_bit(InterpreterWarning::ContentTruncated) != 0;
        let size = match val.as_deref() {
            Some(v) => Some(size_of(v)),
            // PdfCraft patch (91): a failure the page's JBIG2 allowance may have caused isn't
            // the image's to keep, nor one a JBIG2 decode's work did, which may be the page's too.
            None if hayro_syntax::page_jbig2_work_spent() || (truncated && !stream_limited()) => None,
            None => Some(FAILED_DECODE_BYTES),
        };
        match size {
            Some(size) if size > inner.budget => {
                inner.oversized.insert(key);
            }
            Some(size) => {
                inner.make_room(size);
                inner.bytes = inner.bytes.saturating_add(size);
                let used = inner.clock;
                let entry = match &val {
                    Some(val) => DecodedEntry::Decoded(val.clone() as Arc<dyn Any + Send + Sync>),
                    // Here only when a limit of its stream refused the mask that left it out.
                    None if truncated => DecodedEntry::Failed(reported | STREAM_LIMITED),
                    None => DecodedEntry::Failed(reported),
                };
                // An entry replaced (a failure another thread kept) gives its bytes back.
                if let Some(old) = inner.map.insert(key, KeptImage { entry, size, used }) {
                    inner.bytes = inner.bytes.saturating_sub(old.size);
                }
            }
            None => {}
        }
        // Release the lock before the claim, which takes it again to let the waiters go.
        drop(inner);
        drop(claim);
        val
    }
}

impl DecodedImageCacheInner {
    /// Atlas fork: lets go of the images used longest ago until `size` more bytes fit the budget.
    /// A thread still drawing one keeps it alive until it is done. A kept failure counts its
    /// `FAILED_DECODE_BYTES` and goes the same way.
    fn make_room(&mut self, size: usize) {
        while self.bytes.saturating_add(size) > self.budget {
            let oldest = self
                .map
                .iter()
                .min_by_key(|(_, kept)| kept.used)
                .map(|(key, _)| *key);
            let Some(kept) = oldest.and_then(|key| self.map.remove(&key)) else {
                // Nothing left: `bytes` must already be 0.
                self.bytes = 0;
                return;
            };
            self.bytes = self.bytes.saturating_sub(kept.size);
        }
    }
}

/// PdfCraft patch: a decode a thread has claimed (`DecodedImageCacheInner::pending`). Dropping
/// it releases the claim and wakes the threads waiting for it, however the decode ended (a
/// panic included), so a waiter never waits for a decode that is not running.
struct PendingDecode<'c> {
    cache: &'c (Mutex<DecodedImageCacheInner>, Condvar),
    key: DecodedImageKey,
}

impl Drop for PendingDecode<'_> {
    fn drop(&mut self) {
        let (lock, decoded) = self.cache;
        if let Ok(mut inner) = lock.lock() {
            inner.pending.remove(&self.key);
        }
        decoded.notify_all();
    }
}

/// PdfCraft patch: decoded image pixels shared by several interpreter caches over the same
/// document data (the threads of a render pool), within one byte budget, so a large scan is
/// decoded and held once rather than once per thread. Its keys hold where each image's
/// dictionary lies in the document's data, so every cache sharing it must interpret the same
/// bytes, and it must not outlive them.
#[derive(Clone)]
pub struct SharedImageCache(pub(crate) DecodedImageCache);

impl SharedImageCache {
    /// A cache keeping at most `budget` bytes of decoded pixels.
    pub fn new(budget: usize) -> Self {
        Self(DecodedImageCache::with_budget(budget))
    }

    /// How many images were decoded through the cache (its misses), for PdfCraft's tests.
    #[doc(hidden)]
    pub fn decodes(&self) -> usize {
        self.0.inner.0.lock().map_or(0, |inner| inner.decodes)
    }

    /// Atlas fork: how many bytes of decoded pixels the cache holds, for PdfCraft's tests.
    #[doc(hidden)]
    pub fn bytes(&self) -> usize {
        self.0.inner.0.lock().map_or(0, |inner| inner.bytes)
    }
}

/// A trait for objects that can generate a unique cache key.
pub trait CacheKey {
    /// Returns the cache key for this object.
    fn cache_key(&self) -> u128;
}

impl<T: CacheKey, U: CacheKey> CacheKey for (T, U) {
    fn cache_key(&self) -> u128 {
        hash128(&(self.0.cache_key(), self.1.cache_key()))
    }
}

impl CacheKey for Dict<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self.data())
    }
}

impl CacheKey for Stream<'_> {
    fn cache_key(&self) -> u128 {
        self.dict().cache_key()
    }
}

impl CacheKey for Null {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl CacheKey for bool {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl CacheKey for hayro_syntax::object::Number {
    fn cache_key(&self) -> u128 {
        hash128(&self.as_f64().to_bits())
    }
}

impl CacheKey for hayro_syntax::object::String<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self.as_ref())
    }
}

impl CacheKey for Name<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl CacheKey for Array<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self.data())
    }
}

impl CacheKey for Object<'_> {
    fn cache_key(&self) -> u128 {
        match self {
            Object::Null(n) => n.cache_key(),
            Object::Boolean(b) => b.cache_key(),
            Object::Number(n) => n.cache_key(),
            Object::String(s) => s.cache_key(),
            Object::Name(n) => n.cache_key(),
            Object::Dict(d) => d.cache_key(),
            Object::Array(a) => a.cache_key(),
            Object::Stream(s) => s.cache_key(),
        }
    }
}

impl CacheKey for ObjRef {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl<T: CacheKey> CacheKey for MaybeRef<T> {
    fn cache_key(&self) -> u128 {
        match self {
            Self::Ref(r) => r.cache_key(),
            Self::NotRef(o) => o.cache_key(),
        }
    }
}

impl CacheKey for Affine {
    fn cache_key(&self) -> u128 {
        let c = self.as_coeffs();
        hash128(&[
            c[0].to_bits(),
            c[1].to_bits(),
            c[2].to_bits(),
            c[3].to_bits(),
            c[4].to_bits(),
            c[5].to_bits(),
        ])
    }
}

impl CacheKey for u128 {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}
