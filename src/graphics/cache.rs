//! Built protocols, kept between frames.
//!
//! Encoding an image for a terminal is not free, and a panel redraws thirty
//! times a second. Rebuilding only when the picture or the space it goes in
//! actually changes is the difference between terminal graphics being usable
//! and not.
//!
//! There is one cache rather than one per kind of picture. A cover, a
//! rasterised button and an avatar are all the same thing here -- some pixels,
//! an identity for them, and the rectangle they were encoded for -- and a
//! single least-recently-used table is what keeps the terminal's own memory
//! bounded no matter which application is filling it.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ratatui::layout::Size;
use ratatui_image::protocol::Protocol;

/// Which picture a protocol was built from.
///
/// Deliberately not a name, a path or a URL. Two covers of the same album have
/// the same track and the same panel; keying on anything the application knows
/// the picture *by* rather than on the picture itself is how the second one
/// comes out looking like the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImageId(pub u64);

impl ImageId {
    /// The identity of a shared image: the address it lives at.
    ///
    /// Only sound while something holds the `Arc` -- a freed address is handed
    /// straight back out by the allocator, and the next picture along would
    /// inherit the last one's entry. [`ImageCache`] holds the `Arc` for as long
    /// as it holds the protocol, which is what makes this usable.
    pub fn of_arc<T>(img: &Arc<T>) -> Self {
        Self(Arc::as_ptr(img) as *const u8 as usize as u64)
    }

    /// An identity computed from whatever the picture will be drawn from.
    ///
    /// For pictures the application draws itself -- a vector icon in the
    /// theme's colours, a placeholder with a letter on it -- where there is no
    /// image to point at until it has been drawn, and the recipe is the
    /// identity.
    pub fn of(what: &impl Hash) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        what.hash(&mut h);
        Self(h.finish())
    }
}

/// What a protocol was built for: which picture, over how many cells, at what
/// size in pixels.
///
/// The cell count is in the key as well as the pixel size, and the two are not
/// the same fact. A protocol is transmitted as pixels but *placed* over a
/// number of cells, and the same pixel size can be reached from different cell
/// counts -- four cells of seven pixels and two cells of fourteen are both
/// twenty-eight. Keyed on pixels alone, a protocol built for two cells is
/// handed to a picture that spans four, and the two cells it does not cover
/// keep whatever the terminal already had there: a graphics placement is not
/// erased by painting the cell, only by another placement or a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub id: ImageId,
    /// The rectangle it was placed over.
    pub cells: Size,
    /// That rectangle in pixels, at the cell size in force when it was built.
    pub pixels: (u32, u32),
}

struct Entry {
    /// The picture this was built from, where there was one to hold.
    ///
    /// Held rather than merely pointed at, so an [`ImageId::of_arc`] cannot
    /// outlive the allocation it was taken from and be matched by whatever the
    /// allocator puts at that address next. `None` for a picture the
    /// application rasterised on the spot, whose identity is its recipe rather
    /// than its address.
    image: Option<Arc<image::RgbaImage>>,
    protocol: Protocol,
    /// When this was last handed out, on the cache's own clock.
    used: u64,
}

/// Protocols, keyed by picture and placement, with the oldest dropped first.
pub struct ImageCache {
    entries: HashMap<Key, Entry>,
    capacity: usize,
    clock: u64,
}

/// Entries kept before the least recently used one is dropped.
///
/// Large enough for a working set -- an application's own icons in the current
/// theme, plus the handful of pictures on screen -- and small enough that a
/// scroll through a thousand of them does not leave a thousand images in the
/// terminal's memory. Applications that know their own working set say so with
/// [`ImageCache::set_capacity`].
pub const DEFAULT_CAPACITY: usize = 64;

impl Default for ImageCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl ImageCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            clock: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Change how many entries are kept, dropping the oldest at once if the
    /// new figure is smaller.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        self.evict();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.entries.contains_key(key)
    }

    /// The protocol for `key`, if one was built, counting as a use.
    pub fn get(&mut self, key: &Key) -> Option<&Protocol> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self.entries.get_mut(key)?;
        entry.used = clock;
        Some(&entry.protocol)
    }

    /// Keep `protocol` against `key`, dropping the oldest entry if that puts
    /// the cache over capacity.
    ///
    /// `image` is the picture it was built from, where the key's identity came
    /// from that picture's address: holding it is what stops the address being
    /// reused.
    pub fn insert(&mut self, key: Key, image: Option<Arc<image::RgbaImage>>, protocol: Protocol) {
        self.clock += 1;
        self.entries.insert(
            key,
            Entry {
                image,
                protocol,
                used: self.clock,
            },
        );
        self.evict();
    }

    /// Forget every size of one picture.
    ///
    /// Kitty keeps uploaded images in the terminal's own memory, keyed by an
    /// id it assigned; dropping the protocol is what releases one.
    pub fn forget(&mut self, id: ImageId) {
        self.entries.retain(|k, _| k.id != id);
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Forget every picture not named in `keep`.
    ///
    /// For a view that knows, at the end of a frame, exactly which pictures it
    /// drew: everything else is off screen, and a scrolled-past image costs
    /// terminal memory until something says so.
    pub fn forget_unused(&mut self, keep: &HashSet<ImageId>) {
        self.entries.retain(|k, _| keep.contains(&k.id));
    }

    /// Drop least-recently-used entries until the cache is within capacity.
    fn evict(&mut self) {
        while self.entries.len() > self.capacity {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
            else {
                return;
            };
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::picker::Picker;
    use ratatui_image::Resize;

    /// A real protocol, built without a terminal: half blocks encode in
    /// memory, which is all a cache test needs of them.
    fn protocol() -> Protocol {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        Picker::halfblocks()
            .new_protocol(
                image::DynamicImage::ImageRgba8(img),
                Size::new(2, 1),
                Resize::Fit(None),
            )
            .expect("half blocks encode without a terminal")
    }

    fn key(id: u64, cells: (u16, u16), pixels: (u32, u32)) -> Key {
        Key {
            id: ImageId(id),
            cells: Size::new(cells.0, cells.1),
            pixels,
        }
    }

    #[test]
    fn a_picture_is_found_again_by_its_identity_and_its_placement() {
        let mut cache = ImageCache::default();
        let k = key(1, (12, 6), (96, 102));
        assert!(cache.get(&k).is_none(), "nothing built yet");
        cache.insert(k, None, protocol());
        assert!(cache.get(&k).is_some());

        // Another picture in the same rectangle, and the same picture in
        // another rectangle, are both misses.
        assert!(cache.get(&key(2, (12, 6), (96, 102))).is_none());
        assert!(cache.get(&key(1, (20, 10), (160, 170))).is_none());
    }

    /// The bug the pixel half of the key exists for. Four cells of seven
    /// pixels and two cells of fourteen are both twenty-eight, and a protocol
    /// built for one placed over the other leaves the cells it does not cover
    /// showing whatever the terminal had there before.
    #[test]
    fn the_same_pixels_over_a_different_number_of_cells_is_a_different_entry() {
        let mut cache = ImageCache::default();
        cache.insert(key(1, (4, 3), (28, 48)), None, protocol());
        assert!(cache.get(&key(1, (2, 3), (28, 48))).is_none());
        // And the same cells at a different cell size, which is what a font
        // zoom leaves behind.
        assert!(cache.get(&key(1, (4, 3), (56, 48))).is_none());
    }

    #[test]
    fn two_images_that_happen_to_be_alike_are_still_two_images() {
        // Identity is which picture the caller was handed, not what is in it.
        let one = Arc::new(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([1, 2, 3, 4]),
        ));
        let two = Arc::new(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([1, 2, 3, 4]),
        ));
        assert_ne!(ImageId::of_arc(&one), ImageId::of_arc(&two));
        assert_eq!(ImageId::of_arc(&one), ImageId::of_arc(&Arc::clone(&one)));
    }

    #[test]
    fn a_recipe_names_the_picture_it_will_draw() {
        // Two icons differing only in colour must not share an entry, and the
        // same icon in the same colour must find its own.
        assert_ne!(ImageId::of(&("play", 1u8)), ImageId::of(&("play", 2u8)));
        assert_eq!(ImageId::of(&("play", 1u8)), ImageId::of(&("play", 1u8)));
    }

    #[test]
    fn the_least_recently_used_picture_is_the_one_dropped() {
        let mut cache = ImageCache::new(2);
        let (a, b, c) = (
            key(1, (2, 1), (16, 32)),
            key(2, (2, 1), (16, 32)),
            key(3, (2, 1), (16, 32)),
        );
        cache.insert(a, None, protocol());
        cache.insert(b, None, protocol());
        // Touching `a` makes `b` the oldest.
        assert!(cache.get(&a).is_some());
        cache.insert(c, None, protocol());

        assert_eq!(cache.len(), 2);
        assert!(cache.contains(&a), "the one just used was dropped");
        assert!(!cache.contains(&b));
        assert!(cache.contains(&c));

        // And shrinking the cache drops the oldest at once rather than waiting
        // for the next insert.
        cache.set_capacity(1);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&c));
    }

    #[test]
    fn forgetting_a_picture_forgets_every_size_of_it() {
        let mut cache = ImageCache::default();
        cache.insert(key(1, (2, 1), (16, 32)), None, protocol());
        cache.insert(key(1, (4, 2), (32, 64)), None, protocol());
        cache.insert(key(2, (2, 1), (16, 32)), None, protocol());
        cache.forget(ImageId(1));
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&key(2, (2, 1), (16, 32))));
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn what_a_frame_did_not_draw_is_not_kept() {
        let mut cache = ImageCache::default();
        for id in 1..=4 {
            cache.insert(key(id, (2, 1), (16, 32)), None, protocol());
        }
        let drawn: HashSet<ImageId> = [ImageId(2), ImageId(4)].into_iter().collect();
        cache.forget_unused(&drawn);
        assert_eq!(cache.len(), 2);
        assert!(cache.contains(&key(2, (2, 1), (16, 32))));
        assert!(cache.contains(&key(4, (2, 1), (16, 32))));

        // Nothing drawn is nothing kept, which is what a view with no pictures
        // on it should cost the terminal.
        cache.forget_unused(&HashSet::new());
        assert!(cache.is_empty());
    }

    #[test]
    fn an_image_outlives_the_caller_that_handed_it_over() {
        // The address is the identity, so the entry has to keep the
        // allocation alive: a freed one comes straight back out of the
        // allocator, and the next picture along would inherit this entry.
        let img = Arc::new(image::RgbaImage::from_pixel(4, 4, image::Rgba([9; 4])));
        let id = ImageId::of_arc(&img);
        let mut cache = ImageCache::default();
        cache.insert(
            key(id.0, (2, 1), (16, 32)),
            Some(Arc::clone(&img)),
            protocol(),
        );
        assert_eq!(Arc::strong_count(&img), 2);
        drop(img);
        assert!(cache.contains(&key(id.0, (2, 1), (16, 32))));
    }
}
