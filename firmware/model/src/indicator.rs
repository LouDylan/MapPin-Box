use crate::backup::ExportStatus;
use std::time::Duration;

pub type Rgb = [u8; 3];

const OFF: Rgb = [0, 0, 0];
const BLUE: Rgb = [0, 3, 14];
const AMBER: Rgb = [24, 8, 0];
const RED: Rgb = [30, 0, 0];
const WHITE: Rgb = [16, 16, 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Starting,
    StartupFailed,
    Unpaired,
    Waiting,
    Ready,
    Working,
    Simulating,
    BackupWaiting,
    BackupReady,
    Restored,
    Failed,
}

impl Pattern {
    pub fn color(self, elapsed: Duration) -> Option<Rgb> {
        let ms = elapsed.as_millis() as u64;
        let color = match self {
            Self::Starting => [8, 8, 8],
            Self::StartupFailed => flashes(RED, 3, ms % 3_600).unwrap_or(OFF),
            Self::Unpaired => flashes(AMBER, 2, ms % 3_600).unwrap_or(OFF),
            Self::Waiting => flashes(BLUE, 1, ms % 2_600).unwrap_or(OFF),
            Self::Ready => BLUE,
            Self::Working => breathe([16, 0, 24], ms),
            Self::Simulating => [0, 14, 2],
            Self::BackupWaiting => breathe(AMBER, ms),
            Self::BackupReady => WHITE,
            Self::Restored => return flashes(WHITE, 2, ms),
            Self::Failed => return flashes(RED, 3, ms),
        };
        Some(color)
    }
}

fn flashes(color: Rgb, count: u64, ms: u64) -> Option<Rgb> {
    const HALF_PERIOD: u64 = 180;
    let half = ms / HALF_PERIOD;
    (half < count * 2).then_some(if half % 2 == 0 { color } else { OFF })
}

fn breathe(color: Rgb, ms: u64) -> Rgb {
    const HALF_PERIOD: u64 = 750;
    let step = (ms % (2 * HALF_PERIOD)).abs_diff(HALF_PERIOD);
    let level = 20 + 80 * (HALF_PERIOD - step) / HALF_PERIOD;
    color.map(|channel| (u64::from(channel) * level / 100) as u8)
}

pub struct Status {
    pub paired: bool,
    pub connected: bool,
    pub working: bool,
    pub simulating: bool,
    pub backup: Option<ExportStatus>,
}

impl Status {
    pub fn pattern(&self) -> Pattern {
        match self {
            Self { working: true, .. } => Pattern::Working,
            Self {
                backup: Some(ExportStatus::Waiting),
                ..
            } => Pattern::BackupWaiting,
            Self {
                backup: Some(ExportStatus::Approved),
                ..
            } => Pattern::BackupReady,
            Self { paired: false, .. } => Pattern::Unpaired,
            Self {
                connected: false, ..
            } => Pattern::Waiting,
            Self {
                simulating: true, ..
            } => Pattern::Simulating,
            _ => Pattern::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_simulation_only_after_success_with_an_attached_client() {
        let mut status = Status {
            paired: true,
            connected: false,
            working: false,
            simulating: true,
            backup: None,
        };
        assert_eq!(status.pattern(), Pattern::Waiting);
        status.connected = true;
        assert_eq!(status.pattern(), Pattern::Simulating);
        status.working = true;
        assert_eq!(status.pattern(), Pattern::Working);
        status.working = false;
        status.simulating = false;
        assert_eq!(status.pattern(), Pattern::Ready);
        status.paired = false;
        assert_eq!(status.pattern(), Pattern::Unpaired);
    }

    #[test]
    fn backup_cues_end_when_the_export_ends() {
        let mut status = Status {
            paired: true,
            connected: true,
            working: false,
            simulating: true,
            backup: Some(ExportStatus::Waiting),
        };
        assert_eq!(status.pattern(), Pattern::BackupWaiting);
        status.backup = Some(ExportStatus::Approved);
        assert_eq!(status.pattern(), Pattern::BackupReady);
        status.backup = None;
        assert_eq!(status.pattern(), Pattern::Simulating);
    }

    #[test]
    fn restore_and_failure_are_finite_and_have_distinct_counts() {
        let at = Duration::from_millis;
        assert_eq!(Pattern::Restored.color(at(0)), Some(WHITE));
        assert_eq!(Pattern::Restored.color(at(180)), Some(OFF));
        assert_eq!(Pattern::Restored.color(at(360)), Some(WHITE));
        assert_eq!(Pattern::Restored.color(at(720)), None);
        assert_eq!(Pattern::Failed.color(at(720)), Some(RED));
        assert_eq!(Pattern::Failed.color(at(1080)), None);
        assert_eq!(Pattern::StartupFailed.color(at(3600)), Some(RED));
    }
}
