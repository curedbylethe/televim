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
    Applying {
        installed: String,
        available: String,
        progress: Progress,
    },
    Failed {
        installed: String,
        available: String,
        reason: String,
        offered: bool,
    },
    Installed {
        installed: String,
        available: String,
        offered: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Downloading,
    Verifying,
    Extracting,
    Checking,
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
    Restart,
    InstalledLater,
    Cancel,
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
        match &mut self.stage {
            Stage::Available { offered, .. } | Stage::Failed { offered, .. } => *offered = true,
            _ => return false,
        }
        self.selected = Choice::Later;
        true
    }

    pub(crate) fn close(&mut self) {
        match &mut self.stage {
            Stage::Available { offered, .. }
            | Stage::Failed { offered, .. }
            | Stage::Installed { offered, .. } => *offered = false,
            _ => {}
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        matches!(
            self.stage,
            Stage::Available { offered: true, .. }
                | Stage::Applying { .. }
                | Stage::Failed { offered: true, .. }
                | Stage::Installed { offered: true, .. }
        )
    }

    pub(crate) fn begin_apply(&mut self, available: &str) -> bool {
        let Some((installed, current)) = self.versions() else {
            return false;
        };
        if current != available {
            return false;
        }
        self.stage = Stage::Applying {
            installed,
            available: current,
            progress: Progress::Downloading,
        };
        true
    }

    pub(crate) fn progress(&mut self, progress: Progress) {
        if let Stage::Applying {
            progress: stage, ..
        } = &mut self.stage
        {
            *stage = progress;
        }
    }

    pub(crate) fn fail(&mut self, reason: String) {
        if let Some((installed, available)) = self.versions() {
            self.stage = Stage::Failed {
                installed,
                available,
                reason,
                offered: true,
            };
            self.selected = Choice::Later;
        }
    }

    pub(crate) fn cancelled(&mut self) {
        if let Stage::Applying {
            installed,
            available,
            ..
        } = &self.stage
        {
            self.stage = Stage::Available {
                installed: installed.clone(),
                available: available.clone(),
                offered: false,
            };
        }
    }

    pub(crate) fn installed(&mut self, available: &str) {
        if let Stage::Applying { installed, .. } = &self.stage {
            self.stage = Stage::Installed {
                installed: installed.clone(),
                available: available.to_owned(),
                offered: true,
            };
            self.selected = Choice::Later;
        }
    }

    pub(crate) fn indicator_version(&self) -> Option<&str> {
        match &self.stage {
            Stage::Available { available, .. }
            | Stage::Applying { available, .. }
            | Stage::Failed { available, .. } => Some(available),
            Stage::Idle | Stage::Installed { .. } => None,
        }
    }

    pub(crate) fn selected(&self) -> Choice {
        self.selected
    }

    pub(crate) fn move_choice(&mut self, choice: Choice) {
        self.selected = choice;
    }

    pub(crate) fn answer(&mut self) -> Option<Action> {
        if matches!(self.stage, Stage::Installed { .. }) {
            self.close();
            return Some(match self.selected {
                Choice::Update => Action::Restart,
                Choice::Later => Action::InstalledLater,
            });
        }
        if matches!(self.stage, Stage::Applying { .. }) {
            return Some(Action::Cancel);
        }
        let available = match &self.stage {
            Stage::Available {
                available,
                offered: true,
                ..
            }
            | Stage::Failed {
                available,
                offered: true,
                ..
            } => available.clone(),
            _ => return None,
        };
        self.close();
        Some(match self.selected {
            Choice::Update => Action::Apply { available },
            Choice::Later => Action::Later { available },
        })
    }

    fn versions(&self) -> Option<(String, String)> {
        match &self.stage {
            Stage::Available {
                installed,
                available,
                ..
            }
            | Stage::Failed {
                installed,
                available,
                ..
            }
            | Stage::Applying {
                installed,
                available,
                ..
            } => Some((installed.clone(), available.clone())),
            _ => None,
        }
    }
}
