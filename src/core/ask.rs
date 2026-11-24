//! Terminal questions shared by every interactive command: an arrow-key menu
//! on crossterm (the same backend as the gain TUI) with a numbered fallback,
//! the yes/no form built on it, and `interactive()`, the check that decides
//! whether a menu can be shown at all. Callers keep their own non-interactive
//! behavior and only reach these functions when `interactive()` is true.
//! `in_ci()` marks a terminal that no one is likely to answer (a CI job with a
//! pseudo-terminal), where questions that did not exist before must not appear.
//! Keys typed before a menu opens, or left over after it closes, are dropped,
//! so a `y` + enter typed by habit cannot answer the next question.

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[cfg(feature = "engine")]
const NAV_HINT: &str = "  ↑/↓ choose · enter confirm · esc cancel";
const YES_NO_HINT: &str = "  ↑/↓ choose · y/n answer · enter confirm · esc cancel";
const YES_NO_KEYS: &[char] = &['y', 'n'];

pub(crate) fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub(crate) fn in_ci() -> bool {
    use crate::core::user_env;
    user_env::var("CI").is_some()
        || user_env::var("GITHUB_ACTIONS").is_some()
        || user_env::var("GITLAB_CI").is_some()
        || user_env::var("JENKINS_URL").is_some()
        || user_env::var("BUILDKITE").is_some()
}

fn drop_pending_keys() {
    use crossterm::event::{poll, read};
    while poll(std::time::Duration::ZERO).unwrap_or(false) {
        if read().is_err() {
            break;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Chosen(usize),
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Move(usize),
    Done(Outcome),
    Ignore,
}

fn step(idx: usize, len: usize, shortcuts: &[char], key: KeyEvent) -> Step {
    if key.kind != KeyEventKind::Press {
        return Step::Ignore;
    }
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Step::Move(if idx == 0 { len - 1 } else { idx - 1 }),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => Step::Move((idx + 1) % len),
        KeyCode::Char(c @ '1'..='9') => {
            let n = c as usize - '1' as usize;
            if n < len {
                Step::Done(Outcome::Chosen(n))
            } else {
                Step::Ignore
            }
        }
        KeyCode::Enter => Step::Done(Outcome::Chosen(idx)),
        KeyCode::Esc | KeyCode::Char('q') => Step::Done(Outcome::Cancelled),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Step::Done(Outcome::Interrupted)
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => match shortcuts
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&c))
            .filter(|n| *n < len)
        {
            Some(n) => Step::Done(Outcome::Chosen(n)),
            None => Step::Ignore,
        },
        _ => Step::Ignore,
    }
}

/// Arrow-key menu on crossterm (the same backend as the gain TUI): ↑/↓ or
/// j/k move, enter confirms, 1-9 pick directly, esc/q cancels. The menu
/// erases itself and leaves one line echoing the choice, so transcripts
/// stay readable. Terminals that refuse raw mode get a numbered prompt.
#[cfg(feature = "engine")]
pub(crate) fn select_menu(items: &[(&str, &str)], default: usize) -> Result<Option<usize>> {
    Ok(match run_menu(items, default, &[], NAV_HINT)? {
        Outcome::Chosen(i) => Some(i),
        Outcome::Cancelled | Outcome::Interrupted => None,
    })
}

#[cfg(feature = "engine")]
pub(crate) fn choose(
    question: &str,
    items: &[(&str, &str)],
    default: usize,
) -> Result<Option<usize>> {
    println!("{question}");
    finish(run_menu(items, default, &[], NAV_HINT)?)
}

pub(crate) fn confirm(question: &str, yes_hint: &str, no_hint: &str) -> Result<bool> {
    println!("{question}");
    let picked = finish(run_menu(
        &[("Yes", yes_hint), ("No", no_hint)],
        1,
        YES_NO_KEYS,
        YES_NO_HINT,
    )?)?;
    Ok(picked == Some(0))
}

fn finish(outcome: Outcome) -> Result<Option<usize>> {
    match outcome {
        Outcome::Chosen(i) => Ok(Some(i)),
        Outcome::Cancelled => Ok(None),
        Outcome::Interrupted => std::process::exit(130),
    }
}

fn run_menu(
    items: &[(&str, &str)],
    default: usize,
    shortcuts: &[char],
    footer: &str,
) -> Result<Outcome> {
    use colored::Colorize;
    use crossterm::event::{Event, read};
    use crossterm::{cursor, execute, terminal};
    use std::io::Write;

    if items.is_empty() {
        return Ok(Outcome::Cancelled);
    }
    if terminal::enable_raw_mode().is_err() {
        return select_menu_plain(items, default, shortcuts);
    }
    drop_pending_keys();
    let mut out = std::io::stdout();
    let mut idx = default.min(items.len() - 1);
    let render = |out: &mut std::io::Stdout, idx: usize| -> Result<()> {
        for (i, (label, hint)) in items.iter().enumerate() {
            if i == idx {
                write!(
                    out,
                    "  {} {}  {}\r\n",
                    "▶".green(),
                    label.green().bold(),
                    hint.dimmed()
                )
                .context("render menu")?;
            } else {
                write!(out, "    {label}  {}\r\n", hint.dimmed()).context("render menu")?;
            }
        }
        write!(out, "{}\r\n", footer.dimmed()).context("render menu")?;
        out.flush().context("flush menu")?;
        Ok(())
    };
    let lines = items.len() as u16 + 1;
    let mut run = || -> Result<Outcome> {
        render(&mut std::io::stdout(), idx)?;
        loop {
            let Event::Key(key) = read().context("read key")? else {
                continue;
            };
            match step(idx, items.len(), shortcuts, key) {
                Step::Done(outcome) => return Ok(outcome),
                Step::Ignore => continue,
                Step::Move(next) => idx = next,
            }
            execute!(
                out,
                cursor::MoveUp(lines),
                terminal::Clear(terminal::ClearType::FromCursorDown)
            )
            .context("redraw menu")?;
            render(&mut std::io::stdout(), idx)?;
        }
    };
    let result = run();
    drop_pending_keys();
    let _ = terminal::disable_raw_mode();
    let result = result?;
    execute!(
        out,
        cursor::MoveUp(lines),
        terminal::Clear(terminal::ClearType::FromCursorDown)
    )
    .ok();
    match result {
        Outcome::Chosen(i) => println!("  {} {}", "✓".green(), items[i].0),
        Outcome::Cancelled | Outcome::Interrupted => println!("  ✗ cancelled"),
    }
    Ok(result)
}

fn select_menu_plain(
    items: &[(&str, &str)],
    default: usize,
    shortcuts: &[char],
) -> Result<Outcome> {
    for (i, (label, hint)) in items.iter().enumerate() {
        println!("  {}. {label}  ({hint})", i + 1);
    }
    let answer = prompt(&format!("choice [{}]: ", default + 1))?;
    Ok(plain_answer(&answer, items.len(), default, shortcuts))
}

fn plain_answer(answer: &str, len: usize, default: usize, shortcuts: &[char]) -> Outcome {
    if answer.is_empty() {
        return Outcome::Chosen(default.min(len - 1));
    }
    if let Ok(n) = answer.parse::<usize>() {
        return if (1..=len).contains(&n) {
            Outcome::Chosen(n - 1)
        } else {
            Outcome::Cancelled
        };
    }
    let letter = match answer.to_ascii_lowercase().as_str() {
        "y" | "yes" => 'y',
        "n" | "no" => 'n',
        _ => return Outcome::Cancelled,
    };
    match shortcuts.iter().position(|s| *s == letter) {
        Some(n) if n < len => Outcome::Chosen(n),
        _ => Outcome::Cancelled,
    }
}

pub(crate) fn prompt(question: &str) -> Result<String> {
    use std::io::Write;
    print!("{question}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("read answer")?;
    Ok(line.trim().to_string())
}

const PICK_HINT: &str = "  ↑/↓ move · space select · enter confirm · esc cancel";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManyStep {
    Move(usize),
    Toggle(usize),
    Confirm,
    Cancel,
    Interrupt,
    Ignore,
}

fn step_many(idx: usize, len: usize, key: KeyEvent) -> ManyStep {
    if key.kind != KeyEventKind::Press {
        return ManyStep::Ignore;
    }
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            ManyStep::Move(if idx == 0 { len - 1 } else { idx - 1 })
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => ManyStep::Move((idx + 1) % len),
        KeyCode::Char(' ') => ManyStep::Toggle(idx),
        KeyCode::Char(c @ '1'..='9') => {
            let n = c as usize - '1' as usize;
            if n < len {
                ManyStep::Toggle(n)
            } else {
                ManyStep::Ignore
            }
        }
        KeyCode::Enter => ManyStep::Confirm,
        KeyCode::Esc | KeyCode::Char('q') => ManyStep::Cancel,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => ManyStep::Interrupt,
        _ => ManyStep::Ignore,
    }
}

pub(crate) fn pick_many(
    question: &str,
    items: &[(&str, &str)],
    selected: &[bool],
) -> Result<Option<Vec<usize>>> {
    use colored::Colorize;
    use crossterm::event::{Event, read};
    use crossterm::{cursor, execute, terminal};
    use std::io::Write;

    println!("{question}");
    let mut chosen: Vec<bool> = (0..items.len())
        .map(|i| selected.get(i).copied().unwrap_or(false))
        .collect();
    if items.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if terminal::enable_raw_mode().is_err() {
        return pick_many_plain(items, &chosen);
    }
    drop_pending_keys();
    let width = items
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let render = |idx: usize, chosen: &[bool]| -> Result<()> {
        let mut out = std::io::stdout();
        for (i, (label, hint)) in items.iter().enumerate() {
            let pad = " ".repeat(width - label.chars().count());
            let mark = if chosen[i] {
                "[x]".green().to_string()
            } else {
                "[ ]".to_string()
            };
            if i == idx {
                write!(
                    out,
                    "  {} {mark} {}{pad}  {}\r\n",
                    "▶".green(),
                    label.bold(),
                    hint.dimmed()
                )
            } else {
                write!(out, "    {mark} {label}{pad}  {}\r\n", hint.dimmed())
            }
            .context("render menu")?;
        }
        write!(out, "{}\r\n", PICK_HINT.dimmed()).context("render menu")?;
        out.flush().context("flush menu")
    };
    let lines = items.len() as u16 + 1;
    let mut idx = 0;
    let mut run = || -> Result<ManyStep> {
        render(idx, &chosen)?;
        loop {
            let Event::Key(key) = read().context("read key")? else {
                continue;
            };
            match step_many(idx, items.len(), key) {
                ManyStep::Move(next) => idx = next,
                ManyStep::Toggle(i) => chosen[i] = !chosen[i],
                ManyStep::Ignore => continue,
                done => return Ok(done),
            }
            execute!(
                std::io::stdout(),
                cursor::MoveUp(lines),
                terminal::Clear(terminal::ClearType::FromCursorDown)
            )
            .context("redraw menu")?;
            render(idx, &chosen)?;
        }
    };
    let result = run();
    drop_pending_keys();
    let _ = terminal::disable_raw_mode();
    let result = result?;
    execute!(
        std::io::stdout(),
        cursor::MoveUp(lines),
        terminal::Clear(terminal::ClearType::FromCursorDown)
    )
    .ok();
    let picked: Vec<usize> = (0..items.len()).filter(|i| chosen[*i]).collect();
    match result {
        ManyStep::Confirm if picked.is_empty() => println!("  ✗ nothing selected"),
        ManyStep::Confirm => {
            let names: Vec<&str> = picked.iter().map(|i| items[*i].0).collect();
            println!("  {} {}", "✓".green(), names.join(", "));
        }
        _ => println!("  ✗ cancelled"),
    }
    match result {
        ManyStep::Confirm => Ok(Some(picked)),
        ManyStep::Interrupt => std::process::exit(130),
        _ => Ok(None),
    }
}

fn pick_many_plain(items: &[(&str, &str)], chosen: &[bool]) -> Result<Option<Vec<usize>>> {
    for (i, (label, hint)) in items.iter().enumerate() {
        let mark = if chosen[i] { "x" } else { " " };
        println!("  {}. [{mark}] {label}  ({hint})", i + 1);
    }
    let defaults: Vec<String> = (0..items.len())
        .filter(|i| chosen[*i])
        .map(|i| (i + 1).to_string())
        .collect();
    let answer = prompt(&format!(
        "numbers to select, separated by spaces [{}]: ",
        defaults.join(" ")
    ))?;
    Ok(parse_numbers(&answer, items.len(), chosen))
}

fn parse_numbers(answer: &str, len: usize, chosen: &[bool]) -> Option<Vec<usize>> {
    if answer.trim().is_empty() {
        return Some((0..len).filter(|i| chosen[*i]).collect());
    }
    let mut out = Vec::new();
    for token in answer
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
    {
        match token.parse::<usize>() {
            Ok(n) if (1..=len).contains(&n) => {
                if !out.contains(&(n - 1)) {
                    out.push(n - 1);
                }
            }
            _ => return None,
        }
    }
    out.sort_unstable();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn arrows_and_vim_keys_wrap_around() {
        assert_eq!(step(0, 3, &[], key(KeyCode::Up)), Step::Move(2));
        assert_eq!(step(2, 3, &[], key(KeyCode::Down)), Step::Move(0));
        assert_eq!(step(1, 3, &[], key(KeyCode::Char('k'))), Step::Move(0));
        assert_eq!(step(1, 3, &[], key(KeyCode::Char('j'))), Step::Move(2));
        assert_eq!(step(1, 3, &[], key(KeyCode::Tab)), Step::Move(2));
    }

    #[test]
    fn enter_picks_the_highlighted_item_and_digits_pick_directly() {
        assert_eq!(
            step(1, 2, &[], key(KeyCode::Enter)),
            Step::Done(Outcome::Chosen(1))
        );
        assert_eq!(
            step(1, 3, &[], key(KeyCode::Char('1'))),
            Step::Done(Outcome::Chosen(0))
        );
        assert_eq!(step(0, 2, &[], key(KeyCode::Char('3'))), Step::Ignore);
    }

    #[test]
    fn esc_and_q_cancel_while_ctrl_c_interrupts() {
        assert_eq!(
            step(0, 2, &[], key(KeyCode::Esc)),
            Step::Done(Outcome::Cancelled)
        );
        assert_eq!(
            step(0, 2, &[], key(KeyCode::Char('q'))),
            Step::Done(Outcome::Cancelled)
        );
        assert_eq!(step(0, 2, &[], ctrl('c')), Step::Done(Outcome::Interrupted));
    }

    #[test]
    fn yes_no_shortcuts_answer_in_either_case() {
        assert_eq!(
            step(1, 2, YES_NO_KEYS, key(KeyCode::Char('y'))),
            Step::Done(Outcome::Chosen(0))
        );
        assert_eq!(
            step(0, 2, YES_NO_KEYS, key(KeyCode::Char('N'))),
            Step::Done(Outcome::Chosen(1))
        );
        assert_eq!(step(1, 2, &[], key(KeyCode::Char('y'))), Step::Ignore);
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut release = key(KeyCode::Enter);
        release.kind = KeyEventKind::Release;
        assert_eq!(step(0, 2, &[], release), Step::Ignore);
    }

    #[test]
    fn plain_fallback_keeps_numbers_and_accepts_yes_no_words() {
        assert_eq!(plain_answer("", 2, 1, YES_NO_KEYS), Outcome::Chosen(1));
        assert_eq!(plain_answer("1", 2, 1, YES_NO_KEYS), Outcome::Chosen(0));
        assert_eq!(plain_answer("7", 2, 1, YES_NO_KEYS), Outcome::Cancelled);
        assert_eq!(plain_answer("yes", 2, 1, YES_NO_KEYS), Outcome::Chosen(0));
        assert_eq!(plain_answer("Y", 2, 1, YES_NO_KEYS), Outcome::Chosen(0));
        assert_eq!(plain_answer("no", 2, 1, YES_NO_KEYS), Outcome::Chosen(1));
        assert_eq!(plain_answer("yep", 2, 1, YES_NO_KEYS), Outcome::Cancelled);
        assert_eq!(plain_answer("yes", 3, 0, &[]), Outcome::Cancelled);
    }

    #[test]
    fn checkbox_keys_move_toggle_and_finish() {
        assert_eq!(step_many(0, 3, key(KeyCode::Up)), ManyStep::Move(2));
        assert_eq!(step_many(2, 3, key(KeyCode::Char('j'))), ManyStep::Move(0));
        assert_eq!(
            step_many(1, 3, key(KeyCode::Char(' '))),
            ManyStep::Toggle(1)
        );
        assert_eq!(
            step_many(0, 3, key(KeyCode::Char('3'))),
            ManyStep::Toggle(2)
        );
        assert_eq!(step_many(0, 3, key(KeyCode::Char('4'))), ManyStep::Ignore);
        assert_eq!(step_many(0, 3, key(KeyCode::Enter)), ManyStep::Confirm);
        assert_eq!(step_many(0, 3, key(KeyCode::Esc)), ManyStep::Cancel);
        assert_eq!(step_many(0, 3, ctrl('c')), ManyStep::Interrupt);
    }

    #[test]
    fn checkbox_fallback_parses_numbers_and_keeps_defaults_on_enter() {
        let chosen = [true, false, true];
        assert_eq!(parse_numbers("", 3, &chosen), Some(vec![0, 2]));
        assert_eq!(parse_numbers("2", 3, &chosen), Some(vec![1]));
        assert_eq!(parse_numbers("3, 1 3", 3, &chosen), Some(vec![0, 2]));
        assert_eq!(parse_numbers("4", 3, &chosen), None);
        assert_eq!(parse_numbers("claude", 3, &chosen), None);
    }
}
