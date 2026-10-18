//! Live themes from utheme (macOS): the client side of utheme's client
//! protocol v1 (`docs/CLIENT-PROTOCOL.md` in the utheme repo).
//!
//! utheme points `ROOT/.local/share/utheme/current` at a new, never-modified
//! generation folder on every switch. A client resolves that link once, reads
//! `theme.json` and `palette.json` from the resolved folder, watches for the
//! link to change, and reports what it shows in a heartbeat file under
//! `ROOT/.local/state/utheme/clients/`. That heartbeat is the only file a
//! client ever writes.
//!
//! This file has no app types in it and is the same in ttyp and ctui, so it
//! can become a shared crate later. The apps compile it on macOS only.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

/// The protocol version this client speaks.
pub const PROTOCOL: u64 = 1;
/// The theme name that opts an app in (`theme = "utheme"`).
pub const THEME: &str = "utheme";
/// How often `current` is re-read in case a file system event was missed.
pub const POLL: Duration = Duration::from_secs(1);
/// How long to wait after an event before reading, so a burst reads once.
pub const DEBOUNCE: Duration = Duration::from_millis(50);

/// An sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `#rrggbb`, any case.
    pub fn parse(s: &str) -> Option<Rgb> {
        let hex = s.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// `a` blended `t` of the way toward `b`: per channel
/// `round(a·(1−t) + b·t)`, rounding half up (protocol section 7).
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |p: u8, q: u8| (f64::from(p) * (1.0 - t) + f64::from(q) * t + 0.5).floor() as u8;
    Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// Where utheme keeps its files, under `ROOT` (`$UTHEME_HOME` if set and
/// non-empty, else `$HOME`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `ROOT/.local/share/utheme`, the folder holding `current`.
    pub data: PathBuf,
    /// `ROOT/.local/share/utheme/current`.
    pub current: PathBuf,
    /// `ROOT/.local/state/utheme/clients`.
    pub clients: PathBuf,
}

impl Paths {
    /// From the environment; `None` when neither variable is set.
    pub fn from_env() -> Option<Paths> {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        var("UTHEME_HOME")
            .or_else(|| var("HOME"))
            .map(|root| Paths::under(Path::new(&root)))
    }

    pub fn under(root: &Path) -> Paths {
        let data = root.join(".local/share/utheme");
        Paths {
            current: data.join("current"),
            clients: root.join(".local/state/utheme/clients"),
            data,
        }
    }
}

/// One generation, read consistently from its resolved folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub generation: String,
    pub id: String,
    pub name: String,
    /// `"dark"` or `"light"`.
    pub mode: String,
    /// Every `#rrggbb` value in `palette.json`, by key.
    pub palette: HashMap<String, Rgb>,
}

impl Snapshot {
    /// A palette colour. Keys passed to `read` as needed are always there.
    pub fn color(&self, key: &str) -> Result<Rgb, String> {
        self.palette
            .get(key)
            .copied()
            .ok_or_else(|| format!("palette.json has no {key}"))
    }
}

/// Reads the generation `current` points at in `$UTHEME_HOME` / `$HOME`.
/// Fails if any key in `needs` is missing from the palette.
pub fn read_current(needs: &[&str]) -> Result<Snapshot, String> {
    let paths = Paths::from_env().ok_or("no HOME")?;
    match target(&paths) {
        Target::At(dir) => read_generation(&dir, needs),
        Target::Gone => Err("utheme isn't set up (no current theme)".into()),
        Target::Broken(e) => Err(e),
    }
}

/// Reads `theme.json` and `palette.json` from one generation folder.
pub fn read_generation(dir: &Path, needs: &[&str]) -> Result<Snapshot, String> {
    let json = |name: &str| -> Result<serde_json::Value, String> {
        let text = fs::read_to_string(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{name} unreadable: {e}"))
    };
    let theme = json("theme.json")?;
    let theme = theme.as_object().ok_or("theme.json isn't an object")?;
    // Files from before the protocol have no `protocol`; later ones only add keys.
    if let Some(p) = theme.get("protocol")
        && !p.as_u64().is_some_and(|p| p >= 1)
    {
        return Err(format!("theme.json: unsupported protocol {p}"));
    }
    let text = |k: &str| theme.get(k).and_then(|v| v.as_str()).map(str::to_string);

    let palette = json("palette.json")?;
    let palette = palette.as_object().ok_or("palette.json isn't an object")?;
    let mut colors = HashMap::new();
    for (k, v) in palette {
        if let Some(c) = v.as_str().and_then(Rgb::parse) {
            colors.insert(k.clone(), c);
        }
    }
    for k in needs {
        if !colors.contains_key(*k) {
            return Err(match palette.get(*k) {
                Some(_) => format!("palette.json: {k} isn't #rrggbb"),
                None => format!("palette.json has no {k}"),
            });
        }
    }

    let folder = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let generation = text("generation")
        .or_else(|| {
            fs::read_to_string(dir.join("generation"))
                .ok()
                .map(|g| g.trim().to_string())
        })
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| folder.clone());
    let id = text("id").unwrap_or(folder);
    let mode = text("mode")
        .or_else(|| {
            palette
                .get("mode")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "dark".into());
    Ok(Snapshot {
        generation,
        name: text("name").unwrap_or_else(|| id.clone()),
        id,
        mode,
        palette: colors,
    })
}

/// What `current` resolves to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    At(PathBuf),
    /// No `current` at all (utheme isn't set up, or was uninstalled).
    Gone,
    /// `current` exists but doesn't resolve to a folder.
    Broken(String),
}

fn target(paths: &Paths) -> Target {
    match fs::canonicalize(&paths.current) {
        Ok(dir) => Target::At(dir),
        Err(_) if fs::symlink_metadata(&paths.current).is_err() => Target::Gone,
        Err(e) => Target::Broken(format!("current is broken: {e}")),
    }
}

/// What following utheme produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// A new generation to show.
    Theme(Snapshot),
    /// The current generation couldn't be read; keep what you show.
    Error(String),
    /// `current` went away: fall back to your own theme.
    Gone,
}

fn update_for(t: &Target, needs: &[&str]) -> Update {
    match t {
        Target::At(dir) => match read_generation(dir, needs) {
            Ok(s) => Update::Theme(s),
            Err(e) => Update::Error(e),
        },
        Target::Gone => Update::Gone,
        Target::Broken(e) => Update::Error(e.clone()),
    }
}

enum Wake {
    Fs,
    Stop,
}

/// A thread that delivers an `Update` each time `current` resolves somewhere
/// new: on a file system event in the data folder (debounced) or, failing
/// that, on the next one-second poll. Dropping it stops the thread.
pub struct Watcher {
    rx: Receiver<Update>,
    wake: Sender<Wake>,
}

impl Watcher {
    /// Starts watching; `seen` is the target the caller already handled.
    fn start(paths: Paths, needs: &'static [&'static str], seen: Target) -> Watcher {
        let (tx, rx) = mpsc::channel();
        let (wake, wakes) = mpsc::channel();
        let fs_wake = wake.clone();
        // Without a thread there are no live updates, but nothing else breaks.
        let _ = thread::Builder::new()
            .name("utheme".into())
            .spawn(move || watch(paths, needs, seen, tx, fs_wake, wakes));
        Watcher { rx, wake }
    }

    /// The newest update waiting, if any (older ones are superseded).
    pub fn latest(&self) -> Option<Update> {
        self.rx.try_iter().last()
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.wake.send(Wake::Stop);
    }
}

fn watch(
    paths: Paths,
    needs: &'static [&'static str],
    mut seen: Target,
    tx: Sender<Update>,
    fs_wake: Sender<Wake>,
    wakes: Receiver<Wake>,
) {
    let mut fs_watch = None;
    loop {
        // (Re)attach the file system watch whenever the folder exists;
        // after an uninstall the old one may watch a deleted folder.
        if fs_watch.is_none() && paths.data.is_dir() {
            fs_watch = fs_watcher(&paths.data, fs_wake.clone());
        }
        match wakes.recv_timeout(POLL) {
            Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Wake::Fs) => {
                thread::sleep(DEBOUNCE);
                if wakes.try_iter().any(|w| matches!(w, Wake::Stop)) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = target(&paths);
        if now == seen {
            continue;
        }
        if now == Target::Gone {
            fs_watch = None;
        }
        if tx.send(update_for(&now, needs)).is_err() {
            return;
        }
        seen = now;
    }
}

fn fs_watcher(dir: &Path, wake: Sender<Wake>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher as _;
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = wake.send(Wake::Fs);
        }
    })
    .ok()?;
    w.watch(dir, notify::RecursiveMode::NonRecursive).ok()?;
    Some(w)
}

/// Following utheme for one app: the watcher plus the heartbeat. Dropping it
/// stops watching and removes the heartbeat, so quitting or choosing another
/// theme cleans up on every path.
pub struct Client {
    app: &'static str,
    version: &'static str,
    paths: Paths,
    watcher: Watcher,
    /// The generation on screen, `None` while falling back.
    showing: Option<String>,
}

impl Client {
    /// Reads the current generation (a few small files, so it can run before
    /// the first frame), writes the first heartbeat and starts watching.
    /// `None` only when there's no `$HOME` to look in.
    pub fn start(
        app: &'static str,
        version: &'static str,
        needs: &'static [&'static str],
    ) -> Option<(Client, Update)> {
        Some(Self::start_in(Paths::from_env()?, app, version, needs))
    }

    pub fn start_in(
        paths: Paths,
        app: &'static str,
        version: &'static str,
        needs: &'static [&'static str],
    ) -> (Client, Update) {
        let now = target(&paths);
        let first = update_for(&now, needs);
        let mut client = Client {
            app,
            version,
            watcher: Watcher::start(paths.clone(), needs, now),
            paths,
            showing: None,
        };
        client.report(&first);
        (client, first)
    }

    /// The newest update since the last call, heartbeat already written.
    pub fn poll(&mut self) -> Option<Update> {
        let u = self.watcher.latest()?;
        self.report(&u);
        Some(u)
    }

    fn report(&mut self, u: &Update) {
        let _ = match u {
            Update::Theme(s) => {
                self.showing = Some(s.generation.clone());
                heartbeat::write_in(
                    &self.paths,
                    self.app,
                    self.version,
                    Some(&s.generation),
                    None,
                )
            }
            Update::Error(e) => heartbeat::write_in(
                &self.paths,
                self.app,
                self.version,
                self.showing.as_deref(),
                Some(e),
            ),
            Update::Gone => {
                self.showing = None;
                heartbeat::remove_in(&self.paths, self.app);
                Ok(())
            }
        };
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        heartbeat::remove_in(&self.paths, self.app);
    }
}

/// `ROOT/.local/state/utheme/clients/<app>-<pid>.json` (protocol section 5).
pub mod heartbeat {
    use super::*;

    pub fn path(paths: &Paths, app: &str) -> PathBuf {
        paths
            .clients
            .join(format!("{app}-{}.json", std::process::id()))
    }

    /// Writes this process's heartbeat in `$UTHEME_HOME` / `$HOME`.
    pub fn write(
        app: &str,
        version: &str,
        generation: Option<&str>,
        error: Option<&str>,
    ) -> io::Result<()> {
        let paths = Paths::from_env().ok_or_else(|| io::Error::other("no HOME"))?;
        write_in(&paths, app, version, generation, error)
    }

    /// Writes atomically: a `.tmp` file in the same folder, then a rename.
    pub fn write_in(
        paths: &Paths,
        app: &str,
        version: &str,
        generation: Option<&str>,
        error: Option<&str>,
    ) -> io::Result<()> {
        let mut beat = serde_json::json!({
            "protocol": PROTOCOL,
            "app": app,
            "version": version,
            "pid": std::process::id(),
            "generation": generation,
        });
        if let Some(e) = error {
            beat["error"] = e.into();
        }
        fs::create_dir_all(&paths.clients)?;
        let path = path(paths, app);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, beat.to_string() + "\n")?;
        fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// Removes this process's heartbeat in `$UTHEME_HOME` / `$HOME`.
    pub fn remove(app: &str) {
        if let Some(paths) = Paths::from_env() {
            remove_in(&paths, app);
        }
    }

    pub fn remove_in(paths: &Paths, app: &str) {
        let _ = fs::remove_file(path(paths, app));
    }
}

/// Fixtures shaped like utheme's own files, for the apps' tests too.
#[cfg(test)]
pub mod testing {
    use super::*;

    /// A generation folder like utheme writes, with `bg` as background.
    pub fn generation(paths: &Paths, gen_id: &str, bg: &str) -> PathBuf {
        let dir = paths.data.join("generations").join(gen_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("theme.json"),
            format!(
                r#"{{"directory": "/x", "generation": "{gen_id}", "id": "nord", "mode": "dark", "name": "Nord", "protocol": 1}}"#
            ),
        )
        .unwrap();
        fs::write(
            dir.join("palette.json"),
            format!(
                r##"{{"accent": "#88C0D0", "background": "{bg}", "foreground": "#d8dee9", "mode": "dark", "muted": "#4c566a", "red": "#bf616a", "orange": "#d08770", "yellow": "#ebcb8b", "green": "#a3be8c", "cyan": "#8fbcbb", "blue": "#81a1c1", "magenta": "#b48ead", "bg": "{bg}"}}"##
            ),
        )
        .unwrap();
        fs::write(dir.join("generation"), format!("{gen_id}\n")).unwrap();
        dir
    }

    /// Points `current` at `dir` the way utheme does: a new link, renamed over.
    pub fn switch(paths: &Paths, dir: &Path) {
        let tmp = paths.data.join(".current-tmp");
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(dir, &tmp).unwrap();
        fs::rename(&tmp, &paths.current).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{generation, switch};
    use super::*;
    use std::time::Instant;

    const NEEDS: &[&str] = &["background", "foreground", "accent", "muted", "red"];

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "utheme-client-{name}-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mix_matches_the_spec() {
        let red = Rgb(0xff, 0, 0);
        assert_eq!(mix(red, Rgb(0, 0, 0), 0.35), Rgb(0xa6, 0, 0));
        assert_eq!(mix(red, Rgb(0, 0, 0), 0.35).hex(), "#a60000");
        assert_eq!(
            mix(Rgb(1, 0, 0), Rgb(0, 0, 0), 0.5),
            Rgb(1, 0, 0),
            "half rounds up"
        );
        assert_eq!(mix(red, Rgb(0, 0, 255), 0.0), red);
        assert_eq!(mix(red, Rgb(0, 0, 255), 1.0), Rgb(0, 0, 255));
    }

    #[test]
    fn colours_parse_in_any_case() {
        assert_eq!(Rgb::parse("#88C0D0"), Some(Rgb(0x88, 0xc0, 0xd0)));
        assert_eq!(Rgb::parse("#88c0d0"), Some(Rgb(0x88, 0xc0, 0xd0)));
        for bad in ["88c0d0", "#fff", "#gggggg", "#88c0d0ff", "dark", "#+1+2+3"] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn reads_a_generation() {
        let root = scratch("read");
        let paths = Paths::under(&root);
        let dir = generation(&paths, "20261018-123921-077-18ee-nord", "#2e3440");
        let s = read_generation(&dir, NEEDS).unwrap();
        assert_eq!(s.generation, "20261018-123921-077-18ee-nord");
        assert_eq!(
            (s.id.as_str(), s.name.as_str(), s.mode.as_str()),
            ("nord", "Nord", "dark")
        );
        assert_eq!(s.color("accent"), Ok(Rgb(0x88, 0xc0, 0xd0)));
        assert_eq!(s.color("background"), Ok(Rgb(0x2e, 0x34, 0x40)));
        assert!(!s.palette.contains_key("mode"), "not a colour");
        assert!(s.color("brown").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_and_newer_theme_files_are_accepted() {
        let root = scratch("versions");
        let paths = Paths::under(&root);
        let dir = generation(&paths, "g1", "#000000");
        // Before the protocol: no `protocol`, no `generation` key.
        fs::write(
            dir.join("theme.json"),
            r#"{"id": "nord", "name": "Nord", "mode": "dark"}"#,
        )
        .unwrap();
        assert_eq!(read_generation(&dir, NEEDS).unwrap().generation, "g1");
        fs::write(
            dir.join("theme.json"),
            r#"{"protocol": 7, "id": "nord", "name": "Nord", "mode": "light", "extra": [1]}"#,
        )
        .unwrap();
        assert_eq!(read_generation(&dir, NEEDS).unwrap().mode, "light");
        fs::write(dir.join("theme.json"), r#"{"protocol": 0, "id": "nord"}"#).unwrap();
        assert!(read_generation(&dir, NEEDS).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        let root = scratch("bad");
        let paths = Paths::under(&root);
        let dir = generation(&paths, "g1", "#000000");
        let err = |palette: &str| {
            fs::write(dir.join("palette.json"), palette).unwrap();
            read_generation(&dir, NEEDS).unwrap_err()
        };
        assert!(err(r##"{"background": "#000000"}"##).contains("has no foreground"));
        let bad = r##"{"background": "#00000", "foreground": "#ffffff", "accent": "#ffffff", "muted": "#ffffff", "red": "#ffffff"}"##;
        assert!(err(bad).contains("background isn't #rrggbb"));
        assert!(err(r#"{"background": 3"#).contains("unreadable"));
        assert!(err("[]").contains("isn't an object"));
        fs::remove_file(dir.join("palette.json")).unwrap();
        assert!(
            read_generation(&dir, NEEDS)
                .unwrap_err()
                .starts_with("palette.json")
        );
        fs::write(dir.join("theme.json"), "not json").unwrap();
        assert!(
            read_generation(&dir, NEEDS)
                .unwrap_err()
                .starts_with("theme.json")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_follow_utheme_home() {
        let p = Paths::under(Path::new("/r"));
        assert_eq!(p.current, Path::new("/r/.local/share/utheme/current"));
        assert_eq!(p.clients, Path::new("/r/.local/state/utheme/clients"));
    }

    #[test]
    fn heartbeat_is_written_and_removed() {
        let root = scratch("beat");
        let paths = Paths::under(&root);
        heartbeat::write_in(&paths, "ttyp", "1.2.3", Some("g1"), None).unwrap();
        let file = heartbeat::path(&paths, "ttyp");
        let beat: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(beat["protocol"], 1);
        assert_eq!(beat["app"], "ttyp");
        assert_eq!(beat["version"], "1.2.3");
        assert_eq!(beat["pid"], std::process::id());
        assert_eq!(beat["generation"], "g1");
        assert!(beat.get("error").is_none());
        heartbeat::write_in(
            &paths,
            "ttyp",
            "1.2.3",
            None,
            Some("palette.json unreadable"),
        )
        .unwrap();
        let beat: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert!(beat["generation"].is_null());
        assert_eq!(beat["error"], "palette.json unreadable");
        let names: Vec<_> = fs::read_dir(&paths.clients).unwrap().flatten().collect();
        assert_eq!(names.len(), 1, "no .tmp left behind");
        heartbeat::remove_in(&paths, "ttyp");
        assert!(!file.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn wait_for(client: &mut Client, mut done: impl FnMut(&Update) -> bool) -> Update {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(u) = client.poll()
                && done(&u)
            {
                return u;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("no update within 5 s");
    }

    #[test]
    fn follows_current_through_switches() {
        let root = scratch("follow");
        let paths = Paths::under(&root);
        let g1 = generation(&paths, "g1", "#111111");
        let g2 = generation(&paths, "g2", "#222222");
        switch(&paths, &g1);
        let beat = heartbeat::path(&paths, "app");

        let (mut client, first) = Client::start_in(paths.clone(), "app", "0.1.0", NEEDS);
        let Update::Theme(s) = first else {
            panic!("{first:?}")
        };
        assert_eq!(s.generation, "g1");
        assert!(fs::read_to_string(&beat).unwrap().contains("\"g1\""));

        // The target is replaced mid-run; the new one arrives within a second or so.
        let at = Instant::now();
        switch(&paths, &g2);
        let u = wait_for(&mut client, |u| matches!(u, Update::Theme(_)));
        assert_eq!(u, Update::Theme(read_generation(&g2, NEEDS).unwrap()));
        assert!(
            at.elapsed() < Duration::from_millis(1500),
            "{:?}",
            at.elapsed()
        );
        assert!(fs::read_to_string(&beat).unwrap().contains("\"g2\""));

        // A broken generation keeps what's shown and reports why.
        let g3 = generation(&paths, "g3", "#333333");
        fs::write(g3.join("palette.json"), "{").unwrap();
        switch(&paths, &g3);
        let u = wait_for(&mut client, |u| matches!(u, Update::Error(_)));
        assert!(matches!(u, Update::Error(e) if e.contains("palette.json")));
        let text = fs::read_to_string(&beat).unwrap();
        assert!(text.contains("\"g2\"") && text.contains("error"), "{text}");

        // Uninstalled: fall back, heartbeat gone. Back again: followed again.
        fs::remove_file(&paths.current).unwrap();
        assert_eq!(wait_for(&mut client, |_| true), Update::Gone);
        assert!(!beat.exists());
        switch(&paths, &g1);
        assert!(
            matches!(wait_for(&mut client, |_| true), Update::Theme(s) if s.generation == "g1")
        );

        drop(client);
        assert!(!beat.exists(), "dropping the client removes its heartbeat");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rapid_switches_end_on_the_last() {
        let root = scratch("rapid");
        let paths = Paths::under(&root);
        let gens: Vec<_> = (0..10)
            .map(|i| generation(&paths, &format!("g{i}"), &format!("#0000{i:02}")))
            .collect();
        switch(&paths, &gens[0]);
        let (mut client, _) = Client::start_in(paths.clone(), "app", "0.1.0", NEEDS);
        for g in &gens[1..] {
            switch(&paths, g);
        }
        let mut last = None;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if let Some(Update::Theme(s)) = client.poll() {
                last = Some(s.generation);
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(last.as_deref(), Some("g9"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_setup_starts_cleanly() {
        let root = scratch("missing");
        let paths = Paths::under(&root);
        let (client, first) = Client::start_in(paths.clone(), "app", "0.1.0", NEEDS);
        assert_eq!(first, Update::Gone);
        assert!(
            !paths.clients.exists(),
            "nothing written while falling back"
        );
        drop(client);
        fs::remove_dir_all(root).unwrap();
    }
}
