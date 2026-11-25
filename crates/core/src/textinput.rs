#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputMode {
    Insert,
    Normal,
    Visual,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputKey {
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Backspace,
    Delete,
    Enter,
    Esc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct InputMods {
    pub ctrl: bool,
    pub alt: bool,
}

impl InputMods {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        alt: false,
    };
    pub const ALT: Self = Self {
        ctrl: false,
        alt: true,
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputOutcome {
    Continue,
    Submit,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Operator {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FindKind {
    To,
    Till,
    BackTo,
    BackTill,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Awaiting {
    None,
    Find(FindKind),
    Replace,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Snapshot {
    chars: Vec<char>,
    cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

#[derive(Clone)]
pub struct TextInput {
    chars: Vec<char>,
    cursor: usize,
    mode: InputMode,
    vim: bool,
    count: Option<usize>,
    operator: Option<Operator>,
    awaiting: Awaiting,
    register: Vec<char>,
    last_find: Option<(FindKind, char)>,
    anchor: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    dirty: bool,
}

impl TextInput {
    pub fn new(text: impl Into<String>, vim: bool) -> Self {
        let chars: Vec<char> = text.into().chars().collect();
        let cursor = chars.len();
        Self {
            chars,
            cursor,
            mode: InputMode::Insert,
            vim,
            count: None,
            operator: None,
            awaiting: Awaiting::None,
            register: Vec::new(),
            last_find: None,
            anchor: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn vim(&self) -> bool {
        self.vim
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.mode == InputMode::Visual).then(|| {
            let lo = self.anchor.min(self.cursor);
            let hi = self.anchor.max(self.cursor);
            (lo, (hi + 1).min(self.chars.len()))
        })
    }

    pub fn handle(&mut self, key: InputKey, mods: InputMods) -> InputOutcome {
        if mods.ctrl && key == InputKey::Char('c') {
            return InputOutcome::Cancel;
        }
        if key == InputKey::Enter {
            return InputOutcome::Submit;
        }
        match self.mode {
            InputMode::Insert => self.insert_key(key, mods),
            InputMode::Normal | InputMode::Visual => self.normal_key(key, mods),
        }
    }

    fn insert_key(&mut self, key: InputKey, mods: InputMods) -> InputOutcome {
        if mods.ctrl {
            match key {
                InputKey::Char('a') => self.cursor = 0,
                InputKey::Char('e') => self.cursor = self.chars.len(),
                InputKey::Char('b') => self.cursor = self.cursor.saturating_sub(1),
                InputKey::Char('f') => self.cursor = (self.cursor + 1).min(self.chars.len()),
                InputKey::Char('h') => self.backspace(),
                InputKey::Char('d') => self.delete_forward(),
                InputKey::Char('w') => {
                    let to = self.prev_word_start();
                    self.cut(to, self.cursor, true);
                }
                InputKey::Char('u') => self.cut(0, self.cursor, true),
                InputKey::Char('k') => self.cut(self.cursor, self.chars.len(), true),
                InputKey::Left => self.cursor = self.prev_word_start(),
                InputKey::Right => self.cursor = self.next_word_start(),
                _ => {}
            }
            return InputOutcome::Continue;
        }
        if mods.alt {
            match key {
                InputKey::Char('b') => self.cursor = self.prev_word_start(),
                InputKey::Char('f') => self.cursor = self.next_word_end(),
                InputKey::Char('d') => {
                    let to = self.next_word_end();
                    self.cut(self.cursor, to, true);
                }
                InputKey::Backspace => {
                    let to = self.prev_word_start();
                    self.cut(to, self.cursor, true);
                }
                _ => {}
            }
            return InputOutcome::Continue;
        }
        match key {
            InputKey::Esc => {
                if self.vim {
                    self.finish_insert();
                } else {
                    return InputOutcome::Cancel;
                }
            }
            InputKey::Char(c) => {
                self.mark_change();
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            InputKey::Backspace => self.backspace(),
            InputKey::Delete => self.delete_forward(),
            InputKey::Left => self.cursor = self.cursor.saturating_sub(1),
            InputKey::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            InputKey::Home | InputKey::Up => self.cursor = 0,
            InputKey::End | InputKey::Down => self.cursor = self.chars.len(),
            InputKey::Enter => {}
        }
        InputOutcome::Continue
    }

    fn normal_key(&mut self, key: InputKey, mods: InputMods) -> InputOutcome {
        if mods.ctrl {
            if key == InputKey::Char('r') {
                self.redo();
            }
            return InputOutcome::Continue;
        }
        if mods.alt {
            return InputOutcome::Continue;
        }
        match self.awaiting {
            Awaiting::Find(kind) => {
                self.awaiting = Awaiting::None;
                if let InputKey::Char(c) = key {
                    self.last_find = Some((kind, c));
                    self.apply_find(kind, c);
                } else {
                    self.reset_pending();
                }
                return InputOutcome::Continue;
            }
            Awaiting::Replace => {
                self.awaiting = Awaiting::None;
                if let InputKey::Char(c) = key {
                    self.replace_chars(c);
                }
                self.reset_pending();
                return InputOutcome::Continue;
            }
            Awaiting::None => {}
        }

        let key = match key {
            InputKey::Left => InputKey::Char('h'),
            InputKey::Right => InputKey::Char('l'),
            InputKey::Home | InputKey::Up => InputKey::Char('0'),
            InputKey::End | InputKey::Down => InputKey::Char('$'),
            other => other,
        };

        let InputKey::Char(c) = key else {
            match key {
                InputKey::Esc => {
                    if self.operator.is_some() || self.count.is_some() {
                        self.reset_pending();
                    } else if self.mode == InputMode::Visual {
                        self.mode = InputMode::Normal;
                    } else {
                        return InputOutcome::Cancel;
                    }
                }
                InputKey::Backspace => self.move_by(-1),
                InputKey::Delete => self.delete_under_cursor(),
                _ => {}
            }
            return InputOutcome::Continue;
        };

        if c.is_ascii_digit() && !(c == '0' && self.count.is_none()) {
            let d = c as usize - '0' as usize;
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
            return InputOutcome::Continue;
        }

        if let Some(op) = self.operator {
            self.operator_key(op, c);
            return InputOutcome::Continue;
        }

        match c {
            'f' => {
                self.awaiting = Awaiting::Find(FindKind::To);
                return InputOutcome::Continue;
            }
            'F' => {
                self.awaiting = Awaiting::Find(FindKind::BackTo);
                return InputOutcome::Continue;
            }
            't' => {
                self.awaiting = Awaiting::Find(FindKind::Till);
                return InputOutcome::Continue;
            }
            'T' => {
                self.awaiting = Awaiting::Find(FindKind::BackTill);
                return InputOutcome::Continue;
            }
            ';' => {
                if let Some((kind, ch)) = self.last_find {
                    self.repeat_find(kind, ch);
                }
                return InputOutcome::Continue;
            }
            ',' => {
                if let Some((kind, ch)) = self.last_find {
                    self.repeat_find(flip_find(kind), ch);
                }
                return InputOutcome::Continue;
            }
            _ => {}
        }

        let count = self.take_count();
        match c {
            'h' => self.move_by(-(count as isize)),
            'l' => self.move_by(count as isize),
            '0' => self.cursor = 0,
            '^' => self.cursor = self.first_non_blank(),
            '$' => self.cursor = self.last_index(),
            'w' => self.repeat(count, |s| s.cursor = s.next_word_start()),
            'W' => self.repeat(count, |s| s.cursor = s.next_big_word_start()),
            'b' => self.repeat(count, |s| s.cursor = s.prev_word_start()),
            'B' => self.repeat(count, |s| s.cursor = s.prev_big_word_start()),
            'e' => self.repeat(count, |s| s.cursor = s.next_word_end_vim()),
            'E' => self.repeat(count, |s| s.cursor = s.next_big_word_end()),
            '|' => self.cursor = (count - 1).min(self.last_index()),
            'i' => self.enter_insert(),
            'a' => {
                self.cursor = (self.cursor + 1).min(self.chars.len());
                self.enter_insert();
            }
            'I' => {
                self.cursor = self.first_non_blank();
                self.enter_insert();
            }
            'A' => {
                self.cursor = self.chars.len();
                self.enter_insert();
            }
            'v' => {
                if self.mode == InputMode::Visual {
                    self.mode = InputMode::Normal;
                } else {
                    self.anchor = self.cursor;
                    self.mode = InputMode::Visual;
                }
            }
            'd' | 'c' | 'y' | 'x' | 'X' | 's' | 'D' | 'C' | 'Y' | 'S' | 'r' | '~' | 'p' | 'P'
            | 'u' => self.edit_key(c, count),
            'g' => {}
            _ => {}
        }
        InputOutcome::Continue
    }

    fn edit_key(&mut self, c: char, count: usize) {
        if self.mode == InputMode::Visual {
            let Some((lo, hi)) = self.selection() else {
                return;
            };
            match c {
                'd' | 'x' => {
                    self.cut(lo, hi, true);
                    self.mode = InputMode::Normal;
                    self.clamp_normal();
                }
                'c' | 's' => {
                    self.cut(lo, hi, true);
                    self.mode = InputMode::Normal;
                    self.enter_insert();
                }
                'y' => {
                    self.register = self.chars[lo..hi].to_vec();
                    self.cursor = lo;
                    self.mode = InputMode::Normal;
                }
                'D' | 'X' | 'C' | 'S' => {
                    self.cut(0, self.chars.len(), true);
                    self.mode = InputMode::Normal;
                    if c == 'C' || c == 'S' {
                        self.enter_insert();
                    }
                }
                'Y' => {
                    self.register = self.chars[lo..hi].to_vec();
                    self.mode = InputMode::Normal;
                }
                'r' => self.awaiting = Awaiting::Replace,
                '~' => {
                    self.mark_change();
                    for i in lo..hi {
                        self.chars[i] = toggle_case(self.chars[i]);
                    }
                    self.cursor = lo;
                    self.mode = InputMode::Normal;
                    self.clamp_normal();
                }
                'p' | 'P' => {
                    let reg = std::mem::take(&mut self.register);
                    self.cut(lo, hi, true);
                    self.register = reg.clone();
                    self.mark_change();
                    for (i, ch) in reg.into_iter().enumerate() {
                        self.chars.insert(self.cursor + i, ch);
                    }
                    self.mode = InputMode::Normal;
                    self.clamp_normal();
                }
                'u' => {
                    self.mode = InputMode::Normal;
                    self.undo();
                }
                _ => {}
            }
            return;
        }
        match c {
            'x' => {
                let to = (self.cursor + count).min(self.chars.len());
                self.cut(self.cursor, to, true);
                self.clamp_normal();
            }
            'X' => {
                let from = self.cursor.saturating_sub(count);
                self.cut(from, self.cursor, true);
            }
            's' => {
                let to = (self.cursor + count).min(self.chars.len());
                self.cut(self.cursor, to, true);
                self.enter_insert();
            }
            'D' => {
                self.cut(self.cursor, self.chars.len(), true);
                self.clamp_normal();
            }
            'C' => {
                self.cut(self.cursor, self.chars.len(), true);
                self.enter_insert();
            }
            'Y' => self.register = self.chars[self.cursor..].to_vec(),
            'S' => {
                self.cut(0, self.chars.len(), true);
                self.enter_insert();
            }
            'r' => self.awaiting = Awaiting::Replace,
            '~' => {
                if self.cursor < self.chars.len() {
                    self.mark_change();
                    let end = (self.cursor + count).min(self.chars.len());
                    for i in self.cursor..end {
                        self.chars[i] = toggle_case(self.chars[i]);
                    }
                    self.cursor = end.min(self.last_index());
                }
            }
            'p' | 'P' => {
                if !self.register.is_empty() {
                    self.mark_change();
                    let at = if c == 'p' && !self.chars.is_empty() {
                        (self.cursor + 1).min(self.chars.len())
                    } else {
                        self.cursor
                    };
                    let reg = self.register.clone();
                    let n = reg.len();
                    for (i, ch) in reg.into_iter().enumerate() {
                        self.chars.insert(at + i, ch);
                    }
                    self.cursor = at + n - 1;
                }
            }
            'u' => self.undo(),
            'd' | 'c' | 'y' => {
                self.operator = Some(match c {
                    'd' => Operator::Delete,
                    'c' => Operator::Change,
                    _ => Operator::Yank,
                });
                if count > 1 {
                    self.count = Some(count);
                }
            }
            _ => {}
        }
    }

    fn operator_key(&mut self, op: Operator, c: char) {
        if c == 'g' {
            return;
        }
        let doubled = matches!(
            (op, c),
            (Operator::Delete, 'd') | (Operator::Change, 'c') | (Operator::Yank, 'y')
        );
        if doubled {
            self.reset_pending();
            match op {
                Operator::Delete => {
                    self.cut(0, self.chars.len(), true);
                }
                Operator::Change => {
                    self.cut(0, self.chars.len(), true);
                    self.enter_insert();
                }
                Operator::Yank => {
                    self.register = self.chars.clone();
                    self.cursor = 0;
                }
            }
            return;
        }
        if matches!(c, 'f' | 'F' | 't' | 'T') {
            self.awaiting = Awaiting::Find(match c {
                'f' => FindKind::To,
                'F' => FindKind::BackTo,
                't' => FindKind::Till,
                _ => FindKind::BackTill,
            });
            return;
        }
        let count = self.take_count();
        let Some((lo, hi)) = self.motion_range(c, count) else {
            self.reset_pending();
            return;
        };
        self.reset_pending();
        self.apply_operator(op, lo, hi);
    }

    fn apply_operator(&mut self, op: Operator, lo: usize, hi: usize) {
        match op {
            Operator::Delete => {
                self.cut(lo, hi, true);
                self.clamp_normal();
            }
            Operator::Change => {
                self.cut(lo, hi, true);
                self.enter_insert();
            }
            Operator::Yank => {
                self.register = self.chars[lo..hi].to_vec();
                self.cursor = lo;
            }
        }
    }

    fn motion_range(&mut self, c: char, count: usize) -> Option<(usize, usize)> {
        let start = self.cursor;
        let target = match c {
            'h' => start.saturating_sub(count),
            'l' => (start + count).min(self.chars.len()),
            '0' => 0,
            '^' => self.first_non_blank(),
            '$' => self.chars.len(),
            'w' => {
                let mut p = start;
                for _ in 0..count {
                    p = self.word_start_from(p, false);
                }
                p
            }
            'W' => {
                let mut p = start;
                for _ in 0..count {
                    p = self.word_start_from(p, true);
                }
                p
            }
            'b' => {
                let mut p = start;
                for _ in 0..count {
                    p = self.word_back_from(p, false);
                }
                p
            }
            'B' => {
                let mut p = start;
                for _ in 0..count {
                    p = self.word_back_from(p, true);
                }
                p
            }
            'e' => {
                let mut p = start;
                for _ in 0..count {
                    p = self.word_end_from(p, false);
                }
                (p + 1).min(self.chars.len())
            }
            'E' => {
                let mut p = start;
                for _ in 0..count {
                    p = self.word_end_from(p, true);
                }
                (p + 1).min(self.chars.len())
            }
            _ => return None,
        };
        if target >= start {
            Some((start, target))
        } else {
            Some((target, start))
        }
    }

    fn repeat_find(&mut self, kind: FindKind, c: char) {
        self.apply_find_inner(kind, c, true);
    }

    fn apply_find(&mut self, kind: FindKind, c: char) {
        self.apply_find_inner(kind, c, false);
    }

    fn apply_find_inner(&mut self, kind: FindKind, c: char, repeat: bool) {
        let count = self.take_count();
        let op = self.operator;
        let start = self.cursor;
        let mut pos = self.cursor;
        for i in 0..count {
            let Some(next) = self.find_from(pos, kind, c, repeat || i > 0) else {
                self.reset_pending();
                return;
            };
            pos = next;
        }
        self.reset_pending();
        match op {
            None => self.cursor = pos,
            Some(op) => {
                let forward = matches!(kind, FindKind::To | FindKind::Till);
                if forward {
                    self.apply_operator(op, start, (pos + 1).min(self.chars.len()));
                } else {
                    self.apply_operator(op, pos, start);
                }
            }
        }
    }

    fn find_from(&self, from: usize, kind: FindKind, c: char, repeat: bool) -> Option<usize> {
        let skip = usize::from(repeat);
        match kind {
            FindKind::To => (from + 1..self.chars.len()).find(|&i| self.chars[i] == c),
            FindKind::Till => (from + 1 + skip..self.chars.len())
                .find(|&i| self.chars[i] == c)
                .map(|i| i - 1)
                .filter(|&i| i != from),
            FindKind::BackTo => (0..from).rev().find(|&i| self.chars[i] == c),
            FindKind::BackTill => (0..from.saturating_sub(skip))
                .rev()
                .find(|&i| self.chars[i] == c)
                .map(|i| i + 1)
                .filter(|&i| i != from),
        }
    }

    fn replace_chars(&mut self, c: char) {
        if let Some((lo, hi)) = self.selection() {
            self.mark_change();
            for i in lo..hi {
                self.chars[i] = c;
            }
            self.cursor = lo;
            self.mode = InputMode::Normal;
            self.clamp_normal();
            return;
        }
        let count = self.take_count();
        if self.cursor + count > self.chars.len() {
            return;
        }
        self.mark_change();
        for i in self.cursor..self.cursor + count {
            self.chars[i] = c;
        }
        self.cursor = (self.cursor + count - 1).min(self.last_index());
    }

    fn enter_insert(&mut self) {
        self.mode = InputMode::Insert;
        self.dirty = false;
    }

    fn finish_insert(&mut self) {
        self.mode = InputMode::Normal;
        self.dirty = false;
        self.clamp_normal();
    }

    fn clamp_normal(&mut self) {
        self.cursor = self.cursor.min(self.last_index());
    }

    fn last_index(&self) -> usize {
        self.chars.len().saturating_sub(1)
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }

    fn reset_pending(&mut self) {
        self.count = None;
        self.operator = None;
        self.awaiting = Awaiting::None;
    }

    fn repeat(&mut self, count: usize, f: impl Fn(&mut Self)) {
        for _ in 0..count {
            f(self);
        }
        if self.mode != InputMode::Insert {
            self.clamp_normal();
        }
    }

    fn move_by(&mut self, delta: isize) {
        let max = if self.mode == InputMode::Insert {
            self.chars.len()
        } else {
            self.last_index()
        };
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, max as isize) as usize;
    }

    fn mark_change(&mut self) {
        if self.mode == InputMode::Insert && self.dirty {
            return;
        }
        self.undo.push(Snapshot {
            chars: self.chars.clone(),
            cursor: self.cursor,
        });
        self.redo.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        let Some(prev) = self.undo.pop() else {
            return;
        };
        self.redo.push(Snapshot {
            chars: self.chars.clone(),
            cursor: self.cursor,
        });
        self.chars = prev.chars;
        self.cursor = prev.cursor;
        self.clamp_normal();
    }

    fn redo(&mut self) {
        let Some(next) = self.redo.pop() else {
            return;
        };
        self.undo.push(Snapshot {
            chars: self.chars.clone(),
            cursor: self.cursor,
        });
        self.chars = next.chars;
        self.cursor = next.cursor;
        self.clamp_normal();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.mark_change();
        self.cursor -= 1;
        self.chars.remove(self.cursor);
    }

    fn delete_forward(&mut self) {
        if self.cursor >= self.chars.len() {
            return;
        }
        self.mark_change();
        self.chars.remove(self.cursor);
    }

    fn delete_under_cursor(&mut self) {
        self.delete_forward();
        self.clamp_normal();
    }

    fn cut(&mut self, lo: usize, hi: usize, yank: bool) {
        let lo = lo.min(self.chars.len());
        let hi = hi.min(self.chars.len());
        if lo >= hi {
            return;
        }
        self.mark_change();
        let removed: Vec<char> = self.chars.drain(lo..hi).collect();
        if yank {
            self.register = removed;
        }
        self.cursor = lo;
    }

    fn first_non_blank(&self) -> usize {
        self.chars
            .iter()
            .position(|c| !c.is_whitespace())
            .unwrap_or(0)
    }

    fn next_word_start(&self) -> usize {
        self.word_start_from(self.cursor, false)
    }

    fn next_big_word_start(&self) -> usize {
        self.word_start_from(self.cursor, true)
    }

    fn prev_word_start(&self) -> usize {
        self.word_back_from(self.cursor, false)
    }

    fn prev_big_word_start(&self) -> usize {
        self.word_back_from(self.cursor, true)
    }

    fn next_word_end_vim(&self) -> usize {
        self.word_end_from(self.cursor, false)
    }

    fn next_big_word_end(&self) -> usize {
        self.word_end_from(self.cursor, true)
    }

    fn next_word_end(&self) -> usize {
        (self.word_end_from(self.cursor, false) + 1).min(self.chars.len())
    }

    fn word_start_from(&self, from: usize, big: bool) -> usize {
        let n = self.chars.len();
        let mut i = from;
        if i >= n {
            return n;
        }
        let start = self.class(i, big);
        if start != Class::Space {
            while i < n && self.class(i, big) == start {
                i += 1;
            }
        }
        while i < n && self.class(i, big) == Class::Space {
            i += 1;
        }
        i
    }

    fn word_back_from(&self, from: usize, big: bool) -> usize {
        let mut i = from;
        if i == 0 {
            return 0;
        }
        i -= 1;
        while i > 0 && self.class(i, big) == Class::Space {
            i -= 1;
        }
        if self.class(i, big) == Class::Space {
            return i;
        }
        let cls = self.class(i, big);
        while i > 0 && self.class(i - 1, big) == cls {
            i -= 1;
        }
        i
    }

    fn word_end_from(&self, from: usize, big: bool) -> usize {
        let n = self.chars.len();
        if n == 0 {
            return 0;
        }
        let mut i = (from + 1).min(n.saturating_sub(1));
        if from + 1 >= n {
            return n.saturating_sub(1);
        }
        while i < n && self.class(i, big) == Class::Space {
            i += 1;
        }
        if i >= n {
            return n - 1;
        }
        let cls = self.class(i, big);
        while i + 1 < n && self.class(i + 1, big) == cls {
            i += 1;
        }
        i
    }

    fn class(&self, i: usize, big: bool) -> Class {
        let c = self.chars.get(i).copied().unwrap_or(' ');
        match (class_of(c), big) {
            (Class::Space, _) => Class::Space,
            (_, true) => Class::Word,
            (cls, false) => cls,
        }
    }
}

fn flip_find(kind: FindKind) -> FindKind {
    match kind {
        FindKind::To => FindKind::BackTo,
        FindKind::BackTo => FindKind::To,
        FindKind::Till => FindKind::BackTill,
        FindKind::BackTill => FindKind::Till,
    }
}

fn toggle_case(c: char) -> char {
    if c.is_lowercase() {
        c.to_uppercase().next().unwrap_or(c)
    } else if c.is_uppercase() {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vim(text: &str) -> TextInput {
        TextInput::new(text, true)
    }

    fn feed(input: &mut TextInput, keys: &str) {
        for c in keys.chars() {
            input.handle(InputKey::Char(c), InputMods::NONE);
        }
    }

    fn esc(input: &mut TextInput) {
        input.handle(InputKey::Esc, InputMods::NONE);
    }

    #[test]
    fn arrows_move_the_cursor_and_edit_in_place() {
        let mut input = vim("abcd");
        input.handle(InputKey::Left, InputMods::NONE);
        input.handle(InputKey::Left, InputMods::NONE);
        input.handle(InputKey::Backspace, InputMods::NONE);
        assert_eq!(input.text(), "acd");
        assert_eq!(input.cursor(), 1);
        input.handle(InputKey::Char('X'), InputMods::NONE);
        assert_eq!(input.text(), "aXcd");
        input.handle(InputKey::Delete, InputMods::NONE);
        assert_eq!(input.text(), "aXd");
        input.handle(InputKey::Home, InputMods::NONE);
        assert_eq!(input.cursor(), 0);
        input.handle(InputKey::End, InputMods::NONE);
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn emacs_bindings_work_in_insert_mode() {
        let mut input = vim("feat: add the thing");
        input.handle(InputKey::Char('w'), InputMods::CTRL);
        assert_eq!(input.text(), "feat: add the ");
        input.handle(InputKey::Char('a'), InputMods::CTRL);
        assert_eq!(input.cursor(), 0);
        input.handle(InputKey::Char('k'), InputMods::CTRL);
        assert_eq!(input.text(), "");
    }

    #[test]
    fn esc_cancels_without_vim_mode() {
        let mut input = TextInput::new("x", false);
        assert_eq!(
            input.handle(InputKey::Esc, InputMods::NONE),
            InputOutcome::Cancel
        );
    }

    #[test]
    fn esc_enters_normal_mode_then_cancels() {
        let mut input = vim("x");
        assert_eq!(
            input.handle(InputKey::Esc, InputMods::NONE),
            InputOutcome::Continue
        );
        assert_eq!(input.mode(), InputMode::Normal);
        assert_eq!(
            input.handle(InputKey::Esc, InputMods::NONE),
            InputOutcome::Cancel
        );
    }

    #[test]
    fn ctrl_c_always_cancels() {
        let mut input = vim("x");
        assert_eq!(
            input.handle(InputKey::Char('c'), InputMods::CTRL),
            InputOutcome::Cancel
        );
    }

    #[test]
    fn enter_submits_from_either_mode() {
        let mut input = vim("x");
        assert_eq!(
            input.handle(InputKey::Enter, InputMods::NONE),
            InputOutcome::Submit
        );
        esc(&mut input);
        assert_eq!(
            input.handle(InputKey::Enter, InputMods::NONE),
            InputOutcome::Submit
        );
    }

    #[test]
    fn normal_mode_motions() {
        let mut input = vim("feat: add a thing");
        esc(&mut input);
        assert_eq!(input.cursor(), 16);
        feed(&mut input, "0");
        assert_eq!(input.cursor(), 0);
        feed(&mut input, "w");
        assert_eq!(input.cursor(), 4);
        feed(&mut input, "w");
        assert_eq!(input.cursor(), 6);
        feed(&mut input, "e");
        assert_eq!(input.cursor(), 8);
        feed(&mut input, "b");
        assert_eq!(input.cursor(), 6);
        feed(&mut input, "$");
        assert_eq!(input.cursor(), 16);
        feed(&mut input, "^");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn counts_scale_motions() {
        let mut input = vim("abcdefgh");
        esc(&mut input);
        feed(&mut input, "03l");
        assert_eq!(input.cursor(), 3);
        feed(&mut input, "2h");
        assert_eq!(input.cursor(), 1);
    }

    #[test]
    fn delete_word_and_paste() {
        let mut input = vim("one two three");
        esc(&mut input);
        feed(&mut input, "0dw");
        assert_eq!(input.text(), "two three");
        feed(&mut input, "$p");
        assert_eq!(input.text(), "two threeone ");
    }

    #[test]
    fn change_to_end_enters_insert() {
        let mut input = vim("hello world");
        esc(&mut input);
        feed(&mut input, "0wC");
        assert_eq!(input.text(), "hello ");
        assert_eq!(input.mode(), InputMode::Insert);
        feed(&mut input, "vim");
        assert_eq!(input.text(), "hello vim");
    }

    #[test]
    fn find_char_moves_and_deletes() {
        let mut input = vim("a/b/c");
        esc(&mut input);
        feed(&mut input, "0f/");
        assert_eq!(input.cursor(), 1);
        feed(&mut input, ";");
        assert_eq!(input.cursor(), 3);
        feed(&mut input, "0df/");
        assert_eq!(input.text(), "b/c");
    }

    #[test]
    fn till_does_not_move_onto_an_adjacent_match() {
        let mut input = vim("a/b/c");
        esc(&mut input);
        feed(&mut input, "0t/");
        assert_eq!(input.cursor(), 0);
        feed(&mut input, ";");
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn dd_clears_the_line_and_undo_restores_it() {
        let mut input = vim("throwaway");
        esc(&mut input);
        feed(&mut input, "dd");
        assert_eq!(input.text(), "");
        feed(&mut input, "u");
        assert_eq!(input.text(), "throwaway");
        input.handle(InputKey::Char('r'), InputMods::CTRL);
        assert_eq!(input.text(), "");
    }

    #[test]
    fn insert_commands_position_the_cursor() {
        let mut input = vim("bcd");
        esc(&mut input);
        feed(&mut input, "0i");
        feed(&mut input, "a");
        assert_eq!(input.text(), "abcd");
        esc(&mut input);
        feed(&mut input, "A");
        feed(&mut input, "e");
        assert_eq!(input.text(), "abcde");
    }

    #[test]
    fn visual_mode_deletes_the_selection() {
        let mut input = vim("abcdef");
        esc(&mut input);
        feed(&mut input, "0vlld");
        assert_eq!(input.text(), "def");
        assert_eq!(input.mode(), InputMode::Normal);
    }

    #[test]
    fn visual_selection_covers_the_cursor_cell() {
        let mut input = vim("abcdef");
        esc(&mut input);
        feed(&mut input, "0vl");
        assert_eq!(input.selection(), Some((0, 2)));
    }

    #[test]
    fn replace_char_and_toggle_case() {
        let mut input = vim("abc");
        esc(&mut input);
        feed(&mut input, "0rx");
        assert_eq!(input.text(), "xbc");
        feed(&mut input, "0~");
        assert_eq!(input.text(), "Xbc");
    }

    #[test]
    fn x_deletes_under_cursor_and_clamps() {
        let mut input = vim("ab");
        esc(&mut input);
        assert_eq!(input.cursor(), 1);
        feed(&mut input, "x");
        assert_eq!(input.text(), "a");
        assert_eq!(input.cursor(), 0);
        feed(&mut input, "x");
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn insert_edits_collapse_into_one_undo_step() {
        let mut input = vim("");
        feed(&mut input, "abc");
        esc(&mut input);
        feed(&mut input, "u");
        assert_eq!(input.text(), "");
    }

    #[test]
    fn multibyte_text_is_indexed_by_char() {
        let mut input = vim("日本語テキスト");
        input.handle(InputKey::Left, InputMods::NONE);
        input.handle(InputKey::Backspace, InputMods::NONE);
        assert_eq!(input.text(), "日本語テキト");
        esc(&mut input);
        feed(&mut input, "0x");
        assert_eq!(input.text(), "本語テキト");
    }
}
