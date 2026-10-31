//! Ctrl+Z in the prompt: earlier states of the text, a letter or a whole edit at a time.

/// Steps kept; older ones fall off.
const MAX_STEPS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    /// Text removed: a run of Backspace or Delete undoes in one step.
    Delete,
    /// A letter typed, a paste, a history recall, a completion: a step of its own.
    Other,
}

#[derive(Default)]
pub struct ComposerUndo {
    steps: Vec<(String, usize)>,
    last: Option<Edit>,
}

impl ComposerUndo {
    /// Notes the edit that turned `before` into `after`. An empty prompt starts over: there is
    /// nothing left to take back, and a sent message must not come back.
    pub fn record(&mut self, before: (String, usize), after: &str) {
        if after.is_empty() {
            self.clear();
            return;
        }
        if before.0 == after {
            return;
        }
        let edit = if after.len() < before.0.len() { Edit::Delete } else { Edit::Other };
        if !(edit == Edit::Delete && self.last == Some(Edit::Delete)) {
            if self.steps.len() == MAX_STEPS {
                self.steps.remove(0);
            }
            self.steps.push(before);
        }
        self.last = Some(edit);
    }

    /// The state before the last step, if any is left.
    pub fn undo(&mut self) -> Option<(String, usize)> {
        self.last = None;
        self.steps.pop()
    }

    pub fn clear(&mut self) {
        self.steps.clear();
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_text(undo: &mut ComposerUndo, buffer: &mut String, text: &str) {
        for c in text.chars() {
            let before = (buffer.clone(), buffer.len());
            buffer.push(c);
            undo.record(before, buffer);
        }
    }

    #[test]
    fn typing_undoes_a_letter_at_a_time() {
        let mut undo = ComposerUndo::default();
        let mut buffer = String::new();
        type_text(&mut undo, &mut buffer, "hola");
        assert_eq!(undo.undo().unwrap().0, "hol");
        assert_eq!(undo.undo().unwrap().0, "ho");
        assert_eq!(undo.undo().unwrap().0, "h");
        assert_eq!(undo.undo().unwrap().0, "");
        assert_eq!(undo.undo(), None);
    }

    #[test]
    fn a_paste_and_a_run_of_deletions_are_single_steps() {
        let mut undo = ComposerUndo::default();
        let mut buffer = String::new();
        type_text(&mut undo, &mut buffer, "ver ");
        let before = (buffer.clone(), buffer.len());
        buffer.push_str("/home/me/a/very/long/path");
        undo.record(before, &buffer);
        for _ in 0..4 {
            let before = (buffer.clone(), buffer.len());
            buffer.pop();
            undo.record(before, &buffer);
        }
        assert_eq!(undo.undo().unwrap().0, "ver /home/me/a/very/long/path");
        assert_eq!(undo.undo().unwrap().0, "ver ");
    }

    #[test]
    fn an_emptied_prompt_forgets_its_history() {
        let mut undo = ComposerUndo::default();
        let mut buffer = String::new();
        type_text(&mut undo, &mut buffer, "sent message");
        undo.record((buffer.clone(), buffer.len()), "");
        assert_eq!(undo.undo(), None);
    }
}
