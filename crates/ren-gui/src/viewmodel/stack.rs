//! The pipeline as the user edits it: an ordered stack of operation cards.
//!
//! **D8** — *"the operation pipeline is first-class; presets are just saved
//! pipelines"*. This is that pipeline in its editable form. It converts to
//! `Vec<(OpKind, StepConfig)>`, which is what a preset stores, what a job file
//! parses into and what `ren_core::Pipeline` is built from.
//!
//! No egui in here: reorder, duplicate and delete are list operations, and they
//! are worth testing without a window.

use ren_core::model::Scope;
use ren_core::preproc::PreProcessor;
use ren_core::{Answers, OpKind, Pipeline, RunSettings, StepConfig};

use crate::widgets::filter_editor::FilterForm;

/// A card's identity for as long as the app is running.
///
/// **Never the index, and never persisted.** egui keys widget state by id, so a
/// positional id would hand a card's text cursor, open combo box and expansion
/// state to a *different* card the moment anything was reordered or deleted.
/// And a preset is a list, not a set of ids — reloading one starts them afresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CardId(u64);

impl CardId {
    /// An id no card has.
    ///
    /// `CardStack::mint` starts at 1, so zero is unreachable. For the one
    /// caller that must build an `EditorCx` before it knows which card it is
    /// about to draw — `for_card` overwrites it per card, and an editor that
    /// compared against this would be asking the wrong question anyway.
    pub fn none() -> Self {
        Self(0)
    }
}

impl std::fmt::Display for CardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "card{}", self.0)
    }
}

/// One operation, with the settings that belong to it rather than to the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: CardId,
    pub op: OpKind,
    pub scope: Scope,
    pub enabled: bool,
    /// `None` means *inherit the source bar's filter* (D32). The distinction is
    /// the whole point: a preset stores what the card shows, and a card that
    /// shows "using the source bar's" stores nothing.
    pub filter: Option<FilterForm>,
    pub preproc: Option<PreProcessor>,
}

impl Card {
    fn new(id: CardId, op: OpKind, config: StepConfig) -> Self {
        Self {
            id,
            scope: config.scope,
            enabled: config.enabled,
            filter: config.filter.as_ref().map(FilterForm::from_filter),
            preproc: config.preproc,
            op,
        }
    }

    /// What a preset stores: exactly what the card shows.
    pub fn stored_config(&self) -> StepConfig {
        StepConfig {
            scope: self.scope,
            enabled: self.enabled,
            filter: self
                .filter
                .as_ref()
                .filter(|f| f.is_active())
                .map(FilterForm::to_filter),
            preproc: self.preproc.clone(),
        }
    }

    /// What the engine runs: this card's filter, or the run-wide one it
    /// inherits when it has none of its own.
    pub fn runtime_config(&self, inherited: Option<&FilterForm>) -> StepConfig {
        let form = self.filter.as_ref().or(inherited);
        StepConfig {
            filter: form.filter(|f| f.is_active()).map(FilterForm::to_filter),
            ..self.stored_config()
        }
    }

    pub fn title(&self) -> &'static str {
        self.op.label()
    }

    pub fn summary(&self) -> String {
        self.op.summary()
    }

    /// Whether this card is using the run-wide filter rather than one of
    /// its own — which the card says out loud, so inheritance is never a
    /// surprise.
    pub fn inherits_filter(&self) -> bool {
        self.filter.is_none()
    }
}

/// The ordered stack.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardStack {
    /// What the pipeline is called — the name it would be saved under.
    pub name: String,
    cards: Vec<Card>,
    next_id: u64,
}

impl CardStack {
    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    pub fn get(&self, index: usize) -> Option<&Card> {
        self.cards.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Card> {
        self.cards.get_mut(index)
    }

    pub fn index_of(&self, id: CardId) -> Option<usize> {
        self.cards.iter().position(|c| c.id == id)
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Appends an operation with the settings a fresh card starts with.
    pub fn push(&mut self, op: OpKind) -> CardId {
        let config = StepConfig::for_op(&op);
        self.push_with(op, config)
    }

    pub fn push_with(&mut self, op: OpKind, config: StepConfig) -> CardId {
        let id = self.mint();
        self.cards.push(Card::new(id, op, config));
        id
    }

    /// Adds after `index`, or at the end. What the palette does, so a new card
    /// lands next to the one the user was looking at.
    pub fn insert_after(&mut self, index: Option<usize>, op: OpKind) -> CardId {
        let id = self.mint();
        let card = Card::new(id, op.clone(), StepConfig::for_op(&op));
        match index {
            Some(i) if i + 1 < self.cards.len() => self.cards.insert(i + 1, card),
            _ => self.cards.push(card),
        }
        id
    }

    /// A copy with its own identity, right after the original.
    pub fn duplicate(&mut self, index: usize) -> Option<CardId> {
        let mut copy = self.cards.get(index)?.clone();
        copy.id = self.mint();
        let id = copy.id;
        self.cards.insert(index + 1, copy);
        Some(id)
    }

    pub fn remove(&mut self, index: usize) -> Option<Card> {
        (index < self.cards.len()).then(|| self.cards.remove(index))
    }

    /// Moves the card at `from` so it lands at `to`.
    ///
    /// Returns false and changes nothing when that would be a no-op, so a drag
    /// that ends where it started does not dirty the preview.
    pub fn move_card(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= self.cards.len() || to >= self.cards.len() {
            return false;
        }
        let card = self.cards.remove(from);
        self.cards.insert(to, card);
        true
    }

    fn mint(&mut self) -> CardId {
        self.next_id += 1;
        CardId(self.next_id)
    }

    // --- The wire form ------------------------------------------------------

    /// What a preset stores, and what the eframe blob persists.
    pub fn to_steps(&self) -> Vec<(OpKind, StepConfig)> {
        self.cards
            .iter()
            .map(|card| (card.op.clone(), card.stored_config()))
            .collect()
    }

    pub fn from_steps(name: impl Into<String>, steps: Vec<(OpKind, StepConfig)>) -> Self {
        let mut stack = Self {
            name: name.into(),
            ..Default::default()
        };
        stack.append_steps(steps);
        stack
    }

    /// Adds a preset's steps to the end of what is already there.
    pub fn append_steps(&mut self, steps: Vec<(OpKind, StepConfig)>) {
        for (op, config) in steps {
            self.push_with(op, config);
        }
    }

    /// The runnable pipeline.
    ///
    /// Always through `OpKind::to_transform()`, which clones — and a clone
    /// resets the operation's compiled cache (D21). That is the only reason
    /// editing a `Replace`'s pattern takes effect at all, so this must never
    /// become a partial rebuild that reuses transforms across edits.
    pub fn to_pipeline(
        &self,
        inherited: Option<&FilterForm>,
        settings: &RunSettings,
        answers: &Answers,
    ) -> Pipeline {
        let mut pipeline = Pipeline::new();
        for card in &self.cards {
            pipeline.push(card.op.to_step(), card.runtime_config(inherited));
        }
        pipeline.settings = settings.clone();
        pipeline.answers = answers.clone();
        pipeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::ops::{Casing, Replace};
    use ren_core::{IncludeFilter, MatchSpec};

    fn stack_of(count: usize) -> CardStack {
        let mut stack = CardStack::default();
        for i in 0..count {
            stack.push(OpKind::Replace(Replace::new(i.to_string(), "")));
        }
        stack
    }

    fn finds(stack: &CardStack) -> Vec<String> {
        stack
            .cards()
            .iter()
            .map(|c| match &c.op {
                OpKind::Replace(op) => op.find.clone(),
                other => other.name().to_owned(),
            })
            .collect()
    }

    #[test]
    fn cards_keep_their_identity_across_reorder_and_delete() {
        let mut stack = stack_of(3);
        let ids: Vec<CardId> = stack.cards().iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids[0] != ids[1] && ids[1] != ids[2]);

        stack.move_card(0, 2);
        assert_eq!(stack.index_of(ids[0]), Some(2));
        assert_eq!(stack.index_of(ids[1]), Some(0));

        stack.remove(0);
        assert_eq!(stack.index_of(ids[1]), None);
        assert_eq!(stack.index_of(ids[0]), Some(1), "still findable by id");
    }

    #[test]
    fn moving_a_card_reorders_the_stack() {
        let mut stack = stack_of(4);
        assert!(stack.move_card(3, 0));
        assert_eq!(finds(&stack), ["3", "0", "1", "2"]);
        assert!(stack.move_card(0, 3));
        assert_eq!(finds(&stack), ["0", "1", "2", "3"]);
    }

    /// A drag that ends where it started must not dirty the preview.
    #[test]
    fn moving_a_card_onto_itself_changes_nothing() {
        let mut stack = stack_of(3);
        assert!(!stack.move_card(1, 1));
        assert!(!stack.move_card(0, 9), "out of range does nothing");
        assert_eq!(finds(&stack), ["0", "1", "2"]);
    }

    /// Duplicate is an M4 feature, so two cards of one kind is the normal case.
    /// They must not share an id, or they share their editor state.
    #[test]
    fn a_duplicate_is_a_copy_with_its_own_identity() {
        let mut stack = stack_of(2);
        let original = stack.cards()[0].id;
        let copy = stack.duplicate(0).expect("index 0 exists");

        assert_ne!(copy, original);
        assert_eq!(stack.len(), 3);
        assert_eq!(finds(&stack), ["0", "0", "1"], "right after the original");
        assert_eq!(stack.get(0).unwrap().op, stack.get(1).unwrap().op);
    }

    #[test]
    fn the_palette_adds_after_the_card_you_were_looking_at() {
        let mut stack = stack_of(3);
        stack.insert_after(Some(0), OpKind::Casing(Casing::default()));
        assert_eq!(finds(&stack), ["0", "casing", "1", "2"]);

        stack.insert_after(None, OpKind::Casing(Casing::default()));
        assert_eq!(finds(&stack), ["0", "casing", "1", "2", "casing"]);
    }

    /// Free Format rebuilds the whole name, so a fresh card of it starts scoped
    /// to the extension as well.
    #[test]
    fn a_new_card_starts_with_the_scope_its_operation_wants() {
        let mut stack = CardStack::default();
        stack.push(OpKind::Replace(Replace::default()));
        stack.push(OpKind::FreeFormat(ren_core::FreeFormat::default()));

        assert_eq!(stack.get(0).unwrap().scope, Scope::Name);
        assert_eq!(stack.get(1).unwrap().scope, Scope::Both);
    }

    #[test]
    fn the_stack_round_trips_through_the_wire_form() {
        let mut stack = stack_of(2);
        stack.get_mut(1).unwrap().enabled = false;
        stack.get_mut(1).unwrap().scope = Scope::Both;
        stack.get_mut(0).unwrap().filter = Some(FilterForm {
            include: "live".into(),
            ..Default::default()
        });

        let steps = stack.to_steps();
        let back = CardStack::from_steps("Round trip", steps.clone());

        assert_eq!(back.to_steps(), steps);
        assert!(!back.get(1).unwrap().enabled);
        assert_eq!(back.get(1).unwrap().scope, Scope::Both);
        assert_eq!(
            back.get(0).unwrap().filter.as_ref().unwrap().include,
            "live"
        );
        assert!(back.get(1).unwrap().inherits_filter());
    }

    /// D32: the source bar's filter is the default for a card that has none of
    /// its own, and a card with its own uses only its own.
    #[test]
    fn a_card_without_a_filter_inherits_the_run_wide_one() {
        let mut stack = stack_of(2);
        stack.get_mut(1).unwrap().filter = Some(FilterForm {
            include: "mine".into(),
            ..Default::default()
        });

        let inherited = FilterForm {
            include: "global".into(),
            ..Default::default()
        };

        let borrowed = stack.get(0).unwrap().runtime_config(Some(&inherited));
        assert_eq!(
            borrowed.filter.unwrap().include,
            Some(MatchSpec::Substring("global".into()))
        );

        let own = stack.get(1).unwrap().runtime_config(Some(&inherited));
        assert_eq!(
            own.filter.unwrap().include,
            Some(MatchSpec::Substring("mine".into()))
        );

        // And what a preset stores is the card's own, never the inherited one.
        assert_eq!(stack.to_steps()[0].1.filter, None);
        assert_eq!(
            stack.to_steps()[1].1.filter,
            Some(IncludeFilter::new().including(MatchSpec::Substring("mine".into())))
        );
    }

    /// Order is meaning: the same two cards the other way round give a
    /// different name, and `move_card` is what says which.
    #[test]
    fn the_pipeline_it_builds_runs_the_cards_in_order() {
        let mut stack = CardStack::default();
        stack.push(OpKind::Casing(Casing::new(ren_core::CaseMode::Title)));
        stack.push(OpKind::Replace(Replace::new("_", " and ")));

        let entry = ren_core::FileEntry::synthetic("/music/a_b.txt");
        let run = |stack: &CardStack| {
            let pipeline = stack.to_pipeline(None, &RunSettings::default(), &Answers::default());
            ren_core::evaluate_all(std::slice::from_ref(&entry), &pipeline)[0]
                .clone()
                .unwrap()
                .name
        };

        // Title case first, so the replacement text survives as typed.
        assert_eq!(run(&stack), "A and B.txt");

        // Replace first, and the replacement gets title-cased too.
        stack.move_card(1, 0);
        assert_eq!(run(&stack), "A And B.txt");
    }

    #[test]
    fn a_disabled_card_is_still_stored_but_does_not_run() {
        let mut stack = CardStack::default();
        stack.push(OpKind::Replace(Replace::new("_", " ")));
        stack.get_mut(0).unwrap().enabled = false;

        assert_eq!(stack.to_steps().len(), 1, "kept, so it can be re-enabled");
        let entry = ren_core::FileEntry::synthetic("/music/my_song.mp3");
        let pipeline = stack.to_pipeline(None, &RunSettings::default(), &Answers::default());
        let out = ren_core::evaluate_all(std::slice::from_ref(&entry), &pipeline);
        assert_eq!(out[0].as_ref().unwrap().name, "my_song.mp3");
    }

    #[test]
    fn appending_keeps_what_was_already_there() {
        let mut stack = stack_of(2);
        let extra = CardStack::from_steps("Other", stack_of(1).to_steps());
        stack.append_steps(extra.to_steps());
        assert_eq!(finds(&stack), ["0", "1", "0"]);
        // And the appended card got a fresh id.
        let ids: std::collections::HashSet<CardId> = stack.cards().iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 3);
    }
}
