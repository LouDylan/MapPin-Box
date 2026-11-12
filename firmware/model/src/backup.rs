use crate::button::Action;
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

pub const EXPORT_WINDOW: Duration = Duration::from_secs(45);
const EXTRA_HOLD: Duration = Duration::from_secs(3);
const CONFIRM_VISIBLE: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportStatus {
    Waiting,
    Approved,
}

struct Request {
    client: IpAddr,
    token: String,
    expires: Instant,
    approved: bool,
}

#[derive(Default)]
pub struct BackupExport {
    request: Option<Request>,
    held: Option<(String, Instant)>,
    confirmed_at: Option<Instant>,
}

impl BackupExport {
    pub fn status(&self, now: Instant) -> Option<ExportStatus> {
        if let Some(request) = self
            .request
            .as_ref()
            .filter(|request| now < request.expires)
        {
            return Some(if request.approved {
                ExportStatus::Approved
            } else {
                ExportStatus::Waiting
            });
        }
        self.confirmed_at
            .filter(|at| now.saturating_duration_since(*at) < CONFIRM_VISIBLE)
            .map(|_| ExportStatus::Approved)
    }

    pub fn begin(
        &mut self,
        client: IpAddr,
        token: String,
        now: Instant,
    ) -> Result<(), &'static str> {
        if self
            .request
            .as_ref()
            .is_some_and(|request| now < request.expires)
        {
            return Err("已有备份请求，请等它完成后重试");
        }
        self.request = Some(Request {
            client,
            token,
            expires: now + EXPORT_WINDOW,
            approved: false,
        });
        self.confirmed_at = None;
        Ok(())
    }

    pub fn claim(
        &mut self,
        client: IpAddr,
        token: &str,
        now: Instant,
    ) -> Result<bool, &'static str> {
        let request = self
            .request
            .as_ref()
            .filter(|request| {
                request.client == client && request.token == token && now < request.expires
            })
            .ok_or("备份确认已失效，请重新导出")?;
        if !request.approved {
            return Ok(false);
        }
        self.request = None;
        Ok(true)
    }

    pub fn button(
        &mut self,
        pressed: bool,
        action: Option<Action>,
        now: Instant,
    ) -> Option<Action> {
        if let Some((token, started)) = self.held.as_ref() {
            if !pressed {
                self.held = None;
                return Some(Action::Replay);
            }
            if now.saturating_duration_since(*started) >= EXTRA_HOLD {
                if let Some(request) = self
                    .request
                    .as_mut()
                    .filter(|request| request.token == *token && now < request.expires)
                {
                    request.approved = true;
                    self.confirmed_at = Some(now);
                }
                self.held = None;
            }
            return None;
        }
        if action == Some(Action::Replay)
            && pressed
            && let Some(request) = self
                .request
                .as_ref()
                .filter(|request| !request.approved && now < request.expires)
        {
            self.held = Some((request.token.clone(), now));
            return None;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_requires_the_extended_hold_and_is_bound_to_one_client_and_download() {
        let client = "192.168.71.2".parse().unwrap();
        let other = "192.168.71.3".parse().unwrap();
        let start = Instant::now();
        let mut gate = BackupExport::default();
        gate.begin(client, "token".into(), start).unwrap();
        assert_eq!(gate.status(start), Some(ExportStatus::Waiting));
        assert_eq!(
            gate.button(false, Some(Action::Restore), start),
            Some(Action::Restore)
        );
        assert_eq!(gate.button(true, Some(Action::Replay), start), None);
        assert!(!gate.claim(client, "token", start).unwrap());
        assert_eq!(
            gate.button(false, None, start + Duration::from_secs(1)),
            Some(Action::Replay)
        );
        let retry = start + Duration::from_secs(2);
        let approved = retry + EXTRA_HOLD;
        assert_eq!(gate.button(true, Some(Action::Replay), retry), None);
        assert_eq!(gate.button(true, None, approved), None);
        assert_eq!(gate.status(approved), Some(ExportStatus::Approved));
        assert!(gate.claim(other, "token", approved).is_err());
        assert!(gate.claim(client, "wrong", approved).is_err());
        assert!(gate.claim(client, "token", approved).unwrap());
        assert!(gate.claim(client, "token", approved).is_err());
        assert_eq!(gate.status(approved), Some(ExportStatus::Approved));
        assert_eq!(gate.status(approved + CONFIRM_VISIBLE), None);
    }

    #[test]
    fn an_expired_hold_cannot_approve_a_later_request() {
        let client = "192.168.71.2".parse().unwrap();
        let start = Instant::now();
        let mut gate = BackupExport::default();
        gate.begin(client, "first".into(), start).unwrap();
        gate.button(true, Some(Action::Replay), start + Duration::from_secs(44));
        let next = start + EXPORT_WINDOW;
        assert_eq!(gate.status(next), None);
        assert!(gate.claim(client, "first", next).is_err());
        gate.begin(client, "second".into(), next).unwrap();
        gate.button(true, None, next + EXTRA_HOLD);
        assert!(!gate.claim(client, "second", next + EXTRA_HOLD).unwrap());
    }
}
