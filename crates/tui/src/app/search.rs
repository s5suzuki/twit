use super::*;

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub include: String,
    pub exclude: String,
    pub hits: Vec<twit_core::search::FileHit>,
    pub cursor: usize,
    pub scroll: usize,
    pub view_rows: usize,
    pub folded_dirs: HashSet<String>,
    pub folded_files: HashSet<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SearchRow {
    Dir {
        name: String,
        path: String,
        open: bool,
        depth: usize,
    },
    File {
        hit: usize,
        depth: usize,
    },
    Line(usize, usize),
}

impl SearchState {
    pub fn rows(&self) -> Vec<SearchRow> {
        let files: Vec<CommitFile> = self
            .hits
            .iter()
            .map(|f| CommitFile {
                path: f.path.clone(),
                kind: repo::StatusKind::Modified,
            })
            .collect();
        let mut out = Vec::new();
        for row in repo::commit_file_rows(&files, true, &self.folded_dirs) {
            match row.kind {
                repo::CommitRowKind::Folder { name, path, open } => {
                    out.push(SearchRow::Dir {
                        name,
                        path,
                        open,
                        depth: row.depth,
                    });
                }
                repo::CommitRowKind::File(i) => {
                    out.push(SearchRow::File {
                        hit: i,
                        depth: row.depth,
                    });
                    if !self.folded_files.contains(&self.hits[i].path) {
                        for j in 0..self.hits[i].lines.len() {
                            out.push(SearchRow::Line(i, j));
                        }
                    }
                }
            }
        }
        out
    }

    pub fn match_count(&self) -> usize {
        self.hits
            .iter()
            .flat_map(|f| f.lines.iter())
            .map(|l| l.ranges.len())
            .sum()
    }
}

impl TuiApp {
    pub(super) fn search_keys(&mut self, queue: &mut KeyQueue) {
        if queue.take(Modifiers::NONE, Key::Slash) {
            let query = self.search.query.clone();
            self.open_prompt_with(Prompt::SearchQuery, query);
            return;
        }
        if queue.take(Modifiers::NONE, Key::I) {
            let include = self.search.include.clone();
            self.open_prompt_with(Prompt::SearchInclude, include);
            return;
        }
        if queue.take(Modifiers::NONE, Key::X) {
            let exclude = self.search.exclude.clone();
            self.open_prompt_with(Prompt::SearchExclude, exclude);
            return;
        }
        if queue.take(Modifiers::NONE, Key::R) {
            if self.search.hits.is_empty() {
                self.error = Some("nothing to replace (run a search first)".to_string());
            } else {
                self.open_prompt(Prompt::SearchReplace);
            }
            return;
        }
        let rows = self.search.rows();
        if rows.is_empty() {
            return;
        }
        let last = rows.len() - 1;
        let half = (self.search.view_rows / 2).max(1);
        if queue.take(Modifiers::NONE, Key::J) || queue.take(Modifiers::NONE, Key::ArrowDown) {
            self.search.cursor = (self.search.cursor + 1).min(last);
        }
        if queue.take(Modifiers::NONE, Key::K) || queue.take(Modifiers::NONE, Key::ArrowUp) {
            self.search.cursor = self.search.cursor.saturating_sub(1);
        }
        if queue.take(Modifiers::CTRL, Key::D) {
            self.search.cursor = (self.search.cursor + half).min(last);
        }
        if queue.take(Modifiers::CTRL, Key::U) {
            self.search.cursor = self.search.cursor.saturating_sub(half);
        }
        if queue.take(Modifiers::SHIFT, Key::G) {
            self.search.cursor = last;
        }
        let cur = rows.get(self.search.cursor.min(last)).cloned();
        if queue.take(Modifiers::NONE, Key::L) {
            self.search_fold(cur.as_ref(), false);
        }
        if queue.take(Modifiers::NONE, Key::H) {
            self.search_fold(cur.as_ref(), true);
        }
        if queue.take(Modifiers::NONE, Key::Enter) || queue.take(Modifiers::NONE, Key::E) {
            match cur {
                Some(SearchRow::Dir { ref path, .. }) => {
                    if !self.search.folded_dirs.remove(path) {
                        self.search.folded_dirs.insert(path.clone());
                    }
                }
                Some(SearchRow::File { hit, .. }) => {
                    if let Some(f) = self.search.hits.get(hit) {
                        let line = f.lines.first().map(|l| l.line_no);
                        self.pending_editor = Some((self.selected.join(&f.path), line));
                    }
                }
                Some(SearchRow::Line(i, j)) => {
                    if let Some(f) = self.search.hits.get(i) {
                        let line = f.lines.get(j).map(|l| l.line_no);
                        self.pending_editor = Some((self.selected.join(&f.path), line));
                    }
                }
                None => {}
            }
        }
    }

    pub(super) fn search_fold(&mut self, row: Option<&SearchRow>, fold: bool) {
        match row {
            Some(SearchRow::Dir { path, .. }) => {
                if fold {
                    self.search.folded_dirs.insert(path.clone());
                } else {
                    self.search.folded_dirs.remove(path);
                }
            }
            Some(SearchRow::File { hit, .. }) | Some(SearchRow::Line(hit, _)) => {
                if let Some(f) = self.search.hits.get(*hit) {
                    if fold {
                        self.search.folded_files.insert(f.path.clone());
                    } else {
                        self.search.folded_files.remove(&f.path);
                    }
                }
            }
            None => {}
        }
    }

    pub(super) fn run_search(&mut self, query: &str) {
        self.search.query = query.to_string();
        self.search.cursor = 0;
        self.search.scroll = 0;
        if query.is_empty() {
            self.search.hits.clear();
            return;
        }
        let m = match twit_core::search::Matcher::new(query) {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(format!("search failed: {e}"));
                return;
            }
        };
        let filter =
            twit_core::search::SearchFilter::parse(&self.search.include, &self.search.exclude);
        match twit_core::search::search_repo_filtered(&self.selected, &m, &filter) {
            Ok(hits) => {
                self.search.hits = hits;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("search failed: {e}")),
        }
    }

    pub(super) fn run_search_replace(&mut self, replacement: &str) {
        let matcher = match twit_core::search::Matcher::new(&self.search.query) {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(format!("replace failed: {e}"));
                return;
            }
        };
        let paths: Vec<&str> = self.search.hits.iter().map(|h| h.path.as_str()).collect();
        let report =
            twit_core::search::replace_in_files(&self.selected, &paths, &matcher, replacement);
        let mut msg = format!(
            "replaced {} matches in {} files",
            report.matches, report.files
        );
        if !report.failed.is_empty() {
            msg.push_str(&format!("; failed: {}", report.failed.join(", ")));
        }
        self.error = Some(msg);
        let query = self.search.query.clone();
        self.run_search(&query);
        self.refresh();
    }
}
