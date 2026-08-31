//! Loads [`SceneDef`]s from `.viz.ron` files and watches for changes.
//!
//! Polled from the render loop rather than using a filesystem watcher: the
//! visualizer already runs a per-frame poll loop, and re-listing a handful of
//! files a couple of times a second is cheap enough that a dedicated watcher
//! thread and channel would only add moving parts.
//!
//! A reload that yields no valid scenes never clears the current set — a
//! typo while editing a file must not blank the picker.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::scene::SceneDef;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const SUFFIX: &str = ".viz.ron";

pub struct SceneLibrary {
    dir: PathBuf,
    fallback: SceneDef,
    defs: Vec<SceneDef>,
    signature: u64,
    last_poll: Instant,
}

impl SceneLibrary {
    /// `fallback` seeds the library when `dir` has no valid scene files.
    pub fn new(dir: PathBuf, fallback: SceneDef) -> Self {
        let mut lib = Self {
            dir,
            fallback,
            defs: Vec::new(),
            // Deliberately distinct from any real scan result, so the first
            // `reload` always runs regardless of what it finds.
            signature: u64::MAX,
            last_poll: Instant::now(),
        };
        lib.reload();
        lib
    }

    /// Re-scans the directory at most once per [`POLL_INTERVAL`]. Returns
    /// `true` if the active scene set changed.
    pub fn poll(&mut self) -> bool {
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return false;
        }
        self.last_poll = Instant::now();
        self.reload()
    }

    pub fn defs(&self) -> &[SceneDef] {
        &self.defs
    }

    fn reload(&mut self) -> bool {
        let (defs, signature) = scan(&self.dir);
        if signature == self.signature {
            return false;
        }
        self.signature = signature;

        if defs.is_empty() {
            if self.defs.is_empty() {
                log::info!(
                    "viz: no scene files in {}; using built-in default",
                    self.dir.display()
                );
                self.defs = vec![self.fallback.clone()];
                return true;
            }
            log::warn!(
                "viz: reload of {} produced no valid scenes; keeping previous set",
                self.dir.display()
            );
            return false;
        }

        self.defs = defs;
        true
    }
}

/// Parses every `*.viz.ron` file in `dir`, skipping and logging unreadable or
/// malformed ones, and returns them alongside a content fingerprint used to
/// detect changes without re-parsing on every poll.
fn scan(dir: &Path) -> (Vec<SceneDef>, u64) {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(SUFFIX)))
            .collect(),
        Err(_) => return (Vec::new(), 0),
    };
    paths.sort();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut seen_ids = std::collections::HashSet::new();
    let mut defs = Vec::with_capacity(paths.len());

    for path in &paths {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("viz: failed to read scene {}: {e}", path.display());
                continue;
            }
        };
        text.hash(&mut hasher);

        match ron::from_str::<SceneDef>(&text) {
            Ok(def) => {
                if !seen_ids.insert(def.id.clone()) {
                    log::warn!(
                        "viz: duplicate scene id {:?} in {}; skipping",
                        def.id,
                        path.display()
                    );
                    continue;
                }
                defs.push(def);
            }
            Err(e) => log::warn!("viz: failed to parse scene {}: {e}", path.display()),
        }
    }

    (defs, hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eva-viz-test-{name}-{:?}", Instant::now()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fallback() -> SceneDef {
        SceneDef::builtin(0.15, 0.25, 32)
    }

    #[test]
    fn missing_dir_falls_back_to_the_builtin() {
        let lib = SceneLibrary::new(PathBuf::from("/nonexistent/eva-viz-scenes"), fallback());
        assert_eq!(lib.defs().len(), 1);
        assert_eq!(lib.defs()[0].id, "vfd_bars");
    }

    #[test]
    fn empty_dir_falls_back_to_the_builtin() {
        let dir = tmp_dir("empty");
        let lib = SceneLibrary::new(dir, fallback());
        assert_eq!(lib.defs()[0].id, "vfd_bars");
    }

    #[test]
    fn valid_files_are_loaded_and_sorted_by_name() {
        let dir = tmp_dir("valid");
        write(&dir, "b.viz.ron", "(id: \"b\")");
        write(&dir, "a.viz.ron", "(id: \"a\")");
        let lib = SceneLibrary::new(dir, fallback());
        let ids: Vec<&str> = lib.defs().iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn malformed_files_are_skipped_not_fatal() {
        let dir = tmp_dir("malformed");
        write(&dir, "good.viz.ron", "(id: \"good\")");
        write(&dir, "bad.viz.ron", "not valid ron {{{");
        let lib = SceneLibrary::new(dir, fallback());
        assert_eq!(lib.defs().len(), 1);
        assert_eq!(lib.defs()[0].id, "good");
    }

    #[test]
    fn duplicate_ids_keep_the_first_by_filename() {
        let dir = tmp_dir("dup");
        write(&dir, "a.viz.ron", "(id: \"same\")");
        write(&dir, "b.viz.ron", "(id: \"same\")");
        let lib = SceneLibrary::new(dir, fallback());
        assert_eq!(lib.defs().len(), 1);
    }

    #[test]
    fn non_ron_files_are_ignored() {
        let dir = tmp_dir("other-ext");
        write(&dir, "notes.txt", "hello");
        let lib = SceneLibrary::new(dir, fallback());
        assert_eq!(lib.defs()[0].id, "vfd_bars");
    }

    #[test]
    fn a_bad_edit_keeps_serving_the_last_good_set() {
        let dir = tmp_dir("bad-edit");
        write(&dir, "a.viz.ron", "(id: \"a\")");
        let mut lib = SceneLibrary::new(dir.clone(), fallback());
        assert_eq!(lib.defs()[0].id, "a");

        write(&dir, "a.viz.ron", "not valid ron {{{");
        // The active set is unchanged (the edit was rejected), so `reload`
        // must report no change even though the file content did change.
        assert!(!lib.reload());
        assert_eq!(lib.defs()[0].id, "a", "previous scene must persist through a parse error");
    }

    #[test]
    fn unchanged_content_does_not_reload() {
        let dir = tmp_dir("unchanged");
        write(&dir, "a.viz.ron", "(id: \"a\")");
        let mut lib = SceneLibrary::new(dir, fallback());
        assert!(!lib.reload(), "identical content must not report a change");
    }

    #[test]
    fn edited_content_is_picked_up() {
        let dir = tmp_dir("edited");
        write(&dir, "a.viz.ron", "(id: \"a\")");
        let mut lib = SceneLibrary::new(dir.clone(), fallback());

        write(&dir, "a.viz.ron", "(id: \"a-renamed\")");
        assert!(lib.reload());
        assert_eq!(lib.defs()[0].id, "a-renamed");
    }

    #[test]
    fn poll_is_rate_limited() {
        let dir = tmp_dir("rate-limit");
        let mut lib = SceneLibrary::new(dir.clone(), fallback());
        write(&dir, "a.viz.ron", "(id: \"a\")");
        // Too soon after construction: must not observe the new file yet.
        assert!(!lib.poll());
    }
}
