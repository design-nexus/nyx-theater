//! Live poster sizing retains existing models, widgets and scroll containers.
use crate::prefs;
use gtk::prelude::*;
use nyx_ui::media::{Artwork, PosterCard};
use std::cell::RefCell;
thread_local! { static CARDS: RefCell<Vec<Box<dyn Fn() -> bool>>> = RefCell::new(Vec::new()); }
pub fn width() -> i32 {
    match prefs::get().poster_size.as_str() {
        "small" => 150,
        "large" => 210,
        _ => 180,
    }
}
/// What a row of posters aims for: a little under the grid's size, so Home shows more.
pub fn row_width() -> i32 {
    width() * 3 / 4
}
/// Sets a poster card to `w` wide; false once the card has gone.
pub fn sizer(art: &Artwork, card: &PosterCard) -> impl Fn(i32) -> bool + 'static {
    let (root, picture, placeholder, title, meta) = (
        art.root.downgrade(),
        art.picture.downgrade(),
        art.placeholder.downgrade(),
        card.title.downgrade(),
        card.meta.downgrade(),
    );
    move |w| {
        let (Some(root), Some(picture), Some(placeholder), Some(title), Some(meta)) =
            (root.upgrade(), picture.upgrade(), placeholder.upgrade(), title.upgrade(), meta.upgrade())
        else {
            return false;
        };
        let h = w * 3 / 2;
        root.set_size_request(w, h);
        picture.set_size_request(w, h);
        placeholder.set_size_request(w, h);
        title.set_size_request(w, -1);
        meta.set_size_request(w, -1);
        true
    }
}
pub fn register(art: &Artwork, card: &PosterCard) {
    let size = sizer(art, card);
    let resize: Box<dyn Fn() -> bool> = Box::new(move || size(width()));
    resize();
    CARDS.with(|cards| {
        let mut cards = cards.borrow_mut();
        if cards.len() > 256 {
            cards.retain(|f| f());
        }
        cards.push(resize);
    });
}
pub fn resize() {
    CARDS.with(|cards| cards.borrow_mut().retain(|f| f()));
    crate::shelf::refit_all();
}
