use crate::{playback::Playback, prefs, settings, widgets};
use gtk::{gdk, glib, prelude::*};
use nyx_ui::{chrome, settings_card};
use std::{cell::RefCell, rc::Rc};

struct Ui {
    window: gtk::ApplicationWindow,
    overlay: gtk::Overlay,
    nav: gtk::Box,
    stack: gtk::Stack,
    search: gtk::SearchEntry,
    /// The sidebar's foot: each server, and what the servers and the app are doing.
    servers: gtk::Box,
    status_icon: gtk::Image,
    status: gtk::Label,
    /// The Library group's counts, by page.
    counts: Vec<(&'static str, gtk::Label)>,
    /// The Playlists group's rows, refilled as the playlists load.
    playlists: gtk::Box,
    player: Rc<RefCell<Playback>>,
    /// The header (hidden in full screen) and its subtitle, the page you're on.
    header: gtk::WindowHandle,
    subtitle: gtk::Label,
    /// The sidebar's toggle, hidden with the sidebar on the player page.
    collapse: gtk::Button,
    /// The way out of the player page, in the toggle's place.
    back: gtk::Button,
    buttons: Vec<(&'static str, &'static str, gtk::Button)>,
}
thread_local! {
    /// The "Up next" card while it's counting down, and its timer.
    static UP_NEXT_DISMISSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static UP_NEXT: RefCell<Option<(gtk::Box, glib::SourceId)>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static NAV_HISTORY: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static NAV_FORWARD: RefCell<Vec<(String, Option<nyx_media_model::MediaItem>)>> = const { RefCell::new(Vec::new()) };
    static SEARCH_ORIGIN: RefCell<String> = RefCell::new(String::from("home"));
    static SEARCH_CLEARING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CLOSING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static COUNTS: std::cell::Cell<Counts> = const { std::cell::Cell::new(Counts { movies: 0, shows: 0, collections: 0 }) };
    /// The last message, until it's been on screen long enough to give way to the summary.
    static MESSAGE: RefCell<Option<(String, std::time::Instant)>> = const { RefCell::new(None) };
    static MESSAGE_TIMER: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}
/// How long a message stays at the sidebar's foot before the servers' summary returns.
const MESSAGE_SECS: u64 = 6;
pub fn is_closing() -> bool {
    CLOSING.with(|c| c.get())
}
pub fn window() -> Option<gtk::ApplicationWindow> {
    UI.with(|u| u.borrow().as_ref().map(|u| u.window.clone()))
}
pub fn overlay() -> Option<gtk::Overlay> {
    UI.with(|u| u.borrow().as_ref().map(|u| u.overlay.clone()))
}
/// Say what the app or a server is doing, at the sidebar's foot.
pub fn status(text: &str) {
    MESSAGE.with(|m| *m.borrow_mut() = Some((text.to_owned(), std::time::Instant::now())));
    if let Some(timer) = MESSAGE_TIMER.with(|t| t.borrow_mut().take()) {
        timer.remove();
    }
    let timer = glib::timeout_add_seconds_local_once(MESSAGE_SECS as u32, || {
        MESSAGE_TIMER.with(|t| t.borrow_mut().take());
        MESSAGE.with(|m| m.borrow_mut().take());
        update_servers();
    });
    MESSAGE_TIMER.with(|t| *t.borrow_mut() = Some(timer));
    update_servers();
}

/// Titles in each part of the library, for the sidebar.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Counts {
    pub movies: u64,
    pub shows: u64,
    pub collections: u64,
}
pub fn set_counts(counts: Counts) {
    if COUNTS.with(|c| c.replace(counts)) == counts {
        return;
    }
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            for (page, label) in &u.counts {
                let n = match *page {
                    "movies" => counts.movies,
                    "tv" => counts.shows,
                    _ => counts.collections,
                };
                label.set_text(&if n > 0 { thousands(n) } else { String::new() });
                label.set_visible(n > 0 && !u.nav.has_css_class("compact"));
            }
        }
    });
    update_servers();
}
/// "1,204".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
/// The servers card: a row per server with its state as a dot, then what's going on.
pub fn update_servers() {
    let Some((rows, icon, label, compact)) = UI.with(|u| {
        u.borrow().as_ref().map(|u| (u.servers.clone(), u.status_icon.clone(), u.status.clone(), u.nav.has_css_class("compact")))
    }) else {
        return;
    };
    while let Some(child) = rows.first_child() {
        rows.remove(&child);
    }
    let p = prefs::get();
    let states = crate::servers::connection_states();
    let configured: Vec<(nyx_media_model::ProviderKind, String)> = p
        .plex
        .map(|s| (nyx_media_model::ProviderKind::Plex, s.name))
        .into_iter()
        .chain(p.jellyfin.map(|s| (nyx_media_model::ProviderKind::Jellyfin, s.name)))
        .collect();
    for provider in [nyx_media_model::ProviderKind::Plex, nyx_media_model::ProviderKind::Jellyfin] {
        let name = match provider {
            nyx_media_model::ProviderKind::Plex => "Plex",
            nyx_media_model::ProviderKind::Jellyfin => "Jellyfin",
        };
        let row = gtk::Button::new();
        row.add_css_class("theater-server");
        let content = widgets::hbox(8);
        let icon = gtk::Image::from_icon_name("theater-server-symbolic");
        icon.add_css_class("server-icon");
        icon.add_css_class("compact-hide");
        content.append(&icon);
        content.append(&{
            let l = widgets::label(name, "server-name");
            l.add_css_class("compact-hide");
            l
        });
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("server-dot");
        dot.set_valign(gtk::Align::Center);
        if let Some((_, server)) = configured.iter().find(|(p, _)| *p == provider) {
            let state = states.iter().find(|s| s.provider == provider);
            let sub = widgets::label(server, "server-sub");
            sub.add_css_class("compact-hide");
            sub.set_hexpand(true);
            sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
            sub.set_max_width_chars(1);
            content.append(&sub);
            dot.add_css_class(match state {
                Some(s) if s.busy => "busy",
                Some(s) if s.connected && !s.message.contains("couldn't") => "ok",
                Some(s) if s.connected => "busy",
                _ => "off",
            });
            content.append(&dot);
            let message = state.map(|s| s.message.clone()).unwrap_or_default();
            row.set_tooltip_text(Some(&format!("{server} · {message}")));
            row.connect_clicked(move |_| match provider {
                nyx_media_model::ProviderKind::Plex => crate::plex::refresh(),
                nyx_media_model::ProviderKind::Jellyfin => crate::jellyfin::refresh(),
            });
        } else {
            if compact {
                continue;
            }
            let sub = widgets::label("Connect", "server-sub");
            sub.add_css_class("compact-hide");
            sub.set_hexpand(true);
            content.append(&sub);
            row.add_css_class("absent");
            row.set_tooltip_text(Some(&format!("Connect a {name} server")));
            row.connect_clicked(move |_| match provider {
                nyx_media_model::ProviderKind::Plex => crate::plex::connect_dialog(),
                nyx_media_model::ProviderKind::Jellyfin => crate::jellyfin::connect_dialog(),
            });
        }
        if compact {
            content.set_halign(gtk::Align::Center);
        }
        row.set_child(Some(&content));
        rows.append(&row);
    }
    let busy = states.iter().any(|s| s.busy);
    let trouble = states.iter().find(|s| !s.connected && !s.busy);
    let message = MESSAGE.with(|m| m.borrow().clone()).map(|(text, _)| text);
    let titles = COUNTS.with(|c| c.get());
    let summary = if configured.is_empty() {
        "No servers connected".to_owned()
    } else if let Some(state) = trouble {
        state.message.clone()
    } else if busy {
        states.iter().find(|s| s.busy).map(|s| s.message.clone()).unwrap_or_default()
    } else {
        format!("Up to date · {} titles", thousands(titles.movies + titles.shows))
    };
    let text = message.unwrap_or_else(|| summary.clone());
    label.set_text(&text);
    label.set_tooltip_text(Some(&text));
    for class in ["ok", "warn", "busy"] {
        icon.remove_css_class(class);
    }
    let (name, class) = if configured.is_empty() || trouble.is_some() {
        ("dialog-warning-symbolic", "warn")
    } else if busy {
        ("content-loading-symbolic", "busy")
    } else {
        ("object-select-symbolic", "ok")
    };
    icon.set_icon_name(Some(name));
    icon.add_css_class(class);
    nyx_ui::sidebar::set_compact(&rows, compact);
    if let Some(foot) = rows.next_sibling() {
        foot.set_halign(if compact { gtk::Align::Center } else { gtk::Align::Fill });
    }
}
/// One playlist in the sidebar: its id, name, count and where it comes from.
pub struct PlaylistRow {
    pub id: String,
    pub title: String,
    pub count: u64,
    pub server: Option<&'static str>,
}
/// Refill the Playlists group.
pub fn set_playlists(lists: Vec<PlaylistRow>) {
    let Some((rows, compact)) = UI.with(|u| u.borrow().as_ref().map(|u| (u.playlists.clone(), u.nav.has_css_class("compact")))) else {
        return;
    };
    while let Some(child) = rows.first_child() {
        rows.remove(&child);
    }
    if lists.is_empty() {
        let (button, _) = nav_row("theater-playlist-symbolic", "All playlists", None);
        button.set_widget_name("playlists");
        button.connect_clicked(|_| {
            crate::playlists::close_selection();
            navigate("playlists");
        });
        rows.append(&button);
    }
    for list in lists {
        let (button, content) = nav_row("theater-playlist-symbolic", &list.title, (list.count > 0).then(|| thousands(list.count)));
        if let Some(server) = list.server {
            let sub = widgets::label(server, "nav-sub");
            sub.add_css_class("compact-hide");
            sub.set_valign(gtk::Align::Center);
            let label = content.first_child().and_then(|c| c.next_sibling());
            content.insert_child_after(&sub, label.as_ref());
        }
        button.set_widget_name(&format!("playlist:{}", list.id));
        let id = list.id.clone();
        button.connect_clicked(move |_| {
            navigate("playlists");
            crate::playlists::open_id(&id);
        });
        rows.append(&button);
    }
    nyx_ui::sidebar::set_compact(&rows, compact);
    highlight();
}
/// A sidebar row: icon, name and a count at its end.
fn nav_row(icon: &str, title: &str, count: Option<String>) -> (gtk::Button, gtk::Box) {
    let button = gtk::Button::new();
    button.add_css_class("nav-item");
    let content = widgets::hbox(12);
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("nav-icon");
    content.append(&image);
    let label = widgets::label(title, "nav-label");
    label.add_css_class("compact-hide");
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&label);
    let readout = widgets::label(count.as_deref().unwrap_or(""), "nav-readout");
    readout.add_css_class("compact-hide");
    readout.set_visible(count.is_some());
    content.append(&readout);
    button.set_child(Some(&content));
    button.set_tooltip_text(Some(title));
    button.update_property(&[gtk::accessible::Property::Label(title)]);
    (button, content)
}
/// A group's heading in its section's colour, and the icon rail's bar in the same colour.
fn nav_group(list: &gtk::Box, title: &str, section: &str, first: bool, add: Option<(&str, fn())>) {
    let bar = gtk::Separator::new(gtk::Orientation::Horizontal);
    bar.add_css_class("nav-divider");
    bar.add_css_class("compact-show");
    bar.add_css_class(&format!("section-{section}"));
    bar.set_visible(false);
    list.append(&bar);
    let row = match add {
        Some((tip, run)) => nyx_ui::sidebar::group_label_with_add(title, first, tip, run),
        None => nyx_ui::sidebar::group_label(title, first),
    };
    nyx_ui::sidebar::set_section(&row, section);
    list.append(&row);
}
pub fn playlist_selected() {
    highlight();
}
/// Mark the sidebar row for the page (and the open playlist) you're on.
fn highlight() {
    UI.with(|u| {
        let u = u.borrow();
        let Some(u) = u.as_ref() else { return };
        let page = u.stack.visible_child_name().map(|s| s.to_string()).unwrap_or_default();
        for (id, _, button) in &u.buttons {
            if *id == page {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
        }
        let open = crate::playlists::selected_id().map(|id| format!("playlist:{id}"));
        let mut child = u.playlists.first_child();
        while let Some(row) = child {
            let name = row.widget_name();
            let on = page == "playlists" && (open.as_deref() == Some(name.as_str()) || (open.is_none() && name == "playlists"));
            if on {
                row.add_css_class("active");
            } else {
                row.remove_css_class("active");
            }
            child = row.next_sibling();
        }
    });
}

/// Seconds the "Up next" card counts down before playing.
const UP_NEXT_SECS: u32 = 10;

/// Offer `next` over the player: it plays when the countdown ends, or now, or not at all.
pub fn show_up_next(next: nyx_media_model::MediaItem) {
    show_up_next_card(next, false);
}

pub fn preview_up_next(next: nyx_media_model::MediaItem) {
    show_up_next_card(next, true);
}

pub fn up_next_dismissed() -> bool { UP_NEXT_DISMISSED.with(|v| v.get()) }

pub fn reset_up_next() {
    cancel_up_next();
    UP_NEXT_DISMISSED.with(|v| v.set(false));
}

fn show_up_next_card(next: nyx_media_model::MediaItem, preview: bool) {
    if up_next_dismissed() { return; }
    cancel_up_next();
    let Some(overlay) = overlay() else { return };
    let card = widgets::hbox(10);
    card.add_css_class("toast");
    card.add_css_class("up-next");
    card.set_halign(gtk::Align::End);
    card.set_valign(gtk::Align::End);
    card.set_margin_end(24);
    card.set_margin_bottom(96);
    let text = widgets::vbox(2);
    let name = match (next.season_number, next.episode_number) {
        (Some(s), Some(e)) => format!("S{s} · E{e} · {}", next.title),
        _ => next.title.clone(),
    };
    text.append(&widgets::label(&format!("Up next: {name}"), ""));
    let count = widgets::label(if preview { "After this episode" } else { "Plays in 10 s" }, "dim");
    text.append(&count);
    card.append(&text);
    let play = nyx_ui::widgets::icon_button("media-skip-forward-symbolic", "Play it now");
    let stop = nyx_ui::widgets::icon_button("window-close-symbolic", "Don't play it");
    card.append(&play);
    card.append(&stop);
    overlay.add_overlay(&card);
    let left = std::cell::Cell::new(UP_NEXT_SECS);
    let item = next.clone();
    let timer = glib::timeout_add_seconds_local(1, move || {
        if preview {
            let remaining = UI.with(|u| u.borrow().as_ref().map(|u| {
                let p = u.player.borrow();
                (p.duration() - p.position).max(0.0).ceil() as u32
            })).unwrap_or(0);
            let lead = prefs::get().up_next_seconds.min(30);
            if lead == 0 || remaining > lead {
                if let Some((card, _)) = UP_NEXT.with(|v| v.borrow_mut().take()) { remove_card(&card); }
                return glib::ControlFlow::Break;
            }
            count.set_text(&format!("Episode ends in {remaining} s"));
            return glib::ControlFlow::Continue;
        }
        let n = left.get().saturating_sub(1);
        left.set(n);
        if n == 0 {
            UP_NEXT.with(|u| u.borrow_mut().take()).inspect(|(c, _)| remove_card(c));
            crate::library::play_queued(&item);
            return glib::ControlFlow::Break;
        }
        count.set_text(&format!("Plays in {n} s"));
        glib::ControlFlow::Continue
    });
    let item = next;
    play.connect_clicked(move |_| {
        cancel_up_next();
        crate::library::play_queued(&item);
    });
    stop.connect_clicked(|_| {
        UP_NEXT_DISMISSED.with(|v| v.set(true));
        cancel_up_next();
    });
    UP_NEXT.with(|u| *u.borrow_mut() = Some((card, timer)));
}

/// Take the "Up next" card away without playing.
pub fn cancel_up_next() {
    if let Some((card, timer)) = UP_NEXT.with(|u| u.borrow_mut().take()) {
        timer.remove();
        remove_card(&card);
    }
}

fn remove_card(card: &gtk::Box) {
    if let Some(o) = card.parent().and_downcast::<gtk::Overlay>() {
        o.remove_overlay(card);
    }
}

pub fn build(app: &gtk::Application) {
    let window =
        gtk::ApplicationWindow::builder().application(app).title("Theater").default_width(1120).default_height(800).build();
    window.add_css_class("theater-window");
    window.add_css_class("playback-window");
    window.set_icon_name(Some("io.github.design_nexus.nyx.Theater"));
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    nyx_ui::keys::install(
        &window,
        nyx_ui::keys::Common {
            settings: Some(settings::open),
            shortcuts: Some(show_shortcuts),
            close: true,
            ..Default::default()
        },
    );
    let nav = widgets::vbox(0);
    nav.add_css_class("settings-navigation");
    nav.add_css_class("sidebar");
    nav.set_hexpand(false);
    let list = widgets::vbox(1);
    list.set_hexpand(false);
    let mut buttons = Vec::new();
    let mut counts = Vec::new();
    for (id, title, icon) in [
        ("home", "Home", "go-home-symbolic"),
        ("discover", "Discover", "theater-compass-symbolic"),
        ("movies", "Movies", "theater-film-symbolic"),
        ("tv", "TV shows", "tv-symbolic"),
        ("collections", "Collections", "theater-layers-symbolic"),
    ] {
        match id {
            "home" => nav_group(&list, "Watch", nyx_ui::sidebar::ACCENT_SECTION, true, None),
            "movies" => nav_group(&list, "Library", "swatch-green", false, None),
            _ => {}
        }
        let (button, content) = nav_row(icon, title, None);
        if id != "home" && id != "discover"
            && let Some(readout) = content.last_child().and_downcast::<gtk::Label>()
        {
            counts.push((id, readout));
        }
        button.connect_clicked(move |_| navigate(id));
        list.append(&button);
        buttons.push((id, title, button));
    }
    nav_group(&list, "Playlists", "swatch-violet", false, Some(("New playlist", crate::playlists::new_playlist)));
    let playlist_rows = widgets::vbox(1);
    list.append(&playlist_rows);
    nav.append(
        &gtk::ScrolledWindow::builder()
            .hexpand(false)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&list)
            .build(),
    );
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(if prefs::get().reduce_motion { 0 } else { 160 });
    crate::library::build(&stack);
    let player = Playback::attach(&window, Rc::new(status));
    let playing = crate::now_playing::build(&player);
    stack.add_named(&playing, Some("playing"));
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search movies and shows"));
    search.update_property(&[gtk::accessible::Property::Label("Search the library")]);
    search.add_css_class("bar-search");
    search.set_width_chars(30);
    search.set_max_width_chars(40);
    // Typing starts from the page you were on, so its search (movies, playlists…) applies.
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(|_| set_search_origin());
    search.add_controller(focus);
    search.connect_search_changed(|entry| {
        if SEARCH_CLEARING.with(|clearing| clearing.get()) { return; }
        let origin = SEARCH_ORIGIN.with(|origin| origin.borrow().clone());
        if origin == "playlists" { crate::playlists::search(&entry.text()); return; }
        crate::library::search(&entry.text(), &origin);
        if !entry.text().is_empty() {
            show_page("search");
        } else {
            show_page(&origin);
        }
    });
    search.connect_stop_search(|_| hide_search());
    let content = widgets::vbox(0);
    content.add_css_class("settings-content");
    content.set_hexpand(true);
    content.append(&stack);
    // The sidebar's foot: each server with a dot for its state, then what's going on
    // and Refresh.
    let card = widgets::vbox(0);
    card.add_css_class("theater-servers");
    let server_rows = widgets::vbox(0);
    card.append(&server_rows);
    let foot = widgets::hbox(6);
    foot.add_css_class("theater-servers-foot");
    let status_icon = gtk::Image::from_icon_name("object-select-symbolic");
    status_icon.add_css_class("status-icon");
    status_icon.add_css_class("compact-hide");
    foot.append(&status_icon);
    let readout = widgets::label("No servers connected", "status-readout");
    readout.add_css_class("compact-hide");
    readout.set_hexpand(true);
    readout.set_ellipsize(gtk::pango::EllipsizeMode::End);
    readout.set_max_width_chars(1);
    foot.append(&readout);
    let refresh = nyx_ui::widgets::icon_button("view-refresh-symbolic", "Refresh libraries");
    refresh.add_css_class("servers-refresh");
    refresh.connect_clicked(|_| {
        crate::servers::refresh();
        crate::playlists::refresh_servers();
    });
    foot.append(&refresh);
    card.append(&foot);
    nav.append(&card);
    // The header: the app and the sidebar's toggle, search, and while a video plays in
    // the background its controls, then opening a file and Settings.
    let header = chrome::app_header(&window, "theater-film-symbolic", "Theater");
    let collapse = nyx_ui::widgets::bar_button("sidebar-show-symbolic", "Collapse the sidebar (Ctrl+B)");
    collapse.connect_clicked(|_| toggle_sidebar());
    header.start.append(&collapse);
    let back_button = nyx_ui::widgets::bar_button("go-previous-symbolic", "Back (Alt+Left)");
    back_button.set_visible(false);
    back_button.connect_clicked(|_| back());
    header.start.append(&back_button);
    let search_box = gtk::Overlay::new();
    search_box.set_child(Some(&search));
    let hint = widgets::label("Ctrl F", "key-hint");
    hint.set_halign(gtk::Align::End);
    hint.set_valign(gtk::Align::Center);
    hint.set_margin_end(10);
    hint.set_can_target(false);
    search_box.add_overlay(&hint);
    search.connect_changed(move |s| hint.set_visible(s.text().is_empty()));
    header.center.append(&search_box);
    let bar = header.end.clone();
    let open = widgets::labeled_button("folder-symbolic", "Open video");
    open.add_css_class("header-action");
    open.set_valign(gtk::Align::Center);
    open.set_tooltip_text(Some("Open a video file (Ctrl+O)"));
    open.connect_clicked(|_| open_file());
    bar.append(&open);
    bar.append(&chrome::settings_button(settings::open));
    let frame = Rc::new(nyx_ui::frame::AppFrame::new(&header.root));
    frame.set_sidebar(&nav);
    frame.set_page(&content);
    // The frame's minimum width doesn't hold the window open: a tiled window gets the
    // compositor's width even while the wide layout is wider, and goes compact.
    let fit = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_width(true)
        .propagate_natural_height(true)
        .child(&frame.root)
        .build();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&fit));
    window.set_child(Some(&overlay));
    // A title's backdrop and the playing video reach the sheet's edges; every other page
    // keeps the room inside it.
    let f = frame.clone();
    stack.connect_visible_child_name_notify(move |s| {
        f.set_flush(matches!(s.visible_child_name().as_deref(), Some("details" | "playing")))
    });
    let probe = gtk::DrawingArea::new();
    probe.set_can_target(false);
    probe.set_can_focus(false);
    overlay.add_overlay(&probe);
    overlay.set_measure_overlay(&probe, false);
    crate::mini_player::build(&overlay, &player);
    probe.connect_resize(|_, _, _| {
        glib::idle_add_local_once(apply_width);
    });
    window.connect_default_width_notify(|_| apply_width());
    window.connect_realize(|_| apply_width());
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        if key == gdk::Key::space
            && !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK)
        {
            if typing() {
                return glib::Propagation::Proceed;
            }
            UI.with(|u| {
                if let Some(u) = u.borrow().as_ref()
                    && u.player.borrow().has_video()
                {
                    u.player.borrow().toggle();
                }
            });
            return glib::Propagation::Stop;
        }
        match key {
            gdk::Key::f | gdk::Key::F if ctrl => show_search(),
            gdk::Key::b | gdk::Key::B if ctrl => toggle_sidebar(),
            gdk::Key::o | gdk::Key::O if ctrl => open_file(),
            gdk::Key::F1 => show_shortcuts(),
            gdk::Key::Left if mods.contains(gdk::ModifierType::ALT_MASK) => back(),
            gdk::Key::Right if mods.contains(gdk::ModifierType::ALT_MASK) => forward(),
            gdk::Key::Escape if self::window().is_some_and(|w| w.is_fullscreen()) => leave_fullscreen(),
            // Full screen is for the player; with nothing loaded there's nothing to show.
            gdk::Key::f if !ctrl && !typing() && has_video() => toggle_fullscreen(),
            gdk::Key::m if !ctrl && !typing() => UI.with(|u| {
                if let Some(u) = u.borrow().as_ref() {
                    u.player.borrow().toggle_mute();
                }
            }),
            gdk::Key::Escape if settings_card::is_open() => settings_card::escape(),
            gdk::Key::Escape if search_focused() => {
                hide_search();
                drop_search_focus();
            }
            gdk::Key::Escape if UI.with(|u| u.borrow().as_ref().is_some_and(|u| !u.search.text().is_empty())) => {
                hide_search()
            }
            gdk::Key::Left | gdk::Key::Right if !ctrl => {
                let handled = UI.with(|u| {
                    if let Some(u) = u.borrow().as_ref() {
                        let focus = gtk::prelude::GtkWindowExt::focus(&u.window);
                        if focus.is_some_and(|w| {
                            w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.ancestor(gtk::Entry::static_type()).is_some()
                        }) {
                            return false;
                        }
                        if settings_card::is_open() {
                            return false;
                        }
                        if u.stack.visible_child_name().as_deref() == Some("playing") {
                            let p = u.player.borrow();
                            match key {
                                gdk::Key::Left => p.seek_by(-(prefs::get().seek_seconds as f64)),
                                _ => p.seek_by(prefs::get().seek_seconds as f64),
                            }
                            return true;
                        }
                    }
                    false
                });
                return if handled { glib::Propagation::Stop } else { glib::Propagation::Proceed };
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    let outside = gtk::GestureClick::new();
    outside.set_button(0);
    outside.set_propagation_phase(gtk::PropagationPhase::Capture);
    outside.connect_pressed(|g, _, x, y| {
        let Some(window) = self::window() else { return };
        // A popover (the field's own menu) is another surface; its clicks aren't outside.
        if g.current_event().and_then(|e| e.surface()) != window.surface() || !search_focused() {
            return;
        }
        let inside = UI.with(|u| {
            u.borrow().as_ref().is_some_and(|u| {
                window.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| in_search(&w, &u.search))
            })
        });
        if !inside {
            drop_search_focus();
        }
    });
    window.add_controller(outside);
    crate::navigation::install(&window);
    window.connect_fullscreened_notify(|w| {
        UI.with(|u| {
            if let Some(u) = u.borrow().as_ref() {
                let full = w.is_fullscreen();
                if full {
                    w.add_css_class("fullscreen");
                } else {
                    w.remove_css_class("fullscreen");
                }
                u.nav.set_visible(!full && u.stack.visible_child_name().as_deref() != Some("playing"));
                u.header.set_visible(!full);
            }
        });
    });
    let p = player.clone();
    window.connect_close_request(move |_| {
        CLOSING.with(|c| c.set(true));
        crate::inhibit::shutdown();
        crate::library::cancel();
        crate::playlists::cancel();
        crate::artwork::cancel();
        crate::servers::cancel();
        p.borrow_mut().shutdown();
        crate::playback::finish_reporting_on_close();
        prefs::flush();
        UI.with(|u| u.borrow_mut().take());
        glib::Propagation::Proceed
    });
    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            window: window.clone(),
            overlay,
            nav,
            stack,
            search,
            servers: server_rows,
            status_icon,
            status: readout,
            counts,
            playlists: playlist_rows,
            player,
            header: header.root.clone(),
            subtitle: header.subtitle.clone(),
            collapse,
            back: back_button,
            buttons,
        })
    });
    set_playlists(vec![]);
    update_servers();
    crate::playlists::load_saved();
    let p = prefs::get();
    let start = prefs::startup_page(&p).to_owned();
    navigate(&start);
    window.present();
    crate::servers::start();
    if prefs::take_broken() {
        status("Settings could not be read. Defaults are in use; a backup was kept.");
    }
    // The first database initialization stays off GTK, as future provider jobs will.
    nyx_core::cmd::background(
        || {
            nyx_media_cache::Cache::open(&nyx_core::paths::cache_dir().join("library.sqlite3"))
                .map(|_| ())
                .map_err(|_| "Metadata cache is unavailable")
        },
        |result| {
            if let Err(error) = result {
                status(error);
            }
        },
    );
}

/// Ctrl+F: into the search field.
fn show_search() {
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            u.search.grab_focus();
        }
    });
}
/// Search what the page you're on holds (movies, TV shows, playlists…), or everything.
fn set_search_origin() {
    if page() == "search" {
        return;
    }
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            let current = page();
            let origin = if matches!(current.as_str(), "home" | "discover" | "movies" | "tv" | "collections" | "playlists") {
                current
            } else { prefs::get().last_library_page };
            let origin = if origin.is_empty() { "home".into() } else { origin };
            u.search.set_placeholder_text(Some(match origin.as_str() {
                "movies" => "Search movies", "tv" => "Search TV shows", "collections" => "Search collections",
                "playlists" => "Search playlists or playlist videos",
                _ => "Search movies and shows",
            }));
            SEARCH_ORIGIN.with(|stored| *stored.borrow_mut() = origin);
        }
    });
}
fn hide_search() {
    SEARCH_CLEARING.with(|clearing| clearing.set(true));
    let origin = SEARCH_ORIGIN.with(|origin| origin.borrow().clone());
    UI.with(|u| if let Some(u) = u.borrow().as_ref() { u.search.set_text(""); });
    if origin == "playlists" { crate::playlists::search(""); }
    else { crate::library::search("", &origin); }
    SEARCH_CLEARING.with(|clearing| clearing.set(false));
    if page() == "search" { show_page(&origin); }
}
fn show_page(id: &str) {
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            let id = if u.stack.child_by_name(id).is_some() { id } else { "home" };
            u.stack.set_visible_child_name(id);
            u.nav.set_visible(id != "playing" && !u.window.is_fullscreen());
            u.collapse.set_visible(id != "playing");
            u.back.set_visible(matches!(id, "playing" | "details"));
            if id == "playing"
                && let Some(stage) = u.stack.visible_child()
            {
                stage.grab_focus();
            }
            let title = match id {
                "details" => "Details",
                "playing" => "Playback",
                "playlists" => "Playlists",
                _ => u.buttons.iter().find(|(page, _, _)| *page == id).map_or("Search", |(_, name, _)| name),
            };
            // The sidebar says where you are; the header keeps to the app's name.
            chrome::set_title(&u.window, "Theater", title);
            u.subtitle.set_visible(false);
        }
    });
    highlight();
    crate::library::show(id);
}
pub fn page() -> String {
    UI.with(|u| {
        u.borrow().as_ref().and_then(|u| u.stack.visible_child_name()).map(|s| s.to_string()).unwrap_or_else(|| "home".into())
    })
}
pub fn navigate(id: &str) {
    if id != "search" && (page() == "search" || UI.with(|u| u.borrow().as_ref().is_some_and(|u| !u.search.text().is_empty()))) { hide_search(); }
    NAV_FORWARD.with(|h| h.borrow_mut().clear());
    let previous = page();
    if previous != id {
        NAV_HISTORY.with(|h| {
            let mut h = h.borrow_mut();
            if h.last() != Some(&previous) {
                h.push(previous);
            }
            if h.len() > 64 {
                h.remove(0);
            }
        });
    }
    navigate_inner(id);
}
fn navigate_inner(id: &str) {
    if id != "playing" {
        crate::library::refresh_watch();
    }
    show_page(id);
    if id == "playlists" {
        crate::playlists::show();
    }
    prefs::update(|p| {
        p.last_section = id.into();
        if matches!(id, "home" | "movies" | "tv" | "collections" | "discover" | "playlists") {
            p.last_library_page = id.into();
        }
    });
}
pub fn back() {
    if settings_card::is_open() {
        settings_card::escape();
        return;
    }
    if page() == "search" {
        hide_search();
        return;
    }
    if page() == "details" {
        let item = crate::library::current();
        crate::library::back();
        NAV_FORWARD.with(|h| h.borrow_mut().push(("details".into(), item)));
        return;
    }
    if page() == "playlists" && crate::playlists::has_selection() {
        crate::playlists::close_selection();
        NAV_FORWARD.with(|h| h.borrow_mut().push(("playlist".into(), None)));
        return;
    }
    back_from_details();
}
pub fn back_from_details() {
    let current = page();
    let previous = NAV_HISTORY
        .with(|h| {
            let mut h = h.borrow_mut();
            while let Some(page) = h.pop() {
                if page != current {
                    return Some(page);
                }
            }
            None
        })
        .unwrap_or_else(|| "home".into());
    if current != "details" { NAV_FORWARD.with(|h| h.borrow_mut().push((current, None))); }
    navigate_inner(&previous);
}
pub fn forward() {
    if let Some((next, item)) = NAV_FORWARD.with(|h| h.borrow_mut().pop()) {
        // Reopening a playlist stays on the playlists page, so history doesn't change.
        if next == "playlist" {
            if page() == "playlists" {
                crate::playlists::reopen();
            }
            return;
        }
        let remaining = NAV_FORWARD.with(|h| h.borrow().clone());
        let current = page();
        NAV_HISTORY.with(|h| h.borrow_mut().push(current));
        if let Some(item) = item { crate::library::show_detail(item); }
        else { navigate_inner(&next); }
        NAV_FORWARD.with(|h| *h.borrow_mut() = remaining);
    }
}
fn toggle_sidebar() {
    prefs::update(|p| p.sidebar_collapsed = !p.sidebar_collapsed);
    apply_width();
}
/// The icon rail's own marks (a group's coloured bar) show only on the rail.
fn compact_show(widget: &gtk::Widget, compact: bool) {
    let mut child = widget.first_child();
    while let Some(c) = child {
        if c.has_css_class("compact-show") {
            c.set_visible(compact);
        }
        compact_show(&c, compact);
        child = c.next_sibling();
    }
}
fn apply_width() {
    let changed = UI.with(|u| {
        let u = u.borrow();
        let Some(u) = u.as_ref() else { return false };
        let width = if u.window.width() > 0 { u.window.width() } else { u.window.default_width() };
        let compact = width < 980 || prefs::get().sidebar_collapsed;
        compact != u.nav.has_css_class("compact") || !u.nav.has_css_class("sized")
    });
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            let width = if u.window.width() > 0 { u.window.width() } else { u.window.default_width() };
            let compact = width < 980 || prefs::get().sidebar_collapsed;
            u.nav.add_css_class("sized");
            nyx_ui::sidebar::set_compact(&u.nav, compact);
            compact_show(u.nav.upcast_ref(), compact);
            crate::library::set_narrow(width < 980);
            settings_card::fit(&u.window);
            // Existing page padding narrows with the allocated window size too.
            for page in u.stack.pages().iter::<gtk::StackPage>().flatten() {
                if let Some(body) = page.child().downcast_ref::<gtk::ScrolledWindow>().and_then(|s| s.child()) {
                    if width < 980 {
                        body.add_css_class("narrow");
                    } else {
                        body.remove_css_class("narrow");
                    }
                }
            }
        }
    });
    if changed {
        update_servers();
    }
}
pub fn update_motion() {
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            u.stack.set_transition_duration(if prefs::get().reduce_motion { 0 } else { 160 });
        }
    });
}
pub fn open_file() {
    crate::playlists::clear_queue();
    crate::library::clear_fallback();
    let Some(window) = window() else { return };
    let dialog = gtk::FileDialog::builder().title("Open video").build();
    dialog.open(Some(&window), None::<&gtk::gio::Cancellable>, |result| {
        if let Ok(file) = result {
            let result = UI.with(|u| u.borrow().as_ref().map(|u| u.player.borrow_mut().open(file.uri().as_str())));
            match result {
                Some(Ok(())) => {
                    status("Local playback");
                    navigate("playing");
                }
                Some(Err(error)) => status(&error),
                None => {}
            }
        }
    });
}
pub const SHORTCUTS: &[(&[&str], &str)] = &[
    (&["Ctrl", "O"], "Open video"),
    (&["Space"], "Play or pause"),
    (&["F"], "Fullscreen"),
    (&["M"], "Mute or unmute"),
    (&["←", "→"], "Seek by chosen jump interval"),
    (&["Ctrl", "F"], "Search library"),
    (&["Alt", "←"], "Back · or swipe right with two fingers"),
    (&["Alt", "→"], "Forward · or swipe left with two fingers"),
    (&["Esc"], "Clear or leave search, or close settings"),
    (&["Ctrl", "B"], "Collapse sidebar"),
    (&["Ctrl", ","], "Settings"),
    (&["Ctrl", "Shift", "P"], "Command palette"),
    (&["F1"], "Keyboard shortcuts"),
    (&["Ctrl", "Q"], "Close window"),
];
fn show_shortcuts() {
    let (dialog, card) = widgets::dialog("Keyboard shortcuts", 520);
    let list = widgets::vbox(0);
    list.add_css_class("group-list");
    for (keys, action) in SHORTCUTS {
        list.append(&widgets::row(action, "", Some(widgets::key_caps(keys).upcast_ref())));
    }
    card.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .propagate_natural_height(true)
            .max_content_height(560)
            .child(&list)
            .build(),
    );
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    card.append(&close);
    dialog.present();
}

fn search_focused() -> bool {
    UI.with(|u| {
        u.borrow().as_ref().is_some_and(|u| {
            gtk::prelude::GtkWindowExt::focus(&u.window).is_some_and(|w| in_search(&w, &u.search))
        })
    })
}
fn in_search(widget: &gtk::Widget, search: &gtk::SearchEntry) -> bool {
    widget == search.upcast_ref::<gtk::Widget>() || widget.is_ancestor(search)
}
/// Leave the search field; on the player page the player takes the keys again.
fn drop_search_focus() {
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            match u.stack.visible_child().filter(|_| u.stack.visible_child_name().as_deref() == Some("playing")) {
                Some(stage) => {
                    stage.grab_focus();
                }
                None => gtk::prelude::GtkWindowExt::set_focus(&u.window, None::<&gtk::Widget>),
            }
        }
    });
}
fn typing() -> bool {
    window()
        .and_then(|w| gtk::prelude::GtkWindowExt::focus(&w))
        .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.ancestor(gtk::Entry::static_type()).is_some())
}
/// What's playing: title, position and length in seconds, paused.
pub fn now_playing() -> Option<(String, f64, f64, bool)> {
    UI.with(|u| {
        let u = u.borrow();
        let p = u.as_ref()?.player.borrow();
        p.has_video().then(|| (p.title.clone(), p.position, p.duration(), p.paused))
    })
}

/// Work the player from outside the window (Nyx Assistant).
pub fn control(action: &str, seconds: f64) -> Result<(), String> {
    let player = UI.with(|u| u.borrow().as_ref().map(|u| u.player.clone())).ok_or("Theater isn't open")?;
    if !player.borrow().has_video() {
        return Err("nothing is playing".into());
    }
    match action {
        "pause" => player.borrow().set_string("pause", "yes"),
        "resume" => player.borrow().set_string("pause", "no"),
        "toggle" => player.borrow().toggle(),
        "forward" => player.borrow().seek_by(seconds),
        "back" => player.borrow().seek_by(-seconds),
        "stop" => {
            player.borrow_mut().close();
            back();
        }
        _ => return Err(format!("unknown action {action}")),
    }
    Ok(())
}

/// Play a local file or URI (from the command line or the file manager).
pub fn open_uri(uri: &str) {
    crate::playlists::clear_queue();
    crate::library::clear_fallback();
    let result = UI.with(|u| u.borrow().as_ref().map(|u| u.player.borrow_mut().open(uri)));
    match result {
        Some(Ok(())) => {
            status("Local playback");
            navigate("playing");
        }
        Some(Err(error)) => status(&error),
        None => {}
    }
}

fn has_video() -> bool {
    UI.with(|u| u.borrow().as_ref().is_some_and(|u| u.player.borrow().has_video()))
}

pub fn toggle_fullscreen() {
    let Some(w) = window() else { return };
    if w.is_fullscreen() {
        leave_fullscreen();
    } else {
        navigate("playing");
        w.fullscreen();
    }
}
pub fn leave_fullscreen() {
    if let Some(w) = window() {
        w.unfullscreen();
    }
}

pub fn play_remote(
    source: nyx_media_model::PlaybackSource,
    item: nyx_media_model::MediaItem,
    client: crate::servers::Client,
    resume_ms: u64,
    version: String,
    auto_transcode: bool,
    request_id: u64,
) {
    let player = UI.with(|u| u.borrow().as_ref().map(|u| u.player.clone()));
    let Some(player) = player else {
        client.discard_session(source.session_id, source.mode);
        return;
    };
    if !player.borrow().accepts_request(request_id) {
        client.discard_session(source.session_id, source.mode);
        return;
    }
    let mode = source.mode;
    let label = client.label();
    let result = player.borrow_mut().open_remote(source, item, client, resume_ms, version, auto_transcode);
    match result {
        Ok(()) => {
            status(&format!(
                "{label} · {}",
                if mode == nyx_media_model::PlaybackMode::Transcode { "Transcoding" } else { "Direct Play" }
            ));
            navigate("playing");
        }
        Err(e) => status(&e),
    }
}

pub fn begin_playback_request(server: nyx_media_model::ServerId) -> Option<u64> {
    if crate::reconcile::busy() {
        status("Wait for the watch-state update to finish before starting playback.");
        return None;
    }
    let player = UI.with(|u| u.borrow().as_ref().map(|u| u.player.clone()))?;
    let request = player.borrow_mut().begin_request(server);
    Some(request)
}

pub fn stop_server(server: &nyx_media_model::ServerId) {
    crate::library::clear_fallback();
    crate::playlists::clear_queue();
    crate::reconcile::cancel();
    let player = UI.with(|u| u.borrow().as_ref().map(|u| u.player.clone()));
    if let Some(player) = player {
        player.borrow_mut().cancel_server(server)
    }
}

pub fn accepts_playback_request(request: u64) -> bool {
    UI.with(|u| u.borrow().as_ref().is_some_and(|u| u.player.borrow().accepts_request(request)))
}

pub fn server_playing(server: &nyx_media_model::ServerId) -> bool {
    UI.with(|u| u.borrow().as_ref().is_some_and(|u| u.player.borrow().uses_server(server)))
}
