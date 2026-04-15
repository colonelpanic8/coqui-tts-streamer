use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Segment {
    pub id: usize,
    pub paragraph_index: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    text: Arc<str>,
}

impl Segment {
    pub fn new(
        id: usize,
        paragraph_index: usize,
        start_byte: usize,
        end_byte: usize,
        text: String,
    ) -> Self {
        Self {
            id,
            paragraph_index,
            start_byte,
            end_byte,
            text: Arc::<str>::from(text),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }
}
