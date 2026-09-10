//! Prefix rules that rewrite a torrent's save path.
//!
//! A mapping is always a *set* of rules, even when the set has one member.
//! That is the shape transfer and rewrite will consume, so it is derived here
//! rather than as a single string replace.
//!
//! Matching is component-wise. The string `/data/movie` is not a prefix of
//! `/data/movies`; those are different directories, and treating them as the
//! same is how existing rewrite tools corrupt a library.

/// A prefix replacement: every path under `from` is rewritten under `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Source prefix, as path components (`["Users", "me", "data"]`).
    pub from: Vec<String>,
    /// Destination prefix, same representation.
    pub to: Vec<String>,
}

impl Rule {
    /// `from` as a `/`-joined path with a leading slash (Unix form).
    #[must_use]
    pub fn from_display(&self) -> String {
        display(&self.from)
    }

    /// `to` as a `/`-joined path with a leading slash (Unix form).
    #[must_use]
    pub fn to_display(&self) -> String {
        display(&self.to)
    }
}

/// Ordered rules; the longest matching `from` wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    rules: Vec<Rule>,
}

/// Why a mapping could not be derived.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No save paths were supplied.
    #[error("no save paths to derive a mapping from")]
    NoSavePaths,
    /// The destination root was empty or only separators.
    #[error("destination root is empty")]
    EmptyDestination,
}

impl Mapping {
    /// Derives rules from observed save paths and a destination root.
    ///
    /// One shared non-degenerate prefix becomes one rule. If the only common
    /// prefix is `/`, `/Users`, `/home`, `/mnt`, `/media`, or a drive letter,
    /// each distinct top-level root gets its own rule, all pointing at `dest`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when there are no paths or `dest` is empty.
    pub fn derive(save_paths: &[impl AsRef<str>], dest: &str) -> Result<Self, Error> {
        let dest = components(dest);
        if dest.is_empty() {
            return Err(Error::EmptyDestination);
        }

        let paths: Vec<Vec<String>> = save_paths
            .iter()
            .map(AsRef::as_ref)
            .map(components)
            .filter(|path| !path.is_empty())
            .collect();

        if paths.is_empty() {
            return Err(Error::NoSavePaths);
        }

        let shared = longest_common_prefix(&paths);
        let mut rules = if is_degenerate(&shared) {
            let mut roots: Vec<Vec<String>> =
                paths.iter().map(|path| top_level_root(path)).collect();
            roots.sort();
            roots.dedup();
            roots
                .into_iter()
                .map(|from| Rule {
                    from,
                    to: dest.clone(),
                })
                .collect()
        } else {
            vec![Rule {
                from: shared,
                to: dest,
            }]
        };

        // Longest from first so a more specific rule wins an overlap.
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.from.len()));
        Ok(Self { rules })
    }

    /// The rules, longest `from` first.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Rewrites `path` if a rule's `from` is a component-wise prefix.
    #[must_use]
    pub fn apply(&self, path: &str) -> Option<String> {
        let parts = components(path);
        for rule in &self.rules {
            if is_prefix(&rule.from, &parts) {
                let mut out = rule.to.clone();
                out.extend(parts[rule.from.len()..].iter().cloned());
                return Some(display(&out));
            }
        }
        None
    }

    /// Pairs of rules that share a destination prefix.
    ///
    /// Two sources mapping onto one destination can land two different
    /// `Album/` directories on each other. Plan reports this before transfer.
    #[must_use]
    pub fn converging(&self) -> Vec<(&Rule, &Rule)> {
        let mut out = Vec::new();
        for (i, left) in self.rules.iter().enumerate() {
            for right in &self.rules[i + 1..] {
                if left.to == right.to && left.from != right.from {
                    out.push((left, right));
                }
            }
        }
        out
    }
}

/// Split a path on `/` or `\`, dropping empties so `/foo/` and `foo` agree.
#[must_use]
pub fn components(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect()
}

fn display(parts: &[String]) -> String {
    if parts.is_empty() {
        return "/".to_owned();
    }
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    out
}

fn longest_common_prefix(paths: &[Vec<String>]) -> Vec<String> {
    let Some(first) = paths.first() else {
        return Vec::new();
    };
    let mut prefix = first.clone();
    for path in &paths[1..] {
        let shared = prefix
            .iter()
            .zip(path.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(shared);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

fn is_prefix(prefix: &[String], path: &[String]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| a == b)
}

/// Prefixes that must not become a rule. Rewriting from these mirrors the
/// source machine's mount layout onto the destination, which is never wanted.
fn is_degenerate(parts: &[String]) -> bool {
    match parts {
        [] => true,
        [only] => {
            matches!(
                only.as_str(),
                "Users" | "home" | "mnt" | "media" | "Volumes"
            ) || is_drive_root(only)
        }
        _ => false,
    }
}

fn is_drive_root(part: &str) -> bool {
    let bytes = part.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn top_level_root(path: &[String]) -> Vec<String> {
    for end in 1..=path.len() {
        if !is_degenerate(&path[..end]) {
            return path[..end].to_vec();
        }
    }
    path.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shared_root_becomes_one_rule() {
        let mapping = Mapping::derive(
            &["/srv/music/flac", "/srv/music/mp3", "/srv/music/vinyl-rips"],
            "/data",
        )
        .expect("derive");

        assert_eq!(mapping.rules().len(), 1);
        assert_eq!(mapping.rules()[0].from_display(), "/srv/music");
        assert_eq!(
            mapping.apply("/srv/music/flac"),
            Some("/data/flac".to_owned())
        );
    }

    #[test]
    fn movie_is_not_a_prefix_of_movies() {
        let mapping = Mapping::derive(&["/data/movie"], "/dest").expect("derive");
        assert_eq!(mapping.apply("/data/movies"), None);
        assert_eq!(mapping.apply("/data/movie"), Some("/dest".to_owned()));
        assert_eq!(
            mapping.apply("/data/movie/album"),
            Some("/dest/album".to_owned())
        );
    }

    #[test]
    fn degenerate_mnt_falls_back_to_per_disk_rules() {
        let mapping =
            Mapping::derive(&["/mnt/disk1/movies", "/mnt/disk2/music"], "/data").expect("derive");

        assert_eq!(mapping.rules().len(), 2);
        assert_eq!(
            mapping.apply("/mnt/disk1/movies"),
            Some("/data/movies".to_owned())
        );
        assert_eq!(
            mapping.apply("/mnt/disk2/music"),
            Some("/data/music".to_owned())
        );
        assert!(!mapping.converging().is_empty());
    }

    #[test]
    fn users_alone_is_rejected() {
        let mapping = Mapping::derive(&["/Users/alex/data/a", "/Users/alex/other/b"], "/data")
            .expect("derive");

        // LCP is /Users/alex — not degenerate (two components). One rule.
        assert_eq!(mapping.rules().len(), 1);
        assert_eq!(mapping.rules()[0].from_display(), "/Users/alex");
    }

    #[test]
    fn users_without_a_deeper_shared_prefix_splits() {
        let mapping =
            Mapping::derive(&["/Users/alex/data", "/Users/sam/data"], "/data").expect("derive");

        // LCP is /Users, which is degenerate → one rule per /Users/<name>.
        assert_eq!(mapping.rules().len(), 2);
        assert_eq!(mapping.converging().len(), 1);
    }

    #[test]
    fn longest_from_wins() {
        let mapping = Mapping {
            rules: vec![
                Rule {
                    from: components("/data/music"),
                    to: components("/new/music"),
                },
                Rule {
                    from: components("/data"),
                    to: components("/new"),
                },
            ],
        };
        // Manually unsorted — Mapping::apply still walks in vec order,
        // so put the long rule first as derive would.
        assert_eq!(
            mapping.apply("/data/music/album"),
            Some("/new/music/album".to_owned())
        );
    }

    #[test]
    fn empty_dest_is_an_error() {
        assert_eq!(
            Mapping::derive(&["/srv/a"], "/").unwrap_err(),
            Error::EmptyDestination
        );
    }
}
