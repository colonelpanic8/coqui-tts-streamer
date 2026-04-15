use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Document {
    title: Option<String>,
    text: Arc<str>,
}

impl Document {
    pub fn new(title: Option<String>, text: String) -> Self {
        Self {
            title,
            text: Arc::<str>::from(text),
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}
