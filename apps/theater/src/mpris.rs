//! MPRIS (org.mpris.MediaPlayer2): media keys, `playerctl`, the bar, Control Center and
//! the dock. As in Media Player, the D-Bus side runs on its own thread with zbus'
//! blocking API, reads a snapshot the GTK thread keeps current, and sends commands back
//! over a channel. The GTK thread refreshes the snapshot each second while Theater runs,
//! and right after each command, and asks for change signals when something differs.

use crate::{playlists, window};
use gtk::{glib, prelude::*};
use nyx_media_model::MediaKind;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";
pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.nyx_theater";
const DESKTOP_ENTRY: &str = "io.github.design_nexus.nyx.Theater";
/// A position this far from where playback should be by now was a seek.
const SEEK_JUMP_US: i64 = 1_500_000;

#[derive(Debug, Clone)]
enum Command {
    Raise,
    Quit,
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative, in microseconds.
    Seek(i64),
    /// Absolute, in microseconds, for the given track id.
    SetPosition(String, i64),
    OpenUri(String),
    Volume(f64),
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Meta {
    track_id: String,
    length_us: i64,
    title: String,
    show: String,
    episode: Option<u32>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    status: &'static str,
    volume: f64,
    position_us: i64,
    at: Instant,
    can_next: bool,
    can_previous: bool,
    meta: Option<Meta>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            status: "Stopped",
            volume: 1.0,
            position_us: 0,
            at: Instant::now(),
            can_next: false,
            can_previous: false,
            meta: None,
        }
    }
}

impl Snapshot {
    /// Where playback is now, counting from when the snapshot was taken.
    fn position(&self) -> i64 {
        if self.status == "Playing" { self.position_us + self.at.elapsed().as_micros() as i64 } else { self.position_us }
    }

    fn has_track(&self) -> bool {
        self.meta.is_some()
    }

    fn can_seek(&self) -> bool {
        self.meta.as_ref().is_some_and(|m| m.length_us > 0)
    }
}

type Shared = Arc<Mutex<Snapshot>>;

enum Signal {
    Changed(Vec<&'static str>),
    Seeked(i64),
}

fn track_id(title: &str, show: &str, episode: Option<u32>) -> String {
    let hash = format!("{:x}", Sha256::digest(format!("{show}\0{title}\0{episode:?}")));
    format!("/io/github/design_nexus/nyx/Theater/title/{}", &hash[..16])
}

fn value(v: Value<'_>) -> Option<OwnedValue> {
    v.try_to_owned().ok()
}

fn metadata(meta: &Option<Meta>) -> HashMap<String, OwnedValue> {
    let mut m = HashMap::new();
    let mut put = |k: &str, v: Value<'_>| {
        if let Some(v) = value(v) {
            m.insert(k.to_string(), v);
        }
    };
    let Some(t) = meta else {
        if let Ok(p) = ObjectPath::try_from("/org/mpris/MediaPlayer2/TrackList/NoTrack") {
            put("mpris:trackid", Value::from(p));
        }
        return m;
    };
    if let Ok(p) = ObjectPath::try_from(t.track_id.as_str()) {
        put("mpris:trackid", Value::from(p));
    }
    if t.length_us > 0 {
        put("mpris:length", Value::from(t.length_us));
    }
    put("xesam:title", Value::from(t.title.as_str()));
    if !t.show.is_empty() {
        put("xesam:artist", Value::from(vec![t.show.as_str()]));
        put("xesam:album", Value::from(t.show.as_str()));
    }
    if let Some(n) = t.episode {
        put("xesam:trackNumber", Value::from(n as i32));
    }
    m
}

struct Root {
    tx: async_channel::Sender<Command>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        let _ = self.tx.send_blocking(Command::Raise);
    }

    fn quit(&self) {
        let _ = self.tx.send_blocking(Command::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "Theater".into()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        DESKTOP_ENTRY.into()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        ["file", "http", "https"].iter().map(|s| s.to_string()).collect()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        ["video/mp4", "video/x-matroska", "video/webm", "video/quicktime", "video/x-msvideo", "video/mpeg", "video/ogg"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

struct Player {
    tx: async_channel::Sender<Command>,
    state: Shared,
}

impl Player {
    fn snap(&self) -> Snapshot {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn send(&self, c: Command) {
        let _ = self.tx.send_blocking(c);
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.send(Command::Next);
    }

    fn previous(&self) {
        self.send(Command::Previous);
    }

    fn pause(&self) {
        self.send(Command::Pause);
    }

    fn play_pause(&self) {
        self.send(Command::PlayPause);
    }

    fn stop(&self) {
        self.send(Command::Stop);
    }

    fn play(&self) {
        self.send(Command::Play);
    }

    fn seek(&self, offset: i64) {
        self.send(Command::Seek(offset));
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        self.send(Command::SetPosition(track_id.to_string(), position));
    }

    fn open_uri(&self, uri: String) {
        self.send(Command::OpenUri(uri));
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.snap().status.into()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    /// Theater plays at normal speed only (MinimumRate = MaximumRate = 1).
    #[zbus(property)]
    fn set_rate(&mut self, _value: f64) {}

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(&self.snap().meta)
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.snap().volume
    }

    #[zbus(property)]
    fn set_volume(&mut self, value: f64) {
        self.send(Command::Volume(value));
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.snap().position()
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.snap().can_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.snap().can_previous
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.snap().has_track()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.snap().has_track()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.snap().can_seek()
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

fn serve(name: String, state: Shared, tx: async_channel::Sender<Command>, signals: mpsc::Receiver<Signal>) -> zbus::Result<()> {
    let conn = zbus::blocking::connection::Builder::session()?
        .name(name)?
        .serve_at(PATH, Root { tx: tx.clone() })?
        .serve_at(PATH, Player { tx, state })?
        .build()?;
    let iface = conn.object_server().interface::<_, Player>(PATH)?;
    for sig in signals {
        let emitter = iface.signal_emitter();
        let guard = iface.get();
        let r = zbus::block_on(async {
            match sig {
                Signal::Seeked(pos) => Player::seeked(emitter, pos).await,
                Signal::Changed(props) => {
                    for p in props {
                        match p {
                            "PlaybackStatus" => guard.playback_status_changed(emitter).await?,
                            "Metadata" => guard.metadata_changed(emitter).await?,
                            "Volume" => guard.volume_changed(emitter).await?,
                            "CanGoNext" => guard.can_go_next_changed(emitter).await?,
                            "CanGoPrevious" => guard.can_go_previous_changed(emitter).await?,
                            "CanPlay" => guard.can_play_changed(emitter).await?,
                            "CanPause" => guard.can_pause_changed(emitter).await?,
                            "CanSeek" => guard.can_seek_changed(emitter).await?,
                            _ => {}
                        }
                    }
                    Ok(())
                }
            }
        });
        drop(guard);
        if let Err(e) = r {
            eprintln!("nyx-theater: mpris: {e}");
        }
    }
    Ok(())
}

fn snapshot() -> Snapshot {
    let settings = crate::prefs::get();
    let volume = if settings.muted { 0.0 } else { settings.volume };
    let Some(player) = window::player() else {
        return Snapshot { volume, ..Snapshot::default() };
    };
    let Ok(p) = player.try_borrow() else {
        return Snapshot { volume, ..Snapshot::default() };
    };
    if !p.has_video() {
        return Snapshot { volume, ..Snapshot::default() };
    }
    let episode = p.media().filter(|m| m.kind == MediaKind::Episode);
    let show = episode.and_then(|m| m.tv.series_title.clone()).unwrap_or_default();
    let number = episode.and_then(|m| m.episode_number);
    let title = p.title.clone();
    Snapshot {
        status: if p.paused { "Paused" } else { "Playing" },
        volume,
        position_us: (p.position * 1e6) as i64,
        at: Instant::now(),
        can_next: playlists::can_step(1),
        can_previous: playlists::can_step(-1),
        meta: Some(Meta {
            track_id: track_id(&title, &show, number),
            length_us: (p.duration() * 1e6) as i64,
            title,
            show,
            episode: number,
        }),
    }
}

/// What changed between two snapshots, by MPRIS property name, and whether playback
/// jumped (a seek) rather than ran on.
fn changes(prev: &Snapshot, now: &Snapshot) -> (Vec<&'static str>, bool) {
    let mut changed = Vec::new();
    if prev.status != now.status {
        changed.push("PlaybackStatus");
    }
    if (prev.volume - now.volume).abs() > 1e-6 {
        changed.push("Volume");
    }
    if prev.can_next != now.can_next {
        changed.push("CanGoNext");
    }
    if prev.can_previous != now.can_previous {
        changed.push("CanGoPrevious");
    }
    if prev.has_track() != now.has_track() {
        changed.extend(["CanPlay", "CanPause"]);
    }
    if prev.can_seek() != now.can_seek() {
        changed.push("CanSeek");
    }
    if prev.meta != now.meta {
        changed.push("Metadata");
    }
    let same_title = prev.meta.as_ref().map(|m| &m.track_id) == now.meta.as_ref().map(|m| &m.track_id);
    let jumped = same_title && now.has_track() && (now.position_us - prev.position()).abs() > SEEK_JUMP_US;
    (changed, jumped)
}

fn handle(app: &gtk::Application, c: Command) {
    let control = |action: &str, seconds: f64| {
        let _ = window::control(action, seconds);
    };
    match c {
        Command::Raise => {
            if let Some(w) = window::window() {
                w.present();
            }
        }
        Command::Quit => app.quit(),
        Command::Play => control("resume", 0.0),
        Command::Pause => control("pause", 0.0),
        Command::PlayPause => control("toggle", 0.0),
        Command::Stop => control("stop", 0.0),
        Command::Next => {
            if playlists::can_step(1) {
                playlists::advance(1);
            }
        }
        Command::Previous => {
            if playlists::can_step(-1) {
                playlists::advance(-1);
            }
        }
        Command::Seek(us) => {
            if let Some(p) = window::player() {
                p.borrow().seek_by(us as f64 / 1e6);
            }
        }
        Command::SetPosition(id, us) => {
            let now = snapshot();
            if let (Some(meta), Some(p)) = (now.meta, window::player())
                && meta.track_id == id
                && us >= 0
                && us <= meta.length_us
            {
                p.borrow().seek(us as f64 / 1e6);
            }
        }
        Command::OpenUri(uri) => {
            if uri.starts_with("file://") || uri.starts_with("http://") || uri.starts_with("https://") {
                window::open_uri(&uri);
            }
        }
        Command::Volume(v) => {
            if let Some(p) = window::player() {
                p.borrow().set_volume(v);
            }
        }
    }
}

/// Register on the session bus (the primary instance; a test copy under its own app id
/// takes an `.instance<pid>` name). Failures (no session bus, name taken) are reported
/// once and otherwise ignored; Theater works without MPRIS.
pub fn start(app: &gtk::Application) {
    let name = if std::env::var_os("THEATER_APP_ID").is_some() {
        format!("{BUS_NAME}.instance{}", std::process::id())
    } else {
        BUS_NAME.to_string()
    };
    let shared: Shared = Arc::new(Mutex::new(snapshot()));
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<Command>();
    let (sig_tx, sig_rx) = mpsc::channel::<Signal>();
    {
        let shared = shared.clone();
        std::thread::Builder::new()
            .name("mpris".into())
            .spawn(move || {
                if let Err(e) = serve(name, shared, cmd_tx, sig_rx) {
                    eprintln!("nyx-theater: MPRIS unavailable: {e}");
                }
            })
            .ok();
    }

    let last = std::rc::Rc::new(std::cell::RefCell::new(snapshot()));
    let refresh = std::rc::Rc::new(move || {
        let now = snapshot();
        let (changed, jumped) = changes(&last.borrow(), &now);
        if let Ok(mut s) = shared.lock() {
            *s = now.clone();
        }
        let position = now.position_us;
        *last.borrow_mut() = now;
        if !changed.is_empty() {
            let _ = sig_tx.send(Signal::Changed(changed));
        }
        if jumped {
            let _ = sig_tx.send(Signal::Seeked(position));
        }
    });

    let app = app.clone();
    let after_command = refresh.clone();
    glib::spawn_future_local(async move {
        while let Ok(c) = cmd_rx.recv().await {
            handle(&app, c);
            // mpv reports a pause or seek on its next event; look again shortly after.
            let after = after_command.clone();
            glib::timeout_add_local_once(Duration::from_millis(150), move || after());
        }
    });
    glib::timeout_add_local(Duration::from_secs(1), move || {
        refresh();
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(title: &str, position_s: f64) -> Snapshot {
        Snapshot {
            status: "Playing",
            position_us: (position_s * 1e6) as i64,
            meta: Some(Meta {
                track_id: track_id(title, "", None),
                length_us: 3_600_000_000,
                title: title.into(),
                ..Meta::default()
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn track_ids_are_object_paths() {
        let id = track_id("Movie: The Sequel (2024) – ünïcode", "Show", Some(3));
        assert!(ObjectPath::try_from(id.as_str()).is_ok(), "{id}");
        assert_ne!(id, track_id("Movie: The Sequel (2024) – ünïcode", "Show", Some(4)));
    }

    #[test]
    fn running_on_is_not_a_seek_but_a_jump_is() {
        let prev = playing("A", 10.0);
        let (changed, jumped) = changes(&prev, &playing("A", 10.0));
        assert!(changed.is_empty() && !jumped);
        let (_, jumped) = changes(&prev, &playing("A", 95.0));
        assert!(jumped);
    }

    #[test]
    fn a_new_title_changes_metadata_without_a_seek() {
        let (changed, jumped) = changes(&playing("A", 10.0), &playing("B", 0.0));
        assert!(changed.contains(&"Metadata") && !jumped);
    }

    #[test]
    fn stopping_clears_the_controls() {
        let (changed, _) = changes(&playing("A", 10.0), &Snapshot::default());
        for p in ["PlaybackStatus", "Metadata", "CanPlay", "CanPause", "CanSeek"] {
            assert!(changed.contains(&p), "{p}");
        }
    }
}
