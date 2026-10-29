//! Path completion on the daemon's host (PAR-235; the TUI's
//! `remote-path-completion.ts` and `autocomplete.ts`): a word at the caret
//! that looks like a path (`~`, `~/…`, `./…`, `../…`, `/a/…`) completes
//! from `list_path_directory` listings, kept 15 s fresh and up to 5 min
//! stale (shown while a fresh one is asked for); a request not answered in
//! 8 s is forgotten. Directories first; dotfiles only when the typed name
//! starts with ".". Never the phone's own files: the daemon's host is where
//! a path in a message means something.

use std::time::{Duration, Instant};

use super::Item;
use crate::exocortex::proto::PathListing;

const FRESH: Duration = Duration::from_secs(15);
const STALE: Duration = Duration::from_secs(5 * 60);
pub const INFLIGHT: Duration = Duration::from_secs(8);
const KEEP: usize = 192;

/// A path word split into its directory (as typed, with its slash) and the
/// name typed after it.
pub fn parts(token: &str) -> Option<(String, String)> {
    if token == "~" {
        return Some(("~/".into(), String::new()));
    }
    let slash = token.rfind('/')?;
    Some((token[..=slash].to_owned(), token[slash + 1..].to_owned()))
}

/// The completions a listing offers for `token`.
fn matches(token: &str, listing: &PathListing) -> Vec<Item> {
    let Some((directory, prefix)) = parts(token) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = listing
        .entries
        .iter()
        .filter(|e| {
            e.name.starts_with(&prefix) && (prefix.starts_with('.') || !e.name.starts_with('.'))
        })
        .collect();
    entries.sort_by(|a, b| {
        (a.kind != "dir")
            .cmp(&(b.kind != "dir"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
        .into_iter()
        .map(|e| {
            let dir = e.kind == "dir";
            Item::new(
                format!("{directory}{}{}", e.name, if dir { "/" } else { "" }),
                if dir { "dir" } else { "file" },
            )
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Cached {
    listing: PathListing,
    received: Instant,
    used: Instant,
}

#[derive(Debug, Clone)]
struct Asked {
    directory: String,
    prefix: String,
    at: Instant,
}

/// The listings seen and those asked for.
#[derive(Debug, Default)]
pub struct Paths {
    cache: Vec<Cached>,
    asked: Vec<(String, Asked)>,
}

impl Paths {
    fn find(&self, token: &str, now: Instant) -> Option<usize> {
        let (directory, prefix) = parts(token)?;
        let mut best: Option<usize> = None;
        for (index, c) in self.cache.iter().enumerate() {
            if now.saturating_duration_since(c.received) > STALE
                || c.listing.directory != directory
                || !prefix.starts_with(&c.listing.prefix)
            {
                continue;
            }
            // A cut-short broad listing cannot prove a narrower one complete.
            if c.listing.truncated && c.listing.prefix != prefix {
                continue;
            }
            if best.is_none_or(|b| c.listing.prefix.len() > self.cache[b].listing.prefix.len()) {
                best = Some(index);
            }
        }
        best
    }

    /// What `token` completes to, if a listing for it is here (None: not
    /// yet asked, or on its way).
    pub fn matches(&self, token: &str, now: Instant) -> Option<Vec<Item>> {
        if token == "~" {
            return Some(vec![Item::new("~/", "dir")]);
        }
        self.find(token, now)
            .map(|i| matches(token, &self.cache[i].listing))
    }

    /// What to ask the daemon for `token` (its directory and typed name),
    /// unless a fresh listing covers it or a request already does.
    pub fn wanted(&mut self, token: &str, now: Instant) -> Option<(String, String)> {
        if token == "~" {
            return None;
        }
        let (directory, prefix) = parts(token)?;
        self.asked
            .retain(|(_, a)| now.saturating_duration_since(a.at) < INFLIGHT);
        if let Some(i) = self.find(token, now) {
            self.cache[i].used = now;
            if now.saturating_duration_since(self.cache[i].received) <= FRESH {
                return None;
            }
        }
        // One empty-prefix prefetch and one targeted request at most; later
        // characters ride on them.
        let covered = self.asked.iter().any(|(_, a)| {
            a.directory == directory
                && prefix.starts_with(&a.prefix)
                && (!a.prefix.is_empty() || prefix.is_empty())
        });
        (!covered).then_some((directory, prefix))
    }

    /// A request for `directory`/`prefix` went out as `id`.
    pub fn asked(&mut self, id: String, directory: String, prefix: String, now: Instant) {
        self.asked.push((
            id,
            Asked {
                directory,
                prefix,
                at: now,
            },
        ));
    }

    pub fn is_ours(&self, id: &str) -> bool {
        self.asked.iter().any(|(asked, _)| asked == id)
    }

    /// The daemon's answer to `id`; an error (an old daemon) is an empty
    /// listing, never the phone's own files.
    pub fn answered(&mut self, id: &str, listings: Option<Vec<PathListing>>, now: Instant) {
        let Some(at) = self.asked.iter().position(|(asked, _)| asked == id) else {
            return;
        };
        let (_, asked) = self.asked.remove(at);
        let listings = listings.unwrap_or_else(|| {
            vec![PathListing {
                directory: asked.directory,
                prefix: asked.prefix,
                ..PathListing::default()
            }]
        });
        for listing in listings {
            self.cache.retain(|c| {
                c.listing.directory != listing.directory || c.listing.prefix != listing.prefix
            });
            self.cache.push(Cached {
                listing,
                received: now,
                used: now,
            });
        }
        if self.cache.len() > KEEP {
            self.cache.sort_by_key(|c| std::cmp::Reverse(c.used));
            self.cache.truncate(KEEP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::PathEntry;

    fn listing(directory: &str, prefix: &str, names: &[(&str, &str)]) -> PathListing {
        PathListing {
            directory: directory.into(),
            prefix: prefix.into(),
            entries: names
                .iter()
                .map(|(name, kind)| PathEntry {
                    name: (*name).into(),
                    kind: (*kind).into(),
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn listings_complete_and_requests_are_shared() {
        let now = Instant::now();
        let mut paths = Paths::default();
        assert_eq!(parts("~/src/ex").unwrap(), ("~/src/".into(), "ex".into()));
        assert!(paths.matches("~/src/", now).is_none());
        assert_eq!(
            paths.wanted("~/src/", now),
            Some(("~/src/".into(), String::new()))
        );
        paths.asked("r1".into(), "~/src/".into(), String::new(), now);
        assert_eq!(paths.wanted("~/src/", now), None, "on its way");
        assert_eq!(
            paths.wanted("~/src/e", now),
            Some(("~/src/".into(), "e".into())),
            "one targeted request beside the prefetch"
        );
        paths.answered(
            "r1",
            Some(vec![listing(
                "~/src/",
                "",
                &[
                    ("exo", "file"),
                    (".hidden", "dir"),
                    ("apps", "dir"),
                    ("Exo2", "dir"),
                ],
            )]),
            now,
        );
        let names: Vec<String> = paths
            .matches("~/src/", now)
            .unwrap()
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(
            names,
            vec!["~/src/apps/", "~/src/Exo2/", "~/src/exo"],
            "dirs first, no dotfiles"
        );
        let names: Vec<String> = paths
            .matches("~/src/.", now)
            .unwrap()
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, vec!["~/src/.hidden/"]);
        assert_eq!(paths.wanted("~/src/a", now), None, "fresh");
        // Stale after 15 s: shown, and asked again; gone after 5 min.
        let later = now + Duration::from_secs(20);
        assert!(paths.matches("~/src/a", later).is_some());
        assert!(paths.wanted("~/src/a", later).is_some());
        assert!(
            paths
                .matches("~/src/a", now + Duration::from_secs(400))
                .is_none()
        );
        // An error answers with nothing.
        paths.asked("r2".into(), "/opt/".into(), String::new(), now);
        paths.answered("r2", None, now);
        assert_eq!(paths.matches("/opt/x", now), Some(Vec::new()));
        assert_eq!(paths.matches("~", now).unwrap()[0].name, "~/");
    }
}
