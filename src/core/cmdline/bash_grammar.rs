//! Bash's reserved words, as one table: which words they are, where the word
//! after each one stands, and what each one opens, continues or closes.
//! Every reader of a command line that has to know one of these facts reads
//! it here.

use crate::core::arg_tokenizer::{Flag, Grammar, TokenKind as ArgKind, tokenize_grammar};

/// A bash reserved word, as `compgen -k` lists them in bash 5.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reserved {
    If,
    Then,
    Elif,
    Else,
    Fi,
    Case,
    In,
    Esac,
    For,
    Select,
    While,
    Until,
    Do,
    Done,
    Function,
    Coproc,
    Time,
    Bang,
    OpenBrace,
    CloseBrace,
    OpenTest,
    CloseTest,
}

/// A compound command, or definition, that reserved words delimit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compound {
    If,
    Case,
    For,
    Select,
    While,
    Until,
    /// `{ list; }`.
    Group,
    /// `[[ expression ]]`.
    Test,
    Function,
    Coproc,
}

impl Compound {
    /// Whether its parts are command lists. `[[ ]]` holds an expression.
    fn holds_commands(self) -> bool {
        self != Compound::Test
    }

    /// Whether a reserved word ends it. The body of a `function` or `coproc`
    /// definition is the command after it, which ends on its own.
    fn has_closing_word(self) -> bool {
        !matches!(self, Compound::Function | Compound::Coproc)
    }
}

/// What a reserved word does in the compound command it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Starts one. `function` and `coproc` start a definition whose body is
    /// the command after them, so no reserved word ends it.
    Opens(Compound),
    /// Ends one part of it and starts the next, in any of these.
    Continues(&'static [Compound]),
    /// Ends it, in any of these.
    Closes(&'static [Compound]),
    /// Starts a pipeline and belongs to no compound command.
    Prefix,
}

/// Where a word stands, which decides what bash reads it as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// Where a command starts: bash reads a reserved word or a command name.
    Command,
    /// Where bash reads a reserved word but no command, as after a compound
    /// command's last word: `if (true) then`, `case x in a) (ls) esac`. Any
    /// other word there is a syntax error, or an argument of `coproc`'s
    /// command (`coproc ls -l`).
    ReservedWord,
    /// Anywhere else: a name, a `case` subject, the words of a list, an
    /// operand or an argument. No word there is reserved.
    Word,
}

/// One row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservedWord {
    pub word: Reserved,
    pub spelling: &'static str,
    /// Where the word after it stands. After `coproc` a command starts, and
    /// its first word may instead name the coprocess: see `names`.
    pub next: Position,
    /// Whether the word after it may be a name, after which bash reads a
    /// reserved word and no command: `for NAME do`, `select NAME in`,
    /// `function NAME {`, `coproc NAME {`.
    pub names: bool,
    pub role: Role,
    /// The options bash reads right after the word, if it takes any
    /// ([`ReservedWord::options_read`]). The word after them stands where the
    /// word after this one would.
    pub options: Option<Grammar>,
}

impl ReservedWord {
    /// Whether the word delimits a command list of a compound command: it
    /// opens or closes one that holds commands, or starts one of its lists
    /// (`then`, `elif`, `else`, `do`). A line that starts with one belongs to a
    /// construct the lines around it are part of. `in` starts a word list or
    /// the patterns, and `[[ ]]` holds an expression, so neither counts.
    pub fn delimits_command_list(&self) -> bool {
        match self.role {
            Role::Opens(compound) => compound.holds_commands(),
            Role::Continues(_) => self.next == Position::Command,
            Role::Closes(compounds) => compounds.iter().all(|c| c.holds_commands()),
            Role::Prefix => false,
        }
    }

    /// Whether the word opens a compound command whose parts are command
    /// lists and that a reserved word ends: one of the rows of
    /// [`RESERVED_WORDS`] whose [`Role::Opens`] names such a compound.
    pub fn opens_command_lists(&self) -> bool {
        matches!(self.role, Role::Opens(c) if c.holds_commands() && c.has_closing_word())
    }

    /// Whether the word ends a compound command that
    /// [`ReservedWord::opens_command_lists`] opens: one of the rows of
    /// [`RESERVED_WORDS`] whose [`Role::Closes`] names only such compounds.
    pub fn closes_command_lists(&self) -> bool {
        matches!(self.role, Role::Closes(compounds) if compounds.iter().all(|c| c.holds_commands()))
    }

    /// Whether a pipeline may start after it, and not only a command. Bash
    /// reads `time` as reserved only where a pipeline starts
    /// (`time_command_acceptable` in its `parse.y`), and `coproc` takes one
    /// command, so `coproc time ls` runs the program `time`, as `ls | time ls`
    /// does.
    pub fn pipeline_follows(&self) -> bool {
        self.next == Position::Command && self.word != Reserved::Coproc
    }

    /// How many of `words`, the words right after this one as written, bash
    /// reads as its options: each option its grammar declares once, then a
    /// `--`, each one optional (`time -p --`). A word that repeats an option,
    /// or follows the `--`, is none: in `time -p -p` and `time -- -p` the
    /// second word is a command's name.
    pub fn options_read<'w>(&self, words: impl IntoIterator<Item = &'w str>) -> usize {
        let Some(grammar) = &self.options else {
            return 0;
        };
        let mut read: Vec<&Flag> = Vec::new();
        for word in words {
            let args = [word];
            let [token] = tokenize_grammar(&args, grammar)[..] else {
                break;
            };
            if token.kind == ArgKind::DashDash {
                return read.len() + 1;
            }
            match token.flag {
                Some(flag) if !read.contains(&flag) => read.push(flag),
                _ => break,
            }
        }
        read.len()
    }

    /// Whether the word starts `compound`.
    pub fn opens(&self, compound: Compound) -> bool {
        self.role == Role::Opens(compound)
    }

    /// Whether the word ends `compound`.
    pub fn closes(&self, compound: Compound) -> bool {
        matches!(self.role, Role::Closes(compounds) if compounds.contains(&compound))
    }
}

const LOOPS: &[Compound] = &[
    Compound::For,
    Compound::Select,
    Compound::While,
    Compound::Until,
];

const fn row(word: Reserved, spelling: &'static str, next: Position, role: Role) -> ReservedWord {
    ReservedWord {
        word,
        spelling,
        next,
        names: false,
        role,
        options: None,
    }
}

impl ReservedWord {
    const fn with_options(self, options: Grammar) -> Self {
        ReservedWord {
            options: Some(options),
            ..self
        }
    }

    const fn naming(self) -> Self {
        ReservedWord {
            names: true,
            ..self
        }
    }
}

/// `time`'s one option, `-p`. Bash compares the word as written
/// (`special_case_tokens` in its `parse.y`): no clustering, no attached value
/// and no abbreviation, so `time "-p"` and `time -P` run a command of that
/// name.
const TIME_OPTIONS: Grammar = Grammar::exact(&[&[Flag::short("p")]]);

const COMMAND: Position = Position::Command;
const RESERVED_WORD: Position = Position::ReservedWord;
const WORD: Position = Position::Word;

/// Every bash reserved word. Bash reads one only unquoted and where a command
/// or a reserved word could start (and `in`/`do` after a `case`/`for`/
/// `select` header), so a caller matches a word's raw text here only at such
/// a position.
pub const RESERVED_WORDS: &[ReservedWord] = &[
    row(Reserved::If, "if", COMMAND, Role::Opens(Compound::If)),
    row(
        Reserved::Then,
        "then",
        COMMAND,
        Role::Continues(&[Compound::If]),
    ),
    row(
        Reserved::Elif,
        "elif",
        COMMAND,
        Role::Continues(&[Compound::If]),
    ),
    row(
        Reserved::Else,
        "else",
        COMMAND,
        Role::Continues(&[Compound::If]),
    ),
    row(
        Reserved::Fi,
        "fi",
        RESERVED_WORD,
        Role::Closes(&[Compound::If]),
    ),
    row(Reserved::Case, "case", WORD, Role::Opens(Compound::Case)),
    row(
        Reserved::In,
        "in",
        WORD,
        Role::Continues(&[Compound::Case, Compound::For, Compound::Select]),
    ),
    row(
        Reserved::Esac,
        "esac",
        RESERVED_WORD,
        Role::Closes(&[Compound::Case]),
    ),
    row(Reserved::For, "for", WORD, Role::Opens(Compound::For)).naming(),
    row(
        Reserved::Select,
        "select",
        WORD,
        Role::Opens(Compound::Select),
    )
    .naming(),
    row(
        Reserved::While,
        "while",
        COMMAND,
        Role::Opens(Compound::While),
    ),
    row(
        Reserved::Until,
        "until",
        COMMAND,
        Role::Opens(Compound::Until),
    ),
    row(Reserved::Do, "do", COMMAND, Role::Continues(LOOPS)),
    row(Reserved::Done, "done", RESERVED_WORD, Role::Closes(LOOPS)),
    row(
        Reserved::Function,
        "function",
        WORD,
        Role::Opens(Compound::Function),
    )
    .naming(),
    row(
        Reserved::Coproc,
        "coproc",
        COMMAND,
        Role::Opens(Compound::Coproc),
    )
    .naming(),
    row(Reserved::Time, "time", COMMAND, Role::Prefix).with_options(TIME_OPTIONS),
    row(Reserved::Bang, "!", COMMAND, Role::Prefix),
    row(
        Reserved::OpenBrace,
        "{",
        COMMAND,
        Role::Opens(Compound::Group),
    ),
    row(
        Reserved::CloseBrace,
        "}",
        RESERVED_WORD,
        Role::Closes(&[Compound::Group]),
    ),
    row(Reserved::OpenTest, "[[", WORD, Role::Opens(Compound::Test)),
    row(
        Reserved::CloseTest,
        "]]",
        RESERVED_WORD,
        Role::Closes(&[Compound::Test]),
    ),
];

/// The reserved word spelled exactly `text`. A quoted or escaped spelling is
/// an ordinary word to bash, so `text` is the word as written.
pub fn reserved_word(text: &str) -> Option<&'static ReservedWord> {
    let word = match text {
        "if" => Reserved::If,
        "then" => Reserved::Then,
        "elif" => Reserved::Elif,
        "else" => Reserved::Else,
        "fi" => Reserved::Fi,
        "case" => Reserved::Case,
        "in" => Reserved::In,
        "esac" => Reserved::Esac,
        "for" => Reserved::For,
        "select" => Reserved::Select,
        "while" => Reserved::While,
        "until" => Reserved::Until,
        "do" => Reserved::Do,
        "done" => Reserved::Done,
        "function" => Reserved::Function,
        "coproc" => Reserved::Coproc,
        "time" => Reserved::Time,
        "!" => Reserved::Bang,
        "{" => Reserved::OpenBrace,
        "}" => Reserved::CloseBrace,
        "[[" => Reserved::OpenTest,
        "]]" => Reserved::CloseTest,
        _ => return None,
    };
    RESERVED_WORDS.iter().find(|w| w.word == word)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `compgen -k | sort` under bash 5.3.9.
    const BASH_5_3_KEYWORDS: &[&str] = &[
        "!", "[[", "]]", "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for",
        "function", "if", "in", "select", "then", "time", "until", "while", "{", "}",
    ];

    #[test]
    fn table_is_bash_5_3_reserved_words() {
        let mut spellings: Vec<&str> = RESERVED_WORDS.iter().map(|w| w.spelling).collect();
        spellings.sort_unstable();
        assert_eq!(spellings, BASH_5_3_KEYWORDS);
    }

    #[test]
    fn each_word_is_one_row() {
        for (i, a) in RESERVED_WORDS.iter().enumerate() {
            for b in &RESERVED_WORDS[i + 1..] {
                assert_ne!(a.word, b.word, "{} and {}", a.spelling, b.spelling);
            }
        }
    }

    /// The compound commands of bash 5.3's `parse.y`, each with the reserved
    /// words its productions take, in their order: `if_command` and
    /// `elif_clause`, `case_command`, `for_command`, `select_command`,
    /// `while_command`, `until_command`, `group_command`, `cond_command`,
    /// `function_def` and `coproc`. `for NAME { … }` and `select NAME { … }`
    /// spell their body with a group's braces, which are the group's words
    /// here.
    const PRODUCTIONS: &[(Compound, &[&str])] = &[
        (Compound::If, &["if", "then", "elif", "else", "fi"]),
        (Compound::Case, &["case", "in", "esac"]),
        (Compound::For, &["for", "in", "do", "done"]),
        (Compound::Select, &["select", "in", "do", "done"]),
        (Compound::While, &["while", "do", "done"]),
        (Compound::Until, &["until", "do", "done"]),
        (Compound::Group, &["{", "}"]),
        (Compound::Test, &["[[", "]]"]),
        (Compound::Function, &["function"]),
        (Compound::Coproc, &["coproc"]),
    ];

    /// The compounds whose production holds `word` at an index `place` takes,
    /// given the production's length.
    fn compounds_where(word: &str, place: fn(usize, usize) -> bool) -> Vec<Compound> {
        PRODUCTIONS
            .iter()
            .filter(|(_, words)| {
                words
                    .iter()
                    .position(|w| *w == word)
                    .is_some_and(|at| place(at, words.len()))
            })
            .map(|(compound, _)| *compound)
            .collect()
    }

    /// Every row's role is what the productions make of its word: the first
    /// word of one opens it, the last word of a longer one closes it, a word
    /// between continues it, and a word in none of them prefixes a pipeline.
    #[test]
    fn roles_follow_bash_productions() {
        for row in RESERVED_WORDS {
            let word = row.spelling;
            let opens = compounds_where(word, |at, _| at == 0);
            let continues = compounds_where(word, |at, len| at > 0 && at + 1 < len);
            let closes = compounds_where(word, |at, len| at > 0 && at + 1 == len);
            let expected = match (opens.as_slice(), continues.as_slice(), closes.as_slice()) {
                ([compound], [], []) => Role::Opens(*compound),
                ([], [_, ..], []) => Role::Continues(&[]),
                ([], [], [_, ..]) => Role::Closes(&[]),
                ([], [], []) => Role::Prefix,
                _ => panic!("{word} plays more than one part"),
            };
            match (row.role, expected) {
                (Role::Continues(got), Role::Continues(_)) => {
                    assert_eq!(got, continues, "{word}")
                }
                (Role::Closes(got), Role::Closes(_)) => assert_eq!(got, closes, "{word}"),
                (got, expected) => assert_eq!(got, expected, "{word}"),
            }
        }
    }

    /// Checked under bash 5.3.9:
    /// - after each word in `Command` a command runs (`if echo A; then echo
    ///   B; …`, `while echo W; …`, `time echo T`, `coproc echo C`);
    /// - after each word in `ReservedWord` a reserved word is read
    ///   (`case x in x) [[ x ]] esac`, `… fi esac`, `… done esac`,
    ///   `{ ls; } esac`, `case y in y) :;; esac esac`, `if [[ b ]] then`)
    ///   and a command is a syntax error (`fi echo X`, `} echo X`);
    /// - after every other word the next word is a name, a subject, a list
    ///   word or an operand (`case echo in echo)`, `for echo in x`,
    ///   `function echo2 { …; }`, `for x in do done`, `[[ echo ]]`).
    #[test]
    fn next_as_bash_reads_it() {
        let at = |position: Position| -> Vec<&str> {
            RESERVED_WORDS
                .iter()
                .filter(|w| w.next == position)
                .map(|w| w.spelling)
                .collect()
        };
        assert_eq!(
            at(Position::ReservedWord),
            ["fi", "esac", "done", "}", "]]"]
        );
        assert_eq!(
            at(Position::Word),
            ["case", "in", "for", "select", "function", "[["]
        );
    }

    /// Checked under bash 5.3.9: `for x do`, `select x do`,
    /// `function f [[ … ]]` and `coproc NAME { … }` read a reserved word after
    /// the name, where `for x ls` is a syntax error and `coproc ls -l if` runs
    /// `ls -l if`.
    #[test]
    fn names_as_bash_reads_them() {
        let naming: Vec<&str> = RESERVED_WORDS
            .iter()
            .filter(|w| w.names)
            .map(|w| w.spelling)
            .collect();
        assert_eq!(naming, ["for", "select", "function", "coproc"]);
    }

    /// Checked under bash 5.3.9: `time -p`, `time --` and `time -p --` take
    /// a `[[ … ]]` after them, and in `time -- -p`, `time -p -p`, `time "-p"`
    /// and `time -P` the second word is a command's name (`-p: command not
    /// found`).
    #[test]
    fn options_as_bash_reads_them() {
        let with_options: Vec<&str> = RESERVED_WORDS
            .iter()
            .filter(|w| w.options.is_some())
            .map(|w| w.spelling)
            .collect();
        assert_eq!(with_options, ["time"]);
        let time = reserved_word("time").expect("a reserved word");
        for (words, read) in [
            (&["-p", "ls"][..], 1),
            (&["--", "ls"], 1),
            (&["-p", "--", "ls"], 2),
            (&["--", "-p"], 1),
            (&["-p", "-p"], 1),
            (&["\"-p\"", "ls"], 0),
            (&["-P", "ls"], 0),
            (&["-pp", "ls"], 0),
            (&["--p", "ls"], 0),
            (&["-p=1", "ls"], 0),
            (&["ls"], 0),
        ] {
            assert_eq!(time.options_read(words.iter().copied()), read, "{words:?}");
        }
        let if_word = reserved_word("if").expect("a reserved word");
        assert_eq!(if_word.options_read(["-p"]), 0);
    }

    #[test]
    fn loops_conditionals_and_case_hold_command_lists() {
        let word = |spelling| reserved_word(spelling).expect("a reserved word");
        for spelling in ["if", "case", "for", "select", "while", "until"] {
            assert!(word(spelling).opens_command_lists(), "{spelling}");
            assert!(!word(spelling).closes_command_lists(), "{spelling}");
        }
        for spelling in ["fi", "esac", "done"] {
            assert!(word(spelling).closes_command_lists(), "{spelling}");
            assert!(!word(spelling).opens_command_lists(), "{spelling}");
        }
        for spelling in ["then", "do", "in", "[[", "]]", "function", "coproc", "time"] {
            assert!(!word(spelling).opens_command_lists(), "{spelling}");
            assert!(!word(spelling).closes_command_lists(), "{spelling}");
        }
    }

    #[test]
    fn lookup_is_by_exact_spelling() {
        for row in RESERVED_WORDS {
            assert_eq!(reserved_word(row.spelling), Some(row), "{}", row.spelling);
        }
        for word in ["\"if\"", "\\if", "If", "[", "(", "noglob", "command", ""] {
            assert_eq!(reserved_word(word), None, "{word}");
        }
    }
}
