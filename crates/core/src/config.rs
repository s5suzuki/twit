use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::keymap::Keymap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub graph_commit_limit: usize,
    pub graph_show_author: bool,
    pub graph_show_email: bool,
    pub graph_show_date: bool,
    pub graph_files_tree: bool,
    #[serde(alias = "confirm_delete")]
    pub confirm_discard: bool,
    pub vim_input: bool,
    #[serde(default)]
    pub keys: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            graph_commit_limit: 200,
            graph_show_author: true,
            graph_show_email: true,
            graph_show_date: true,
            graph_files_tree: true,
            confirm_discard: true,
            vim_input: true,
            keys: BTreeMap::new(),
        }
    }
}

const KEY_ALIASES: [(&str, &str); 1] = [("confirm_discard", "confirm_delete")];

pub struct InitReport {
    pub path: PathBuf,
    pub created: bool,
    pub regenerated: bool,
    pub backup: Option<PathBuf>,
    pub added: Vec<String>,
    pub added_bindings: usize,
}

impl Config {
    pub fn load() -> Self {
        Self::load_checked().0
    }

    pub fn load_checked() -> (Self, Vec<String>) {
        let Some(path) = config_path() else {
            return (Self::default(), Vec::new());
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Self::default(), Vec::new());
        };
        match toml::from_str::<Self>(&text) {
            Ok(cfg) => {
                let warnings = unknown_keys(&text)
                    .into_iter()
                    .map(|k| format!("config: unknown key \"{k}\" in {}, ignored", path.display()))
                    .collect();
                (cfg, warnings)
            }
            Err(e) => (
                Self::default(),
                vec![format!(
                    "config: {}: {}, using defaults",
                    path.display(),
                    e.message()
                )],
            ),
        }
    }

    pub fn init_file(force: bool) -> Result<InitReport, String> {
        let path = config_path()
            .ok_or("cannot locate the config directory (set XDG_CONFIG_HOME or HOME)")?;
        init_at(path, force)
    }

    pub fn save(&self) {
        let Some(path) = config_path() else {
            return;
        };
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if let Ok(s) = merge_into(&existing, self) {
            let _ = write_file(&path, &s);
        }
    }
}

fn init_at(path: PathBuf, force: bool) -> Result<InitReport, String> {
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let created = existing.is_none();
    let base = if force { None } else { existing.as_deref() };
    let (text, added, added_bindings) =
        fill_defaults(base.unwrap_or("")).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut backup = None;
    if force && let Some(existing) = existing.filter(|s| *s != text) {
        let bak = path.with_extension("toml.bak");
        write_file(&bak, &existing)?;
        backup = Some(bak);
    }
    if !added.is_empty() || added_bindings > 0 || created {
        write_file(&path, &text)?;
    }
    Ok(InitReport {
        path,
        created,
        regenerated: force && !created,
        backup,
        added,
        added_bindings,
    })
}

pub fn init_summary(r: &InitReport) -> String {
    let path = r.path.display();
    let keys = r.added.len();
    let bindings = r.added_bindings;
    let mut s = if r.created {
        format!("created {path} ({keys} keys, {bindings} bindings)")
    } else if r.regenerated {
        format!("regenerated {path} ({keys} keys, {bindings} bindings)")
    } else if keys == 0 && bindings == 0 {
        format!("{path} is already complete")
    } else {
        format!("updated {path} (+{keys} keys, +{bindings} bindings)")
    };
    if let Some(bak) = &r.backup {
        s.push_str(&format!("; the previous file is at {}", bak.display()));
    }
    s
}

fn document_of(cfg: &Config) -> Result<DocumentMut, String> {
    toml::to_string(cfg)
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())
}

fn default_document() -> Result<DocumentMut, String> {
    document_of(&Config::default())
}

fn merge_into(existing: &str, cfg: &Config) -> Result<String, String> {
    toml::from_str::<Config>(existing).map_err(|e| e.message().to_string())?;
    let mut doc: DocumentMut = existing.parse().map_err(|e| format!("{e}"))?;
    for (key, item) in document_of(cfg)?.as_table().iter() {
        if item.is_table_like()
            && (doc.contains_key(key) || item.as_table_like().is_some_and(|t| t.is_empty()))
        {
            continue;
        }
        for (canonical, alias) in KEY_ALIASES {
            if canonical == key {
                doc.remove(alias);
            }
        }
        doc[key] = item.clone();
    }
    Ok(doc.to_string())
}

fn fill_defaults(existing: &str) -> Result<(String, Vec<String>, usize), String> {
    let mut doc: DocumentMut = existing.parse().map_err(|e: toml_edit::TomlError| {
        let mut m = e.to_string();
        m.truncate(m.find('\n').unwrap_or(m.len()));
        m
    })?;

    let mut added = Vec::new();
    for (key, item) in default_document()?.as_table().iter() {
        if item.is_table_like() || doc.contains_key(key) {
            continue;
        }
        if KEY_ALIASES
            .iter()
            .any(|(k, alias)| *k == key && doc.contains_key(alias))
        {
            continue;
        }
        doc[key] = item.clone();
        added.push(key.to_string());
    }

    let added_bindings = fill_default_bindings(&mut doc);
    Ok((doc.to_string(), added, added_bindings))
}

fn fill_default_bindings(doc: &mut DocumentMut) -> usize {
    if !doc.contains_key("keys") {
        doc["keys"] = Item::Table(implicit_table());
    }
    let Some(keys) = doc["keys"].as_table_mut() else {
        return 0;
    };
    keys.set_implicit(true);

    let mut added = 0;
    for (ctx, bindings) in Keymap::default_bindings() {
        if !keys.contains_key(ctx.name()) {
            keys.insert(ctx.name(), Item::Table(Table::new()));
        }
        let Some(table) = keys[ctx.name()].as_table_mut() else {
            continue;
        };
        for (chord, action) in bindings {
            if table.contains_key(&chord) {
                continue;
            }
            table.insert(&chord, value(action));
            added += 1;
        }
    }
    added
}

fn implicit_table() -> Table {
    let mut t = Table::new();
    t.set_implicit(true);
    t
}

fn unknown_keys(text: &str) -> Vec<String> {
    let Ok(known) = default_document() else {
        return Vec::new();
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    doc.as_table()
        .iter()
        .map(|(k, _)| k)
        .filter(|k| !known.contains_key(k) && !KEY_ALIASES.iter().any(|(_, alias)| alias == k))
        .map(str::to_string)
        .collect()
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("twit").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Action, Context};

    #[test]
    fn roundtrip_preserves_values() {
        let cfg = Config {
            graph_commit_limit: 500,
            vim_input: false,
            ..Config::default()
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.graph_commit_limit, 500);
        assert!(!back.vim_input);
    }

    #[test]
    fn partial_config_fills_defaults() {
        let back: Config = toml::from_str("graph_commit_limit = 50").unwrap();
        assert_eq!(back.graph_commit_limit, 50);
        assert!(back.graph_show_author);
        assert!(back.graph_show_email);
        assert!(back.graph_show_date);
        assert!(back.graph_files_tree);
        assert!(back.confirm_discard);
        assert!(back.vim_input);
    }

    #[test]
    fn keys_table_deserializes() {
        let src = "\
graph_commit_limit = 100

[keys.diff]
\"ctrl+e\" = \"diff-half-page-down\"

[keys.global]
\"ctrl+t\" = \"open-search\"
";
        let cfg: Config = toml::from_str(src).unwrap();
        assert_eq!(
            cfg.keys["diff"]["ctrl+e"],
            "diff-half-page-down".to_string()
        );
        assert_eq!(cfg.keys["global"]["ctrl+t"], "open-search".to_string());
    }

    #[test]
    fn missing_keys_table_defaults_empty() {
        let cfg: Config = toml::from_str("graph_commit_limit = 100").unwrap();
        assert!(cfg.keys.is_empty());
    }

    #[test]
    fn corrupt_toml_is_a_parse_error() {
        assert!(toml::from_str::<Config>("this is not toml = = =").is_err());
        assert!(toml::from_str::<Config>("vim_input = \"maybe\"").is_err());
    }

    #[test]
    fn unknown_keys_are_reported() {
        let src = "\
graph_commit_limit = 100
grpah_commit_limit = 100
confirm_delete = false

[keys.global]
\"ctrl+t\" = \"open-search\"
";
        assert_eq!(unknown_keys(src), vec!["grpah_commit_limit".to_string()]);
        assert!(unknown_keys("").is_empty());
    }

    #[test]
    fn fill_defaults_writes_every_key() {
        let (text, added, bindings) = fill_defaults("").unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.graph_commit_limit, Config::default().graph_commit_limit);
        assert!(added.contains(&"graph_commit_limit".to_string()));
        assert!(added.contains(&"vim_input".to_string()));
        assert!(unknown_keys(&text).is_empty());

        let expected: usize = Keymap::default_bindings()
            .iter()
            .map(|(_, table)| table.len())
            .sum();
        assert_eq!(bindings, expected);
        for ctx in Context::ALL {
            assert!(
                !cfg.keys[ctx.name()].is_empty(),
                "[keys.{}] is generated:\n{text}",
                ctx.name()
            );
        }
        assert_eq!(cfg.keys["global"]["alt+o"], "cycle-tab-fwd".to_string());
        assert_eq!(cfg.keys["changes"]["g g"], "changes-top".to_string());
        assert_eq!(cfg.keys["changes"]["shift+g"], "changes-bottom".to_string());

        let (again, added, bindings) = fill_defaults(&text).unwrap();
        assert!(added.is_empty(), "second run adds nothing");
        assert_eq!(bindings, 0, "second run adds no bindings");
        assert_eq!(again, text);
    }

    #[test]
    fn generated_bindings_reproduce_the_defaults() {
        let (text, _, _) = fill_defaults("").unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        let from_file = Keymap::from_config(&cfg.keys);
        let defaults = Keymap::default();

        for ctx in Context::ALL {
            let mut a: Vec<(String, &str)> = from_file
                .help_for(ctx)
                .iter()
                .map(|e| (e.keys.clone(), e.action.name()))
                .collect();
            let mut b: Vec<(String, &str)> = defaults
                .help_for(ctx)
                .iter()
                .map(|e| (e.keys.clone(), e.action.name()))
                .collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "[keys.{}] round-trips", ctx.name());
        }
    }

    fn temp_config(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("twit-config-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    #[test]
    fn init_creates_then_leaves_the_file_alone() {
        let path = temp_config("create");
        let r = init_at(path.clone(), false).unwrap();
        assert!(r.created && !r.regenerated && r.backup.is_none());
        assert_eq!(r.added.len(), 7);

        let text = std::fs::read_to_string(&path).unwrap();
        let r = init_at(path.clone(), false).unwrap();
        assert!(!r.created && r.added.is_empty() && r.added_bindings == 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn force_regenerates_and_backs_up() {
        let path = temp_config("force");
        let src = "\
# my settings
graph_commit_limit = 500

[keys.diff]
\"ctrl+d\" = \"none\"
";
        std::fs::write(&path, src).unwrap();

        let r = init_at(path.clone(), true).unwrap();
        assert!(r.regenerated && !r.created);
        assert_eq!(r.added.len(), 7, "every key is rewritten");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("# my settings"), "regenerated:\n{text}");
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.graph_commit_limit, 200, "reset to the default");
        assert_eq!(
            cfg.keys["diff"]["ctrl+d"],
            "diff-half-page-down".to_string()
        );

        let bak = r.backup.expect("the old file is kept");
        assert_eq!(bak, path.with_extension("toml.bak"));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), src);

        assert_eq!(text, fill_defaults("").unwrap().0, "same as a fresh init");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn force_on_a_missing_file_just_creates_it() {
        let path = temp_config("force-new");
        let r = init_at(path.clone(), true).unwrap();
        assert!(r.created && !r.regenerated && r.backup.is_none());
        assert!(!path.with_extension("toml.bak").exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn fill_defaults_keeps_existing_bindings() {
        let src = "\
[keys.diff]
\"ctrl+d\" = \"none\"
\"j\" = \"diff-half-page-down\"
";
        let (text, _, _) = fill_defaults(src).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.keys["diff"]["ctrl+d"], "none".to_string());
        assert_eq!(
            cfg.keys["diff"]["j"],
            "diff-half-page-down".to_string(),
            "a rebound chord is not reset to its default action"
        );
        assert_eq!(cfg.keys["diff"]["k"], "diff-up".to_string());

        let km = Keymap::from_config(&cfg.keys);
        let entries = km.help_for(Context::Diff);
        assert!(!entries.iter().any(|e| e.keys == "Ctrl+d"), "still unbound");
        assert!(
            entries
                .iter()
                .any(|e| e.keys == "j" && e.action == Action::DiffHalfPageDown)
        );
    }

    #[test]
    fn fill_defaults_keeps_existing_values_and_comments() {
        let src = "\
# my settings
graph_commit_limit = 500
confirm_delete = false

[keys.global]
\"ctrl+t\" = \"open-search\"
";
        let (text, added, _) = fill_defaults(src).unwrap();
        assert!(text.contains("# my settings"), "got:\n{text}");
        assert!(!added.contains(&"graph_commit_limit".to_string()));
        assert!(
            !added.contains(&"confirm_discard".to_string()),
            "the confirm_delete alias counts as set"
        );
        assert!(added.contains(&"vim_input".to_string()));

        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.graph_commit_limit, 500);
        assert!(!cfg.confirm_discard);
        assert!(cfg.vim_input);
        assert_eq!(cfg.keys["global"]["ctrl+t"], "open-search".to_string());
    }

    #[test]
    fn fill_defaults_rejects_corrupt_toml() {
        assert!(fill_defaults("this is not toml = = =").is_err());
    }

    #[test]
    fn saving_keeps_comments_and_rebindings() {
        let src = "\
# my settings
graph_commit_limit = 500
confirm_delete = true

[keys.global]
\"ctrl+t\" = \"open-search\"
";
        let mut cfg: Config = toml::from_str(src).unwrap();
        cfg.confirm_discard = false;
        let text = merge_into(src, &cfg).unwrap();
        assert!(text.contains("# my settings"), "got:\n{text}");
        assert!(
            !text.contains("confirm_delete"),
            "the alias is replaced, not duplicated:\n{text}"
        );

        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.graph_commit_limit, 500);
        assert!(!back.confirm_discard);
        assert_eq!(back.keys["global"]["ctrl+t"], "open-search".to_string());
        assert!(unknown_keys(&text).is_empty());
    }

    #[test]
    fn saving_never_overwrites_a_file_that_does_not_load() {
        assert!(merge_into("this is not toml = = =", &Config::default()).is_err());
        assert!(
            merge_into(
                "vim_input = false\ngraph_commit_limit = \"x\"\n",
                &Config::default()
            )
            .is_err()
        );
    }

    #[test]
    fn saving_into_an_empty_file_writes_the_settings() {
        let text = merge_into("", &Config::default()).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.graph_commit_limit, 200);
    }
}
