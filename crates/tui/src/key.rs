#[cfg(not(target_arch = "wasm32"))]
pub use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::ops::{BitOr, BitOrAssign};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyCode {
        Char(char),
        F(u8),
        Backspace,
        Enter,
        Left,
        Right,
        Up,
        Down,
        Tab,
        BackTab,
        Delete,
        Home,
        End,
        PageUp,
        PageDown,
        Esc,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyModifiers(u8);

    impl KeyModifiers {
        pub const NONE: Self = Self(0);
        pub const SHIFT: Self = Self(1);
        pub const CONTROL: Self = Self(2);
        pub const ALT: Self = Self(4);

        pub const fn empty() -> Self {
            Self::NONE
        }

        pub const fn is_empty(self) -> bool {
            self.0 == 0
        }

        pub const fn contains(self, other: Self) -> bool {
            self.0 & other.0 == other.0
        }
    }

    impl Default for KeyModifiers {
        fn default() -> Self {
            Self::NONE
        }
    }

    impl BitOr for KeyModifiers {
        type Output = Self;

        fn bitor(self, other: Self) -> Self::Output {
            Self(self.0 | other.0)
        }
    }

    impl BitOrAssign for KeyModifiers {
        fn bitor_assign(&mut self, other: Self) {
            self.0 |= other.0;
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyEvent {
        pub code: KeyCode,
        pub modifiers: KeyModifiers,
    }

    impl KeyEvent {
        pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
            Self { code, modifiers }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::{KeyCode, KeyEvent, KeyModifiers};
