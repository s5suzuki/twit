use std::path::{Path, PathBuf};

use git2::{Config, ConfigLevel, Repository, Signature};

pub fn signature(repo: &Repository) -> Result<Signature<'static>, git2::Error> {
    let Ok(cfg) = repo.config() else {
        return repo.signature();
    };
    let includes = matched_includes(&cfg);
    if includes.is_empty() {
        return repo.signature();
    }
    match (
        resolve(&cfg, &includes, "user.name"),
        resolve(&cfg, &includes, "user.email"),
    ) {
        (Some(name), Some(email)) => Signature::now(&name, &email),
        _ => repo.signature(),
    }
}

fn resolve(cfg: &Config, includes: &[(ConfigLevel, PathBuf)], key: &str) -> Option<String> {
    let included_rank = includes.iter().map(|(l, _)| rank(*l)).max().unwrap_or(0);
    for level in [
        ConfigLevel::App,
        ConfigLevel::Worktree,
        ConfigLevel::Local,
        ConfigLevel::Global,
        ConfigLevel::XDG,
        ConfigLevel::System,
    ] {
        if rank(level) <= included_rank {
            break;
        }
        if let Ok(level) = cfg.open_level(level)
            && let Ok(value) = level.get_string(key)
        {
            return Some(value);
        }
    }
    let mut found = None;
    for (_, path) in includes {
        if let Ok(included) = Config::open(path)
            && let Ok(value) = included.get_string(key)
        {
            found = Some(value);
        }
    }
    found.or_else(|| cfg.get_string(key).ok())
}

fn rank(level: ConfigLevel) -> u8 {
    match level {
        ConfigLevel::ProgramData => 1,
        ConfigLevel::System => 2,
        ConfigLevel::XDG => 3,
        ConfigLevel::Global => 4,
        ConfigLevel::Local => 5,
        ConfigLevel::Worktree => 6,
        ConfigLevel::App => 7,
        ConfigLevel::Highest => u8::MAX,
    }
}

fn matched_includes(cfg: &Config) -> Vec<(ConfigLevel, PathBuf)> {
    let urls = remote_urls(cfg);
    let mut out = Vec::new();
    if urls.is_empty() {
        return out;
    }
    let Ok(mut entries) = cfg.entries(None) else {
        return out;
    };
    while let Some(entry) = entries.next() {
        let Ok(entry) = entry else {
            continue;
        };
        let (Ok(name), Ok(value)) = (entry.name(), entry.value()) else {
            continue;
        };
        let Some(pattern) = hasconfig_url_pattern(name) else {
            continue;
        };
        if urls.iter().any(|url| wildmatch(pattern, url))
            && let Some(path) = expand_path(value)
        {
            let found = (entry.level(), path);
            if !out.contains(&found) {
                out.push(found);
            }
        }
    }
    out
}

fn hasconfig_url_pattern(name: &str) -> Option<&str> {
    name.strip_prefix("includeif.")?
        .strip_suffix(".path")?
        .strip_prefix("hasconfig:remote.*.url:")
}

fn remote_urls(cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(mut entries) = cfg.entries(Some("^remote\\..*\\.url$")) else {
        return out;
    };
    while let Some(entry) = entries.next() {
        let Ok(entry) = entry else {
            continue;
        };
        if let Ok(value) = entry.value() {
            out.push(value.to_string());
        }
    }
    out
}

fn expand_path(value: &str) -> Option<PathBuf> {
    if value == "~" {
        return home();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home().map(|h| h.join(rest));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    Config::find_global()
        .ok()
        .and_then(|global| global.parent().map(|dir| dir.join(path)))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn wildmatch(pattern: &str, text: &str) -> bool {
    dowild(pattern.as_bytes(), 0, text.as_bytes(), 0)
}

fn dowild(p: &[u8], mut pi: usize, t: &[u8], mut ti: usize) -> bool {
    while pi < p.len() {
        let pc = p[pi];
        if pc != b'*' && ti >= t.len() {
            return false;
        }
        match pc {
            b'\\' => {
                pi += 1;
                if pi >= p.len() || t[ti] != p[pi] {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            b'?' => {
                if t[ti] == b'/' {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            b'[' => match class_match(p, pi, t[ti]) {
                Some(next) => {
                    pi = next;
                    ti += 1;
                }
                None => return false,
            },
            b'*' => {
                let after_slash = pi == 0 || p[pi - 1] == b'/';
                pi += 1;
                let mut match_slash = false;
                if pi < p.len() && p[pi] == b'*' {
                    while pi < p.len() && p[pi] == b'*' {
                        pi += 1;
                    }
                    let before_slash = pi >= p.len() || p[pi] == b'/';
                    if after_slash && before_slash {
                        if pi < p.len() && dowild(p, pi + 1, t, ti) {
                            return true;
                        }
                        match_slash = true;
                    }
                }
                if pi >= p.len() {
                    return match_slash || !t[ti..].contains(&b'/');
                }
                let mut k = ti;
                loop {
                    if dowild(p, pi, t, k) {
                        return true;
                    }
                    if k >= t.len() {
                        return false;
                    }
                    if !match_slash && t[k] == b'/' {
                        return false;
                    }
                    k += 1;
                }
            }
            _ => {
                if t[ti] != pc {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    ti == t.len()
}

fn class_match(p: &[u8], mut pi: usize, ch: u8) -> Option<usize> {
    pi += 1;
    let negated = matches!(p.get(pi), Some(b'!') | Some(b'^'));
    if negated {
        pi += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let mut lo = *p.get(pi)?;
        if lo == b']' && !first {
            break;
        }
        first = false;
        if lo == b'\\' {
            pi += 1;
            lo = *p.get(pi)?;
        }
        pi += 1;
        if p.get(pi) == Some(&b'-') && p.get(pi + 1).is_some_and(|c| *c != b']') {
            pi += 1;
            let mut hi = *p.get(pi)?;
            if hi == b'\\' {
                pi += 1;
                hi = *p.get(pi)?;
            }
            pi += 1;
            if lo <= ch && ch <= hi {
                matched = true;
            }
        } else if ch == lo {
            matched = true;
        }
    }
    if matched == negated || ch == b'/' {
        return None;
    }
    Some(pi + 1)
}

#[cfg(test)]
mod tests {
    use super::{hasconfig_url_pattern, wildmatch};

    #[test]
    fn https_url_matches_owner_pattern() {
        assert!(wildmatch(
            "**/tixel-co-ltd/**",
            "https://github.com/tixel-co-ltd/tixel-core.git"
        ));
        assert!(!wildmatch(
            "**/tixel-co-ltd/**",
            "https://github.com/other-org/tixel-core.git"
        ));
    }

    #[test]
    fn ssh_url_matches_colon_pattern() {
        assert!(wildmatch(
            "**:tixel-co-ltd/**",
            "git@github.com:tixel-co-ltd/tixel-core.git"
        ));
        assert!(!wildmatch(
            "**/tixel-co-ltd/**",
            "git@github.com:tixel-co-ltd/tixel-core.git"
        ));
    }

    #[test]
    fn double_star_spans_slashes_and_single_star_does_not() {
        assert!(wildmatch("https://**/repo.git", "https://a/b/c/repo.git"));
        assert!(!wildmatch("https://*/repo.git", "https://a/b/repo.git"));
        assert!(wildmatch("https://*/repo.git", "https://a/repo.git"));
    }

    #[test]
    fn double_star_slash_matches_zero_components() {
        assert!(wildmatch("**/shinolab/**", "/shinolab/autd3.git"));
        assert!(wildmatch("a/**/b", "a/b"));
        assert!(wildmatch("a/**/b", "a/x/y/b"));
    }

    #[test]
    fn question_mark_and_class_do_not_cross_slashes() {
        assert!(wildmatch("a?c", "abc"));
        assert!(!wildmatch("a?c", "a/c"));
        assert!(wildmatch("a[bc]d", "abd"));
        assert!(!wildmatch("a[!bc]d", "abd"));
        assert!(wildmatch("a[!bc]d", "axd"));
        assert!(wildmatch("a[0-9]d", "a5d"));
    }

    #[test]
    fn trailing_star_stops_at_slash() {
        assert!(wildmatch("https://github.com/*", "https://github.com/repo"));
        assert!(!wildmatch(
            "https://github.com/*",
            "https://github.com/org/repo"
        ));
        assert!(wildmatch("https://github.com/**", "https://github.com/o/r"));
    }

    #[test]
    fn only_hasconfig_url_include_keys_are_recognized() {
        assert_eq!(
            hasconfig_url_pattern("includeif.hasconfig:remote.*.url:**/org/**.path"),
            Some("**/org/**")
        );
        assert_eq!(
            hasconfig_url_pattern("includeif.gitdir:/home/shun/repos/autd3-rewrite/.path"),
            None
        );
        assert_eq!(hasconfig_url_pattern("include.path"), None);
    }
}
