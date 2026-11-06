//! State for the stable release offer.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateState {
    stage: Stage,
    selected: Choice,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Idle,
    Available {
        installed: String,
        available: String,
        offered: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    Update,
    #[default]
    Later,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open,
    Apply { available: String },
    Later { available: String },
}

impl UpdateState {
    pub(crate) fn available(&mut self, installed: &str, available: &str) {
        self.stage = Stage::Available {
            installed: installed.to_owned(),
            available: available.to_owned(),
            offered: false,
        };
        self.selected = Choice::Later;
    }

    pub(crate) fn stage(&self) -> &Stage {
        &self.stage
    }

    pub(crate) fn open(&mut self) -> bool {
        let Stage::Available { offered, .. } = &mut self.stage else {
            return false;
        };
        *offered = true;
        self.selected = Choice::Later;
        true
    }

    pub(crate) fn close(&mut self) {
        if let Stage::Available { offered, .. } = &mut self.stage {
            *offered = false;
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        matches!(self.stage, Stage::Available { offered: true, .. })
    }

    pub(crate) fn selected(&self) -> Choice {
        self.selected
    }

    pub(crate) fn move_choice(&mut self, choice: Choice) {
        self.selected = choice;
    }

    pub(crate) fn answer(&mut self) -> Option<Action> {
        let Stage::Available { available, .. } = &self.stage else {
            return None;
        };
        let action = match self.selected {
            Choice::Update => Action::Apply {
                available: available.clone(),
            },
            Choice::Later => Action::Later {
                available: available.clone(),
            },
        };
        self.close();
        Some(action)
    }
}
