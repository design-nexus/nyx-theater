//! The mini-player: while you browse with a video loaded, a card in the corner keeps
//! it playing live, with its transport, and takes you back to it.
use crate::{playback::Playback, prefs, widgets, window};
use gtk::{glib, prelude::*};
use nyx_media_model::{MediaItem, MediaKind};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

const WIDTH: i32 = 320;

pub fn build(overlay: &gtk::Overlay, player: &Rc<RefCell<Playback>>) {
    let card = widgets::vbox(0);
    card.add_css_class("theater-mini");
    card.set_halign(gtk::Align::End);
    card.set_valign(gtk::Align::End);
    card.set_margin_end(20);
    card.set_margin_bottom(20);
    card.set_width_request(WIDTH);
    card.set_visible(false);

    let video = gtk::Overlay::new();
    video.add_css_class("theater-mini-video");
    video.set_overflow(gtk::Overflow::Hidden);
    // The card's size comes from this frame; the picture would ask for the video's own.
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_size_request(WIDTH, WIDTH * 9 / 16);
    video.set_child(Some(&frame));
    let picture = gtk::Picture::for_paintable(&nyx_playback::video::paintable());
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    video.add_overlay(&picture);
    let expand = gtk::Image::from_icon_name("view-fullscreen-symbolic");
    expand.add_css_class("theater-mini-expand");
    expand.set_halign(gtk::Align::End);
    expand.set_valign(gtk::Align::Start);
    expand.set_can_target(false);
    video.add_overlay(&expand);
    video.set_cursor_from_name(Some("pointer"));
    video.set_tooltip_text(Some("Return to playback"));
    let open = gtk::GestureClick::new();
    open.connect_released(|_, _, _, _| window::navigate("playing"));
    video.add_controller(open);
    card.append(&video);

    let info = widgets::vbox(2);
    info.add_css_class("theater-mini-info");
    let title = widgets::label("", "theater-mini-title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_xalign(0.0);
    let subtitle = widgets::label("", "theater-mini-subtitle");
    subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    subtitle.set_xalign(0.0);
    info.append(&title);
    info.append(&subtitle);
    card.append(&info);

    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    seek.add_css_class("theater-mini-seek");
    seek.set_draw_value(false);
    seek.set_hexpand(true);
    card.append(&seek);
    let times = widgets::hbox(0);
    times.add_css_class("theater-mini-times");
    let position = widgets::label("", "theater-mini-time");
    let remaining = widgets::label("", "theater-mini-time");
    position.set_hexpand(true);
    position.set_xalign(0.0);
    remaining.set_xalign(1.0);
    times.append(&position);
    times.append(&remaining);
    card.append(&times);

    let transport = widgets::hbox(4);
    transport.add_css_class("theater-mini-transport");
    transport.set_halign(gtk::Align::Center);
    let back = nyx_ui::widgets::icon_button("media-seek-backward-symbolic", "Seek back");
    let play = nyx_ui::widgets::icon_button("media-playback-pause-symbolic", "Pause");
    play.add_css_class("theater-mini-play");
    let fwd = nyx_ui::widgets::icon_button("media-seek-forward-symbolic", "Seek forward");
    let next = nyx_ui::widgets::icon_button("media-skip-forward-symbolic", "Next in playlist");
    let stop = nyx_ui::widgets::icon_button("window-close-symbolic", "Stop playback");
    for button in [&back, &play, &fwd, &next, &stop] {
        transport.append(button);
    }
    card.append(&transport);
    for (button, direction) in [(&back, -1.0), (&fwd, 1.0)] {
        let p = player.clone();
        button.connect_clicked(move |_| p.borrow().seek_by(direction * prefs::get().seek_seconds as f64));
    }
    let p = player.clone();
    play.connect_clicked(move |_| p.borrow().toggle());
    next.connect_clicked(|_| crate::playlists::advance(1));
    let p = player.clone();
    stop.connect_clicked(move |_| p.borrow_mut().close());

    // A drag seeks once it settles, as the player's own seek bar does.
    let dragging = Rc::new(Cell::new(0_u32));
    let (d, p) = (dragging.clone(), player.clone());
    seek.connect_change_value(move |_, _, value| {
        let generation = d.get().wrapping_add(1);
        d.set(generation);
        let (d, p) = (d.clone(), p.clone());
        glib::timeout_add_local_once(Duration::from_millis(200), move || {
            if d.get() == generation {
                d.set(0);
                p.borrow().seek(value);
            }
        });
        glib::Propagation::Proceed
    });

    overlay.add_overlay(&card);
    let weak_card = card.downgrade();
    let weak_player = Rc::downgrade(player);
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let (Some(card), Some(player)) = (weak_card.upgrade(), weak_player.upgrade()) else { return glib::ControlFlow::Break };
        if window::is_closing() {
            return glib::ControlFlow::Break;
        }
        let p = player.borrow();
        let loaded = p.has_video();
        crate::inhibit::set_playing(loaded && !p.paused);
        let full = window::window().is_some_and(|w| w.is_fullscreen());
        let shown = loaded && !full && window::page() != "playing";
        card.set_visible(shown);
        if !shown {
            return glib::ControlFlow::Continue;
        }
        if title.text() != p.title {
            title.set_text(&p.title);
            title.set_tooltip_text(Some(&p.title));
        }
        let about = p.media().map(describe).unwrap_or_else(|| "Local file".into());
        subtitle.set_text(&about);
        subtitle.set_visible(!about.is_empty());
        let duration = p.duration();
        if (seek.adjustment().upper() - duration.max(1.0)).abs() > 0.5 {
            seek.set_range(0.0, duration.max(1.0));
        }
        seek.set_sensitive(p.loaded && duration > 0.0);
        if dragging.get() == 0 {
            seek.set_value(p.position);
        }
        let at = if dragging.get() == 0 { p.position } else { seek.value() };
        position.set_text(&nyx_playback::controls::time(at));
        remaining.set_text(&if duration > 0.0 {
            format!("−{}", nyx_playback::controls::time((duration - at).max(0.0)))
        } else {
            String::new()
        });
        let playing = p.loaded && !p.paused;
        play.set_icon_name(if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
        play.set_tooltip_text(Some(if playing { "Pause" } else { "Play" }));
        next.set_visible(crate::playlists::has_queue());
        next.set_sensitive(crate::playlists::can_step(1));
        glib::ControlFlow::Continue
    });
}

/// The line under the title: the show and episode, or the year.
fn describe(item: &MediaItem) -> String {
    match item.kind {
        MediaKind::Episode => {
            let show = item.tv.series_title.clone().unwrap_or_default();
            let number = match (item.season_number, item.episode_number) {
                (Some(s), Some(e)) => format!("S{s} · E{e}"),
                _ => String::new(),
            };
            [show, number].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
        }
        _ => item.year.map(|y| y.to_string()).unwrap_or_default(),
    }
}
