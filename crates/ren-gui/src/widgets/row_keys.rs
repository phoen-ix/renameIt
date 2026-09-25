//! Stable identities for the rows of an editable list.
//!
//! Widget state — the cursor in a text box, and its undo history — is keyed by
//! id, and an id that is the row's *position* hands row 4's state to whatever
//! lands in slot 4 after row 3 is deleted: click into the row that moved up and
//! press Ctrl+Z, and the deleted row's last edit is replayed into it (P37's
//! hazard, one level down from the cards). So each row gets a key that travels
//! with it, kept in egui's temp store beside the list's other scratch state and
//! permuted alongside the values.
//!
//! A list whose length no longer matches the keys was changed by somebody else
//! — a preset load, *Restore defaults*, the app tidying blank rows — and is
//! simply re-keyed, with keys after every one ever handed out, so no new row
//! can inherit an old row's state either.

/// The keys for one list, loaded for a frame and stored back after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowKeys {
    keys: Vec<u64>,
}

impl RowKeys {
    /// The keys stored under `id`, re-keyed if they no longer fit `len` rows.
    pub fn load(ui: &egui::Ui, id: egui::Id, len: usize) -> Self {
        let keys: Vec<u64> = ui.data_mut(|d| d.get_temp(id).unwrap_or_default());
        Self { keys }.fitted(len)
    }

    /// Puts them back for the next frame.
    pub fn store(self, ui: &egui::Ui, id: egui::Id) {
        ui.data_mut(|d| d.insert_temp(id, self.keys));
    }

    fn fitted(mut self, len: usize) -> Self {
        if self.keys.len() != len {
            let from = self.next();
            self.keys = (from..from + len as u64).collect();
        }
        self
    }

    /// One past the largest key ever handed out in this list.
    fn next(&self) -> u64 {
        self.keys.iter().max().map_or(0, |k| k + 1)
    }

    /// Row `index`'s key, for `ui.push_id`.
    pub fn get(&self, index: usize) -> u64 {
        self.keys[index]
    }

    pub fn remove(&mut self, index: usize) {
        self.keys.remove(index);
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        self.keys.swap(a, b);
    }

    /// Fresh keys for rows appended until there are `len`.
    pub fn grow_to(&mut self, len: usize) {
        let from = self.next();
        let more = len.saturating_sub(self.keys.len()) as u64;
        self.keys.extend(from..from + more);
    }

    /// Forgets every key: the next frame re-keys the list from scratch,
    /// because none of its rows is one the user had.
    pub fn clear(&mut self) {
        self.keys.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed(len: usize) -> RowKeys {
        RowKeys::default().fitted(len)
    }

    /// The point of the module: after a delete, the row that moved up keeps
    /// its own key rather than taking over the deleted row's.
    #[test]
    fn a_row_keeps_its_key_when_the_row_above_it_goes() {
        let mut keys = keyed(3);
        let third = keys.get(2);
        keys.remove(1);
        assert_eq!(keys.get(1), third);
    }

    #[test]
    fn a_swap_moves_the_keys_with_the_rows() {
        let mut keys = keyed(2);
        let (first, second) = (keys.get(0), keys.get(1));
        keys.swap(0, 1);
        assert_eq!((keys.get(0), keys.get(1)), (second, first));
    }

    /// A new row is nobody's old row, however the list was edited before.
    #[test]
    fn new_keys_never_repeat_one_already_handed_out() {
        let mut keys = keyed(3);
        let old: Vec<u64> = (0..3).map(|i| keys.get(i)).collect();
        keys.remove(2);
        keys.grow_to(3);
        assert!(!old[..2].contains(&keys.get(2)));

        // Somebody else changed the length: every row is re-keyed past them.
        let refitted = keys.clone().fitted(5);
        for i in 0..5 {
            assert!(!old.contains(&refitted.get(i)), "{i}");
        }
    }
}
