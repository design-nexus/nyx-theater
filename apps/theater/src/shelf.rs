//! A row of cards on Home, Discover and a title's page: its name, a count, a link to
//! the whole page and arrows, over cards sized so a whole number fills the width.
use crate::{prefs, widgets};
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

type Sizer = Box<dyn Fn(i32)>;

/// The row's widgets; its sizing lives with the scroller's handlers, so dropping
/// this leaves the row working.
#[derive(Clone)]
pub struct Shelf {
    pub root: gtk::Box,
    pub content: gtk::Box,
    sub: gtk::Label,
    more: gtk::Button,
    state: Rc<State>,
}

/// Holds no widget strongly: the scroller's handlers own it.
struct State {
    scroll: glib::WeakRef<gtk::ScrolledWindow>,
    content: glib::WeakRef<gtk::Box>,
    back: glib::WeakRef<gtk::Button>,
    next: glib::WeakRef<gtk::Button>,
    gap: i32,
    /// The width a card aims for; the cards take what fills the row evenly.
    target: Box<dyn Fn() -> i32>,
    width: Cell<i32>,
    sizers: RefCell<Vec<(glib::WeakRef<gtk::Widget>, Sizer)>>,
}

thread_local! {
    static SHELVES: RefCell<Vec<Weak<State>>> = const { RefCell::new(Vec::new()) };
}

impl Shelf {
    pub fn new(title: &str, gap: i32, target: impl Fn() -> i32 + 'static) -> Shelf {
        let root = widgets::vbox(10);
        root.add_css_class("theater-shelf");
        let head = widgets::hbox(8);
        head.add_css_class("shelf-head");
        head.append(&widgets::label(title, "shelf-title"));
        let sub = widgets::label("", "shelf-sub");
        sub.set_visible(false);
        head.append(&sub);
        let spacer = widgets::hbox(0);
        spacer.set_hexpand(true);
        head.append(&spacer);
        let more = gtk::Button::new();
        more.add_css_class("shelf-more");
        more.set_valign(gtk::Align::Center);
        more.set_visible(false);
        head.append(&more);
        let back = nyx_ui::widgets::icon_button("pan-start-symbolic", "Scroll back");
        let next = nyx_ui::widgets::icon_button("pan-end-symbolic", "Scroll on");
        for arrow in [&back, &next] {
            arrow.add_css_class("shelf-arrow");
            arrow.set_valign(gtk::Align::Center);
            head.append(arrow);
        }
        root.append(&head);
        let content = widgets::hbox(gap);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .child(&content)
            .build();
        root.append(&scroll);
        let state = Rc::new(State {
            scroll: scroll.downgrade(),
            content: content.downgrade(),
            back: back.downgrade(),
            next: next.downgrade(),
            gap,
            target: Box::new(target),
            width: Cell::new(0),
            sizers: RefCell::new(Vec::new()),
        });
        let adjustment = scroll.hadjustment();
        let s = state.clone();
        adjustment.connect_page_size_notify(move |_| {
            let weak = Rc::downgrade(&s);
            glib::idle_add_local_once(move || {
                if let Some(s) = weak.upgrade() {
                    s.fit();
                }
            });
        });
        let s = state.clone();
        adjustment.connect_changed(move |_| s.arrows());
        let s = state.clone();
        adjustment.connect_value_changed(move |_| s.arrows());
        for (arrow, direction) in [(&back, -1.0), (&next, 1.0)] {
            let s = Rc::downgrade(&state);
            arrow.connect_clicked(move |_| {
                if let Some(s) = s.upgrade() {
                    s.step(direction);
                }
            });
        }
        state.arrows();
        SHELVES.with(|list| {
            let mut list = list.borrow_mut();
            list.retain(|w| w.strong_count() > 0);
            list.push(Rc::downgrade(&state));
        });
        Shelf { root, content, sub, more, state }
    }

    /// Grey text after the name ("4 in progress"), or none.
    pub fn set_sub(&self, text: &str) {
        self.sub.set_text(text);
        self.sub.set_visible(!text.is_empty());
    }

    /// A link at the head's end to the page the row comes from ("All movies"); once.
    pub fn set_more(&self, label: &str, open: impl Fn() + 'static) {
        if self.more.is_visible() {
            return;
        }
        self.more.set_label(label);
        self.more.set_visible(true);
        self.more.connect_clicked(move |_| open());
    }

    /// Size `card` with the row while it's in it. `size` gets the card's width.
    pub fn track(&self, card: &impl IsA<gtk::Widget>, size: impl Fn(i32) + 'static) {
        size(self.state.card_width());
        self.state.sizers.borrow_mut().push((card.as_ref().downgrade(), Box::new(size)));
    }

    pub fn append(&self, card: &impl IsA<gtk::Widget>, size: impl Fn(i32) + 'static) {
        self.content.append(card);
        self.track(card, size);
    }
}

impl State {
    fn card_width(&self) -> i32 {
        match self.width.get() {
            w if w > 0 => w,
            _ => (self.target)(),
        }
    }

    /// How many cards fill the row, and the width that makes them fit exactly.
    fn columns(&self) -> (i32, i32) {
        let target = (self.target)().max(1);
        let page = self.scroll.upgrade().map_or(0, |s| s.hadjustment().page_size() as i32);
        if page <= 0 {
            return (1, target);
        }
        let n = (((page + self.gap) as f64) / ((target + self.gap) as f64)).round().max(1.0) as i32;
        (n, ((page - self.gap * (n - 1)) / n).max(40))
    }

    fn fit(&self) {
        let (_, width) = self.columns();
        if width == self.width.get() {
            return;
        }
        self.width.set(width);
        let content = self.content.upgrade();
        self.sizers.borrow_mut().retain(|(card, size)| match card.upgrade() {
            Some(card) if card.parent().is_none() || card.parent().and_downcast::<gtk::Box>() == content => {
                size(width);
                true
            }
            _ => false,
        });
    }

    fn arrows(&self) {
        let (Some(scroll), Some(back), Some(next)) = (self.scroll.upgrade(), self.back.upgrade(), self.next.upgrade()) else {
            return;
        };
        let a = scroll.hadjustment();
        let fits = a.upper() - a.page_size() <= a.lower() + 1.0;
        back.set_visible(!fits);
        next.set_visible(!fits);
        back.set_sensitive(a.value() > a.lower() + 1.0);
        next.set_sensitive(a.value() < a.upper() - a.page_size() - 1.0);
    }

    /// A page of whole cards back or on.
    fn step(&self, direction: f64) {
        let Some(scroll) = self.scroll.upgrade() else { return };
        let a = scroll.hadjustment();
        let (n, width) = self.columns();
        let page = (n * (width + self.gap)) as f64;
        let end = (a.value() + direction * page).clamp(a.lower(), (a.upper() - a.page_size()).max(a.lower()));
        if prefs::get().reduce_motion {
            a.set_value(end);
            return;
        }
        let start = a.value();
        let begun = Cell::new(None::<i64>);
        scroll.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let t0 = begun.get().unwrap_or(now);
            begun.set(Some(t0));
            let t = ((now - t0) as f64 / 240_000.0).min(1.0);
            a.set_value(start + (end - start) * (1.0 - (1.0 - t).powi(3)));
            if t >= 1.0 { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    }
}

/// The poster size changed: every row sizes its cards again.
pub fn refit_all() {
    let shelves: Vec<_> = SHELVES.with(|list| {
        let mut list = list.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.iter().filter_map(Weak::upgrade).collect()
    });
    for s in shelves {
        s.width.set(-1);
        s.fit();
    }
}
