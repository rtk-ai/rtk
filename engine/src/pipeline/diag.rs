#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    Load,
    Trust,
    E001,
    E002,
    E003,
    E004,
    E005,
    E006,
    E007,
    E008,
    E009,
    E010,
    E011,
    E012,
    E013,
    E014,
    E015,
    E016,
    E017,
    E018,
    E019,
    E020,
    W001,
    W002,
    W003,
    W004,
    W005,
    W006,
    W007,
    W008,
    W009,
    W010,
    W011,
    W012,
    W013,
    W014,
    W015,
    W016,
    W017,
    W018,
    W019,
    W021,
    W022,
    W023,
    W024,
    W025,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Code {
    pub fn as_str(&self) -> &'static str {
        match self {
            Code::Load => "E000",
            Code::Trust => "TRUST",
            Code::E001 => "E001",
            Code::E002 => "E002",
            Code::E003 => "E003",
            Code::E004 => "E004",
            Code::E005 => "E005",
            Code::E006 => "E006",
            Code::E007 => "E007",
            Code::E008 => "E008",
            Code::E009 => "E009",
            Code::E010 => "E010",
            Code::E011 => "E011",
            Code::E012 => "E012",
            Code::E013 => "E013",
            Code::E014 => "E014",
            Code::E015 => "E015",
            Code::E016 => "E016",
            Code::E017 => "E017",
            Code::E018 => "E018",
            Code::E019 => "E019",
            Code::E020 => "E020",
            Code::W001 => "W001",
            Code::W002 => "W002",
            Code::W003 => "W003",
            Code::W004 => "W004",
            Code::W005 => "W005",
            Code::W006 => "W006",
            Code::W007 => "W007",
            Code::W008 => "W008",
            Code::W009 => "W009",
            Code::W010 => "W010",
            Code::W011 => "W011",
            Code::W012 => "W012",
            Code::W013 => "W013",
            Code::W014 => "W014",
            Code::W015 => "W015",
            Code::W016 => "W016",
            Code::W017 => "W017",
            Code::W018 => "W018",
            Code::W019 => "W019",
            Code::W021 => "W021",
            Code::W022 => "W022",
            Code::W023 => "W023",
            Code::W024 => "W024",
            Code::W025 => "W025",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Code::Trust => Severity::Warning,
            _ if self.as_str().starts_with('E') => Severity::Error,
            _ => Severity::Warning,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub context: Option<String>,
    pub message: String,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            file: None,
            line: None,
            context: None,
            message: message.into(),
            hint: None,
        }
    }

    pub fn at(mut self, file: impl Into<String>, line: Option<usize>) -> Self {
        self.file = Some(file.into());
        self.line = line;
        self
    }

    pub fn context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn severity(&self) -> Severity {
        self.code.severity()
    }

    pub fn render(&self) -> String {
        let mut head = String::new();
        if let Some(file) = &self.file {
            head.push_str(file);
            if let Some(line) = self.line {
                head.push_str(&format!(":{line}"));
            }
        }
        if let Some(context) = &self.context {
            if !head.is_empty() {
                head.push_str("  ");
            }
            head.push_str(context);
        }
        let mut out = String::new();
        if !head.is_empty() {
            out.push_str(&head);
            out.push('\n');
        }
        out.push_str(&format!("  {}  {}", self.code.as_str(), self.message));
        if let Some(hint) = &self.hint {
            out.push_str(&format!("\n        hint: {hint}"));
        }
        out
    }
}

pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity() == Severity::Error)
}

pub fn did_you_mean<'a>(target: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let d = edit_distance(target, candidate);
        if d <= 2 && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, candidate));
        }
    }
    best.map(|(_, name)| name.to_string())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut prev, &mut current);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_split_into_errors_and_warnings() {
        assert_eq!(Code::E003.severity(), Severity::Error);
        assert_eq!(Code::W009.severity(), Severity::Warning);
        assert_eq!(Code::Load.severity(), Severity::Error);
    }

    #[test]
    fn render_matches_the_guide_message_shape() {
        let d = Diagnostic::new(
            Code::E003,
            "condition `file_lines` is not available after the tool ran",
        )
        .at("~/.config/rtk/pipeline.toml", Some(41))
        .context("[[hook.after_tool.shell]] \"my-term\"")
        .hint("`file_lines` is a before_tool condition; for output size use `lines`");
        let rendered = d.render();
        assert!(
            rendered.starts_with(
                "~/.config/rtk/pipeline.toml:41  [[hook.after_tool.shell]] \"my-term\""
            )
        );
        assert!(rendered.contains("E003  condition `file_lines`"));
        assert!(rendered.contains("hint: `file_lines` is a before_tool condition"));
    }

    #[test]
    fn did_you_mean_finds_close_names() {
        let names = ["term-default", "grep-group", "json-reads"];
        assert_eq!(
            did_you_mean("term-defualt", names.iter().copied()),
            Some("term-default".to_string())
        );
        assert_eq!(did_you_mean("zzz", names.iter().copied()), None);
    }
}
