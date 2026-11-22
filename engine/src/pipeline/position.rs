use crate::block::{Event, Subject};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SiteKind {
    Hook,
    Proxy,
}

impl SiteKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SiteKind::Hook => "hook",
            SiteKind::Proxy => "proxy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub site: SiteKind,
    pub event: Event,
    pub subject: Subject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionError {
    UnknownEvent(String),
    UnknownSubject(String),
}

impl Position {
    pub fn new(site: SiteKind, event: Event, subject: Subject) -> Self {
        Position {
            site,
            event,
            subject,
        }
    }

    pub fn hook(event: Event, subject: Subject) -> Self {
        Position::new(SiteKind::Hook, event, subject)
    }

    pub fn proxy(subject: Subject) -> Self {
        Position::new(SiteKind::Proxy, Event::Request, subject)
    }

    pub fn render(&self) -> String {
        format!(
            "{}.{}.{}",
            self.site.as_str(),
            self.event.as_str(),
            self.subject.as_str()
        )
    }

    pub fn header(&self) -> String {
        format!("[[{}]]", self.render())
    }

    pub fn parse_parts(site: SiteKind, event: &str, subject: &str) -> Result<Self, PositionError> {
        let event = match (site, event) {
            (SiteKind::Hook, "before_tool") => Event::BeforeTool,
            (SiteKind::Hook, "after_tool") => Event::AfterTool,
            (SiteKind::Proxy, "request") => Event::Request,
            (_, other) => return Err(PositionError::UnknownEvent(other.to_string())),
        };
        let parsed =
            Subject::parse(subject).ok_or_else(|| PositionError::UnknownSubject(subject.into()))?;
        let legal = match site {
            SiteKind::Hook => parsed.is_hook_category(),
            SiteKind::Proxy => parsed.is_proxy_part(),
        };
        if !legal {
            return Err(PositionError::UnknownSubject(subject.to_string()));
        }
        Ok(Position::new(site, event, parsed))
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.splitn(3, '.');
        let site = match parts.next()? {
            "hook" => SiteKind::Hook,
            "proxy" => SiteKind::Proxy,
            _ => return None,
        };
        let event = parts.next()?;
        let subject = parts.next()?;
        Position::parse_parts(site, event, subject).ok()
    }

    pub fn any_sibling(&self) -> Position {
        Position::new(self.site, self.event, Subject::Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_render_and_parse_round_trip() {
        for text in [
            "hook.before_tool.shell",
            "hook.after_tool.read",
            "proxy.request.tool_result",
            "proxy.request.cache",
        ] {
            let position = Position::parse(text).expect(text);
            assert_eq!(position.render(), text);
        }
    }

    #[test]
    fn illegal_combinations_are_rejected() {
        assert!(Position::parse("hook.during_tool.shell").is_none());
        assert!(Position::parse("hook.before_tool.tool_result").is_none());
        assert!(Position::parse("proxy.request.shell").is_none());
        assert!(Position::parse("proxy.before_tool.cache").is_none());
    }

    #[test]
    fn any_is_legal_at_both_sites() {
        assert!(Position::parse("hook.after_tool.any").is_some());
        assert!(Position::parse("proxy.request.any").is_some());
    }
}
