//! A single-line, character-indexed text editor used by the search bar, the
//! filter field, and the folder prompts. Pure data structure: draws nothing,
//! reads no input device.

/// A single-line editor. The value is held as characters, not bytes, so no
/// operation can split a multi-byte character or panic on an index.
#[derive(Debug, Default, Clone)]
pub struct TextField {
    chars: Vec<char>,
    cursor: usize,
    cache: String,
}

impl TextField {
    /// Creates an empty field.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a field pre-populated with `value`, cursor at the end.
    pub fn with_value(value: &str) -> Self {
        let mut f = Self::new();
        f.set_value(value);
        f
    }

    /// The current value.
    pub fn value(&self) -> &str {
        &self.cache
    }

    /// The cursor position, in characters, always within `0..=len`.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the whole value and moves the cursor to the end.
    pub fn set_value(&mut self, value: &str) {
        self.chars = value.chars().collect();
        self.cursor = self.chars.len();
        self.sync_cache();
    }

    /// Clears the value and resets the cursor.
    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.sync_cache();
    }

    /// Sanitizes and inserts `text` at the cursor, moving the cursor past it.
    pub fn insert(&mut self, text: &str) {
        let clean = sanitize(text);
        if clean.is_empty() {
            return;
        }
        self.clamp_cursor();
        let at = self.cursor;
        for (offset, ch) in clean.chars().enumerate() {
            self.chars.insert(at + offset, ch);
        }
        self.cursor = at + clean.chars().count();
        self.sync_cache();
    }

    /// Deletes the character before the cursor, if any.
    pub fn backspace(&mut self) {
        self.clamp_cursor();
        if self.cursor == 0 {
            return;
        }
        self.chars.remove(self.cursor - 1);
        self.cursor -= 1;
        self.sync_cache();
    }

    /// Deletes the character at the cursor, if any.
    pub fn delete(&mut self) {
        self.clamp_cursor();
        if self.cursor >= self.chars.len() {
            return;
        }
        self.chars.remove(self.cursor);
        self.sync_cache();
    }

    /// Moves the cursor one character left.
    pub fn left(&mut self) {
        self.clamp_cursor();
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Moves the cursor one character right.
    pub fn right(&mut self) {
        self.clamp_cursor();
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    /// Moves the cursor to the start of the value.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end of the value.
    pub fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Moves the cursor left over the run of spaces, then the run of
    /// non-space characters, before it.
    pub fn word_left(&mut self) {
        self.clamp_cursor();
        self.cursor = word_left_index(&self.chars, self.cursor);
    }

    /// Moves the cursor right over any spaces at the cursor, then the run
    /// of non-space characters after them.
    pub fn word_right(&mut self) {
        self.clamp_cursor();
        self.cursor = word_right_index(&self.chars, self.cursor);
    }

    /// Deletes from the start of the previous word up to the cursor.
    pub fn delete_word_before(&mut self) {
        self.clamp_cursor();
        let start = word_left_index(&self.chars, self.cursor);
        if start >= self.cursor {
            return;
        }
        self.chars.drain(start..self.cursor);
        self.cursor = start;
        self.sync_cache();
    }

    /// Deletes from the cursor up to the start of the next word.
    pub fn delete_word_after(&mut self) {
        self.clamp_cursor();
        let end = word_right_index(&self.chars, self.cursor);
        if end <= self.cursor {
            return;
        }
        self.chars.drain(self.cursor..end);
        self.sync_cache();
    }

    /// Deletes from the cursor to the end of the value.
    pub fn kill_to_end(&mut self) {
        self.clamp_cursor();
        if self.cursor >= self.chars.len() {
            return;
        }
        self.chars.truncate(self.cursor);
        self.sync_cache();
    }

    /// Returns the horizontally scrolled slice of the value that fits in
    /// `width` columns, and the cursor's column within that slice.
    ///
    /// Returns `("", 0)` for a zero-width area, which is normal during a
    /// terminal resize rather than an error.
    pub fn view(&self, width: u16) -> (String, usize) {
        let width = width as usize;
        if width == 0 {
            return (String::new(), 0);
        }
        let len = self.chars.len();
        let cursor = self.cursor.min(len);

        // Keep the cursor inside the window, preferring to show one
        // character of trailing context after it when possible.
        let start = (cursor + 1).saturating_sub(width);

        let end = (start + width).min(len);
        let slice: String = self.chars[start..end].iter().collect();
        let col = cursor.saturating_sub(start);
        (slice, col)
    }

    fn clamp_cursor(&mut self) {
        if self.cursor > self.chars.len() {
            self.cursor = self.chars.len();
        }
    }

    fn sync_cache(&mut self) {
        self.cache = self.chars.iter().collect();
    }
}

/// Scans left from `from` over the run of spaces, then the run of
/// non-spaces, returning the resulting index.
fn word_left_index(chars: &[char], from: usize) -> usize {
    let mut i = from.min(chars.len());
    while i > 0 && chars.get(i - 1) == Some(&' ') {
        i -= 1;
    }
    while i > 0 && chars.get(i - 1) != Some(&' ') {
        i -= 1;
    }
    i
}

/// Scans right from `from` over any spaces, then the run of non-spaces,
/// returning the resulting index.
fn word_right_index(chars: &[char], from: usize) -> usize {
    let len = chars.len();
    let mut i = from.min(len);
    while i < len && chars.get(i) == Some(&' ') {
        i += 1;
    }
    while i < len && chars.get(i) != Some(&' ') {
        i += 1;
    }
    i
}

/// Strips bracketed-paste markers, SGR mouse reports, and carriage
/// returns/newlines from pasted or typed text before it ever reaches the
/// value. A paste arrives as one event and may contain anything a hostile
/// web page put on the clipboard.
fn sanitize(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        if c == '\r' || c == '\n' {
            i += 1;
            continue;
        }

        if c == '\u{1b}' {
            // Bracketed-paste markers: ESC [ 200 ~  or  ESC [ 201 ~
            if let Some(len) = match_literal(&chars, i, "\u{1b}[200~") {
                i += len;
                continue;
            }
            if let Some(len) = match_literal(&chars, i, "\u{1b}[201~") {
                i += len;
                continue;
            }
            // SGR mouse report: ESC [ < digits ; digits ; digits (M|m)
            if let Some(len) = match_sgr_mouse_strict(&chars, i) {
                i += len;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }
    out
}

/// If `chars[at..]` starts with the literal `pattern`, returns its length in
/// characters.
fn match_literal(chars: &[char], at: usize, pattern: &str) -> Option<usize> {
    let pat: Vec<char> = pattern.chars().collect();
    if at + pat.len() > chars.len() {
        return None;
    }
    if chars[at..at + pat.len()] == pat[..] {
        Some(pat.len())
    } else {
        None
    }
}

/// If `chars[at..]` starts with an SGR mouse report (`ESC [ < digits ;
/// digits ; digits (M|m)`), returns its length in characters.
fn match_sgr_mouse_strict(chars: &[char], at: usize) -> Option<usize> {
    let prefix: Vec<char> = "\u{1b}[<".chars().collect();
    if at + prefix.len() > chars.len() || chars[at..at + prefix.len()] != prefix[..] {
        return None;
    }
    let mut i = at + prefix.len();

    let read_digits = |i: &mut usize| -> bool {
        let start = *i;
        while *i < chars.len() && chars[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };

    if !read_digits(&mut i) {
        return None;
    }
    if chars.get(i) != Some(&';') {
        return None;
    }
    i += 1;
    if !read_digits(&mut i) {
        return None;
    }
    if chars.get(i) != Some(&';') {
        return None;
    }
    i += 1;
    if !read_digits(&mut i) {
        return None;
    }
    match chars.get(i) {
        Some('M') | Some('m') => Some(i + 1 - at),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_inserts_at_the_cursor_and_moves_it_along() {
        let mut f = TextField::new();
        f.insert("abc");
        assert_eq!(f.value(), "abc");
        assert_eq!(f.cursor(), 3);
        f.left();
        f.insert("X");
        assert_eq!(f.value(), "abXc");
        assert_eq!(f.cursor(), 3);
    }

    #[test]
    fn the_cursor_never_leaves_the_string() {
        let mut f = TextField::with_value("ab");
        f.home();
        f.left();
        assert_eq!(f.cursor(), 0);
        f.end();
        f.right();
        assert_eq!(f.cursor(), 2);
    }

    #[test]
    fn backspace_and_delete_are_no_ops_at_the_edges() {
        let mut f = TextField::with_value("ab");
        f.home();
        f.backspace();
        assert_eq!(f.value(), "ab");
        f.end();
        f.delete();
        assert_eq!(f.value(), "ab");
    }

    #[test]
    fn word_motions_treat_runs_of_spaces_as_the_boundary() {
        let mut f = TextField::with_value("one two three");
        f.end();
        f.word_left();
        assert_eq!(f.cursor(), 8);
        f.word_left();
        assert_eq!(f.cursor(), 4);
        f.home();
        f.word_right();
        assert_eq!(f.cursor(), 3);
    }

    #[test]
    fn deleting_a_word_before_removes_the_run_and_its_leading_spaces() {
        let mut f = TextField::with_value("one two  ");
        f.end();
        f.delete_word_before();
        assert_eq!(f.value(), "one ");
    }

    #[test]
    fn deleting_a_word_after_removes_the_following_run() {
        let mut f = TextField::with_value("one two");
        f.home();
        f.delete_word_after();
        assert_eq!(f.value(), " two");
    }

    #[test]
    fn kill_to_end_and_clear_do_what_they_say() {
        let mut f = TextField::with_value("hello world");
        f.home();
        f.word_right();
        f.kill_to_end();
        // word_right from 0 in "hello world" skips the non-space run and
        // lands at 5 (the space); killing to end keeps "hello".
        assert_eq!(f.value(), "hello");
        f.clear();
        assert_eq!(f.value(), "");
        assert_eq!(f.cursor(), 0);
    }

    #[test]
    fn pasted_text_is_stripped_of_control_sequences() {
        // A paste arrives as one event. Bracketed-paste markers, SGR mouse
        // reports, and newlines must never land in the value — they would be
        // sent to a source as part of the query.
        let mut f = TextField::new();
        f.insert("\u{1b}[200~ubuntu\r\n22.04\u{1b}[201~");
        assert_eq!(f.value(), "ubuntu22.04");
        let mut g = TextField::new();
        g.insert("\u{1b}[<0;12;7M hi");
        assert_eq!(g.value(), " hi");
    }

    #[test]
    fn multibyte_input_is_edited_by_character_not_byte() {
        // Indexing a multi-byte string by byte offset panics. Every cursor
        // operation must be character-based.
        let mut f = TextField::with_value("héllo");
        f.home();
        f.right();
        f.delete();
        assert_eq!(f.value(), "hllo");
        let mut g = TextField::with_value("日本語");
        g.end();
        g.backspace();
        assert_eq!(g.value(), "日本");
    }

    #[test]
    fn the_view_scrolls_to_keep_the_cursor_visible() {
        let mut f = TextField::with_value("abcdefghij");
        f.end();
        let (text, col) = f.view(5);
        assert!(text.chars().count() <= 5);
        assert!(col < 5, "cursor column {col} is outside the view");
        f.home();
        let (text, col) = f.view(5);
        assert!(text.starts_with('a'));
        assert_eq!(col, 0);
    }

    #[test]
    fn a_zero_width_view_returns_empty_instead_of_panicking() {
        let f = TextField::with_value("abc");
        let (text, col) = f.view(0);
        assert_eq!(text, "");
        assert_eq!(col, 0);
    }
}
