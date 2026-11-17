mod artwork;
mod browse;
mod browse_ui;
mod catalog;
mod control;
mod credentials;
mod dialog_dismissal;
mod inhibit;
mod jellyfin;
mod library;
mod metadata;
mod mini_player;
mod mpris;
mod navigation;
mod now_playing;
mod playback;
mod playlist_store;
mod playlists;
mod plex;
mod prefs;
mod presentation;
mod rating_icons;
mod rating_index;
mod reconcile;
mod rich_details;
mod servers;
mod settings;
mod shelf;
mod sync;
mod theme;
mod view_cache;
mod widgets;
mod window;

use gtk::{gio, glib, prelude::*};

/// Hand freed heap back to the system after a large library rebuild; glibc otherwise
/// keeps the high-water mark of every arena.
pub fn trim_heap() {
    // SAFETY: malloc_trim only releases free pages; it's safe from any thread.
    unsafe { libc::malloc_trim(0) };
}

const APP_ID: &str = "io.github.design_nexus.nyx.Theater";

fn main() -> glib::ExitCode {
    // Every worker thread would otherwise get its own 64 MiB glibc arena, and the
    // library's churn leaves each one fragmented and full.
    // SAFETY: called before any other thread exists.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    nyx_core::app::init(nyx_core::app::App { slug: "nyx-theater", name: "Theater" });
    nyx_core::control::describe_if_asked(&control::description(
        &std::env::var("THEATER_APP_ID").unwrap_or_else(|_| APP_ID.to_string()),
    ));
    if prefs::get().client_id.is_empty() {
        prefs::update(|p| p.client_id = uuid::Uuid::new_v4().to_string());
        prefs::flush();
    }
    // Keep the same GL renderer choice as Media Player on hybrid GPU laptops.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: no threads or GTK have started yet.
        unsafe { std::env::set_var("GSK_RENDERER", "gl") };
    }
    let mut args: Vec<String> = std::env::args().collect();
    // Started by D-Bus for an action: GLib leaves this flag to apps that handle their own
    // command line.
    let service = args.iter().any(|a| a == "--gapplication-service");
    args.retain(|a| a != "--gapplication-service");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: nyx-theater [--section home|discover|movies|tv|collections|playlists|settings] [--toggle] [FILE|URI …]");
        return glib::ExitCode::SUCCESS;
    }
    // A test copy runs under its own id, apart from the instance in use.
    let id = std::env::var("THEATER_APP_ID").unwrap_or_else(|_| APP_ID.to_string());
    let app = gtk::Application::builder()
        .application_id(id)
        .flags(if service {
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::IS_SERVICE
        } else {
            gio::ApplicationFlags::HANDLES_COMMAND_LINE
        })
        .build();
    // Nyx Assistant finds and plays titles through this (primary instance only).
    // The nyx command, Glimpse and Nyx Assistant reach it through this (primary instance
    // only); a call to a closed Theater opens its window, which holds the servers.
    let described = control::description(&app.application_id().map(|s| s.to_string()).unwrap_or_default());
    app.connect_startup(move |app| {
        let handle_app = app.clone();
        nyx_core::control::serve(app, described.clone(), move |name, args| {
            if handle_app.active_window().is_none() {
                theme::install();
                window::build(&handle_app);
            }
            control::handle(name, args)
        })
    });
    // Media keys, playerctl, the bar, Control Center and the dock.
    app.connect_startup(mpris::start);
    app.connect_command_line(|app, cl| {
        let argv: Vec<String> = cl.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        if argv.iter().any(|a| a == "--toggle")
            && let Some(w) = app.active_window()
        {
            w.close();
            return glib::ExitCode::SUCCESS;
        }
        let fresh = app.active_window().is_none();
        if fresh {
            theme::install();
            window::build(app);
        } else if let Some(w) = app.active_window() {
            w.present();
        }
        let section = argv.iter().position(|a| a == "--section").and_then(|i| argv.get(i + 1)).cloned();
        match section.as_deref() {
            Some("settings") => settings::open(),
            Some(page) => window::navigate(page),
            None => {}
        }
        // The first file or URI plays (one video at a time); flags and --section's value aren't files.
        let mut files = Vec::new();
        let mut rest = argv.iter();
        while let Some(a) = rest.next() {
            if a == "--section" {
                rest.next();
            } else if !a.starts_with("--") {
                files.push(a);
            }
        }
        if let Some(first) = files.first() {
            let file = cl.create_file_for_arg(first);
            window::open_uri(file.uri().as_str());
        }
        glib::ExitCode::SUCCESS
    });
    app.connect_shutdown(|_| prefs::flush());
    app.run_with_args(&args)
}
