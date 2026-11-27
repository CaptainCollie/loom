#[derive(Debug, Default)]
pub(super) struct TextInput {
    text: String,
    cursor: usize,
}

impl TextInput {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn byte_index(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map_or(self.text.len(), |(index, _)| index)
    }

    pub(super) fn insert(&mut self, ch: char) {
        self.text.insert(self.byte_index(), ch);
        self.cursor += 1;
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_index();
        self.move_left();
        let start = self.byte_index();
        self.text.replace_range(start..end, "");
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_right(&mut self) {
        self.cursor = self.cursor.saturating_add(1).min(self.text.chars().count());
    }
}
