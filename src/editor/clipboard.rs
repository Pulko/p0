pub struct Clipboard {
    internal: String,
    system: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            internal: String::new(),
            system: arboard::Clipboard::new().ok(),
        }
    }

    pub fn copy(&mut self, text: &str) {
        self.internal = text.to_string();
        if let Some(board) = self.system.as_mut() {
            let _ = board.set_text(text.to_string());
        }
    }

    pub fn paste(&mut self) -> String {
        if let Some(board) = self.system.as_mut() {
            if let Ok(text) = board.get_text() {
                if !text.is_empty() {
                    self.internal = text.clone();
                    return text;
                }
            }
        }
        self.internal.clone()
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}
