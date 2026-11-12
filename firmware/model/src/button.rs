const DEBOUNCE_MS: u64 = 40;
const LONG_PRESS_MS: u64 = 3000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Restore,
    Replay,
}

#[derive(Default)]
pub struct Button {
    raw: bool,
    stable: bool,
    changed_at: u64,
    pressed_at: u64,
    long_fired: bool,
}

impl Button {
    pub fn is_pressed(&self) -> bool {
        self.stable
    }

    pub fn update(&mut self, pressed: bool, now: u64) -> Option<Action> {
        if pressed != self.raw {
            self.raw = pressed;
            self.changed_at = now;
        }
        if now.saturating_sub(self.changed_at) < DEBOUNCE_MS {
            return None;
        }
        if pressed != self.stable {
            self.stable = pressed;
            if pressed {
                self.pressed_at = self.changed_at;
                self.long_fired = false;
            } else if !self.long_fired {
                return Some(
                    if self.changed_at.saturating_sub(self.pressed_at) >= LONG_PRESS_MS {
                        Action::Replay
                    } else {
                        Action::Restore
                    },
                );
            }
        }
        if pressed && !self.long_fired && now.saturating_sub(self.pressed_at) >= LONG_PRESS_MS {
            self.long_fired = true;
            return Some(Action::Replay);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_press_waits_for_a_debounced_release() {
        let mut button = Button::default();
        assert_eq!(button.update(true, 0), None);
        assert_eq!(button.update(true, 50), None);
        assert_eq!(button.update(false, 200), None);
        assert_eq!(button.update(false, 250), Some(Action::Restore));
        assert_eq!(button.update(false, 500), None);
    }

    #[test]
    fn long_press_fires_once_and_never_restores_on_release() {
        let mut button = Button::default();
        button.update(true, 0);
        button.update(true, 50);
        assert_eq!(button.update(true, 2999), None);
        assert_eq!(button.update(true, 3000), Some(Action::Replay));
        assert_eq!(button.update(true, 7000), None);
        button.update(false, 7100);
        assert_eq!(button.update(false, 7150), None);
    }

    #[test]
    fn contact_bounce_does_not_trigger_an_action() {
        let mut button = Button::default();
        button.update(true, 0);
        button.update(false, 10);
        assert_eq!(button.update(false, 100), None);
        button.update(true, 200);
        button.update(true, 250);
        button.update(false, 300);
        button.update(true, 310);
        assert_eq!(button.update(true, 360), None);
        button.update(false, 500);
        assert_eq!(button.update(false, 550), Some(Action::Restore));
    }

    #[test]
    fn a_new_press_after_a_long_press_can_restore_normally() {
        let mut button = Button::default();
        for (pressed, now) in [
            (true, 0),
            (true, 50),
            (true, 3000),
            (false, 3100),
            (false, 3150),
            (true, 4000),
            (true, 4050),
            (false, 4200),
        ] {
            button.update(pressed, now);
        }
        assert_eq!(button.update(false, 4250), Some(Action::Restore));
    }

    #[test]
    fn release_after_threshold_is_long_even_if_a_poll_was_delayed() {
        let mut button = Button::default();
        button.update(true, 0);
        button.update(true, 50);
        button.update(true, 2990);
        button.update(false, 3010);
        assert_eq!(button.update(false, 3060), Some(Action::Replay));
        assert_eq!(button.update(false, 3200), None);
    }
}
