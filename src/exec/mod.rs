//! Sam style language for running edit commands using structural regular expressions
use crate::{
    buffer::{Buffer, GapBuffer},
    dot::{Cur, Dot},
    editor::Action,
    regex::{self, Match},
};
use ad_event::Source;
use std::{cmp::min, io::Write, iter::Peekable, str::Chars};

mod addr;
mod cached_stdin;
mod char_iter;
mod expr;

use addr::ParseError;
pub(crate) use addr::{Addr, AddrBase, Address};
pub use cached_stdin::CachedStdin;
pub(crate) use char_iter::IterBoundedChars;
use expr::{Expr, ParseOutput};

/// Variable usable in templates for injecting the current filename.
/// (Following the naming convention used in Awk)
const FNAME_VAR: &str = "$FILENAME";

/// Errors that can be returned by the exec engine
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Empty expression group
    EmptyExpressionGroup,
    /// Empty branch for an expression group
    EmptyExpressionGroupBranch,
    /// Empty program
    EmptyProgram,
    /// Unexpected end of file
    Eof,
    /// Invalid regex
    InvalidRegex(regex::Error),
    /// Invalid substitution
    InvalidSubstitution(usize),
    /// Invalid suffix
    InvalidSuffix,
    /// Missing action
    MissingAction,
    /// Missing delimiter
    MissingDelimiter(&'static str),
    /// Unclosed delimiter
    UnclosedDelimiter(&'static str, char),
    /// Unclosed expression group
    UnclosedExpressionGroup,
    /// Unclosed expression group branch
    UnclosedExpressionGroupBranch,
    /// Unexpected character
    UnexpectedCharacter(char),
}

impl From<regex::Error> for Error {
    fn from(err: regex::Error) -> Self {
        Error::InvalidRegex(err)
    }
}

/// Something that can be edited by a Program
pub trait Edit: Address {
    /// Extract the content of a previous submatch so it can be used in templating
    fn submatch(&self, m: &Match, n: usize) -> Option<String> {
        let (from, to) = m.sub_loc(n)?;
        Some(self.iter_between(from, to).map(|(_, ch)| ch).collect())
    }

    /// Insert a string at the specified index
    fn insert(&mut self, ix: usize, s: &str);

    /// Remove all characters from (from..to)
    fn remove(&mut self, from: usize, to: usize);

    /// Mark the start of an edit transaction
    fn begin_edit_transaction(&mut self) {}

    /// Mark the end of an edit transaction
    fn end_edit_transaction(&mut self) {}
}

impl Edit for GapBuffer {
    fn insert(&mut self, idx: usize, s: &str) {
        self.insert_str(idx, s)
    }

    fn remove(&mut self, from: usize, to: usize) {
        self.remove_range(from, to);
    }
}

impl Edit for Buffer {
    fn insert(&mut self, idx: usize, s: &str) {
        self.dot = Dot::Cur { c: Cur { idx } };
        self.handle_action(Action::InsertString { s: s.to_string() }, Source::Fsys);
    }

    fn remove(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        self.dot = Dot::from_char_indices(from, to.saturating_sub(1)).collapse_null_range();
        self.handle_action(Action::Delete, Source::Fsys);
    }

    fn begin_edit_transaction(&mut self) {
        self.new_edit_log_transaction()
    }

    fn end_edit_transaction(&mut self) {
        self.new_edit_log_transaction()
    }
}

/// A parsed and compiled program that can be executed against an input
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    initial_dot: Addr,
    exprs: Vec<Expr>,
}

impl Program {
    /// Attempt to parse a given program input
    pub fn try_parse(s: &str) -> Result<Self, Error> {
        let mut exprs = vec![];
        let mut it = s.trim().chars().peekable();

        if it.peek().is_none() {
            return Err(Error::EmptyProgram);
        }

        let initial_dot = match Addr::parse(&mut it) {
            Ok(dot_expr) => dot_expr,

            // If the start of input is not an address we default to Full and attempt to parse the
            // rest of the program. We need to reconstruct the iterator here as we may have
            // advanced through the string while we attempt to parse the initial address.
            Err(ParseError::NotAnAddress) => {
                it = s.trim().chars().peekable();
                Addr::full()
            }

            Err(ParseError::InvalidRegex(e)) => return Err(Error::InvalidRegex(e)),
            Err(ParseError::UnclosedDelimiter) => {
                return Err(Error::UnclosedDelimiter("dot expr regex", '/'))
            }
            Err(ParseError::UnexpectedCharacter(c)) => return Err(Error::UnexpectedCharacter(c)),
            Err(ParseError::InvalidSuffix) => return Err(Error::InvalidSuffix),
        };

        consume_whitespace(&mut it);

        loop {
            if it.peek().is_none() {
                break;
            }

            match Expr::try_parse(&mut it) {
                Ok(ParseOutput::Single(expr)) => {
                    exprs.push(expr);
                    consume_whitespace(&mut it);
                }
                Ok(ParseOutput::Pair(e1, e2)) => {
                    exprs.extend([e1, e2]);
                    consume_whitespace(&mut it);
                }
                Err(Error::Eof) => break,
                Err(e) => return Err(e),
            }
        }

        if exprs.is_empty() {
            return Ok(Self { initial_dot, exprs });
        }

        validate(&exprs)?;

        Ok(Self { initial_dot, exprs })
    }

    /// Execute this program against a given Edit
    pub fn execute<E, W>(&mut self, ed: &mut E, fname: &str, out: &mut W) -> Result<Dot, Error>
    where
        E: Edit,
        W: Write,
    {
        let initial_dot = ed.map_addr(&mut self.initial_dot);

        if self.exprs.is_empty() {
            return Ok(initial_dot);
        }

        let (from, to) = initial_dot.as_char_indices();
        let initial = &Match::synthetic(from, to.saturating_add(1));

        ed.begin_edit_transaction();
        let (from, to) = self.step(ed, initial, 0, fname, out)?.as_char_indices();
        ed.end_edit_transaction();

        // In the case of running against a lazy stream our initial `to` will be a sential value of
        // usize::MAX which needs to be clamped to the size of the input. For Buffers and GapBuffers
        // where we know that we should already be in bounds this is not required but the overhead
        // of always doing it is minimal as checking the number of chars in the buffer is O(1) due
        // to us caching the value.
        let ix_max = ed.len_chars();

        Ok(Dot::from_char_indices(min(from, ix_max), min(to, ix_max)))
    }

    fn step<E, W>(
        &mut self,
        ed: &mut E,
        m: &Match,
        pc: usize,
        fname: &str,
        out: &mut W,
    ) -> Result<Dot, Error>
    where
        E: Edit,
        W: Write,
    {
        let (mut from, to) = m.loc();

        match self.exprs[pc].clone() {
            Expr::Group(g) => {
                let mut dot = Dot::from_char_indices(from, to);
                for exprs in g {
                    let mut p = Program {
                        initial_dot: Addr::Explicit(dot),
                        exprs: exprs.clone(),
                    };
                    dot = p.step(ed, m, 0, fname, out)?;
                }

                Ok(dot)
            }

            Expr::LoopMatches(mut re) => {
                let mut initial_matches = Vec::new();
                while let Some(m) = re.match_iter(&mut ed.iter_between(from, to), from) {
                    // It's possible for the Regex we're using to match a 0-length string which
                    // would cause us to get stuck trying to advance to the next match position.
                    // If this happens we advance from by a character to ensure that we search
                    // further in the input.
                    let mut new_from = m.loc().1;
                    if new_from == from {
                        new_from += 1;
                    }
                    from = new_from;

                    initial_matches.push(m);

                    if from >= to || from >= ed.max_iter() {
                        break;
                    }
                }

                self.apply_matches(initial_matches, ed, m, pc, fname, out)
            }

            Expr::LoopBetweenMatches(mut re) => {
                let mut initial_matches = Vec::new();

                while let Some(m) = re.match_iter(&mut ed.iter_between(from, to), from) {
                    let (new_from, new_to) = m.loc();
                    if from < new_from {
                        initial_matches.push(Match::synthetic(from, new_from));
                    }
                    from = new_to;
                    if from > to || from >= ed.max_iter() {
                        break;
                    }
                }

                if from < to {
                    initial_matches.push(Match::synthetic(from, to));
                }

                self.apply_matches(initial_matches, ed, m, pc, fname, out)
            }

            Expr::IfContains(mut re) => {
                if re.matches_iter(&mut ed.iter_between(from, to), from) {
                    self.step(ed, m, pc + 1, fname, out)
                } else {
                    Ok(Dot::from_char_indices(from, to))
                }
            }

            Expr::IfNotContains(mut re) => {
                if !re.matches_iter(&mut ed.iter_between(from, to), from) {
                    self.step(ed, m, pc + 1, fname, out)
                } else {
                    Ok(Dot::from_char_indices(from, to))
                }
            }

            Expr::Print(pat) => {
                let s = template_match(&pat, m, ed, fname)?;
                write!(out, "{s}").expect("to be able to write");
                Ok(Dot::from_char_indices(from, to))
            }

            Expr::Insert(pat) => {
                let s = template_match(&pat, m, ed, fname)?;
                ed.insert(from, &s);
                Ok(Dot::from_char_indices(from, to + s.chars().count()))
            }

            Expr::Append(pat) => {
                let s = template_match(&pat, m, ed, fname)?;
                ed.insert(to, &s);
                Ok(Dot::from_char_indices(from, to + s.chars().count()))
            }

            Expr::Change(pat) => {
                let s = template_match(&pat, m, ed, fname)?;
                ed.remove(from, to);
                ed.insert(from, &s);
                Ok(Dot::from_char_indices(from, from + s.chars().count()))
            }

            Expr::Delete => {
                ed.remove(from, to);
                Ok(Dot::from_char_indices(from, from))
            }

            Expr::Sub(mut re, pat) => match re.match_iter(&mut ed.iter_between(from, to), from) {
                Some(m) => {
                    let (mfrom, mto) = m.loc();
                    let s = template_match(&pat, &m, ed, fname)?;
                    ed.remove(mfrom, mto);
                    ed.insert(mfrom, &s);
                    Ok(Dot::from_char_indices(
                        from,
                        to - (mto - mfrom) + s.chars().count(),
                    ))
                }
                None => Ok(Dot::from_char_indices(from, to)),
            },
        }
    }

    /// When looping over disjoint matches in the input we need to determine all of the initial
    /// match points before we start making any edits as the edits may alter the semantics of
    /// future matches.
    fn apply_matches<E, W>(
        &mut self,
        initial_matches: Vec<Match>,
        ed: &mut E,
        m: &Match,
        pc: usize,
        fname: &str,
        out: &mut W,
    ) -> Result<Dot, Error>
    where
        E: Edit,
        W: Write,
    {
        let mut offset: isize = 0;
        let (from, to) = m.loc();
        let mut dot = Dot::from_char_indices(from, to);

        for mut m in initial_matches.into_iter() {
            m.apply_offset(offset);

            let cur_len = ed.len_chars();
            dot = self.step(ed, &m, pc + 1, fname, out)?;
            let new_len = ed.len_chars();
            offset += new_len as isize - cur_len as isize;
        }

        Ok(dot)
    }
}

fn consume_whitespace(it: &mut Peekable<Chars<'_>>) {
    loop {
        match it.peek() {
            Some(ch) if ch.is_whitespace() => {
                it.next();
            }
            _ => break,
        }
    }
}

fn validate(exprs: &[Expr]) -> Result<(), Error> {
    use Expr::*;

    if exprs.is_empty() {
        return Err(Error::EmptyProgram);
    }

    // Groups branches must be valid sub-programs
    for e in exprs.iter() {
        if let Group(branches) = e {
            for branch in branches.iter() {
                validate(branch)?;
            }
        }
    }

    // Must end with an action
    if !matches!(
        exprs[exprs.len() - 1],
        Group(_) | Insert(_) | Append(_) | Change(_) | Sub(_, _) | Print(_) | Delete
    ) {
        return Err(Error::MissingAction);
    }

    Ok(())
}

// FIXME: if a previous sub-match replacement injects a valid var name for a subsequent one
// then we end up attempting to template THAT in a later iteration of the loop.
fn template_match<E>(s: &str, m: &Match, ed: &E, fname: &str) -> Result<String, Error>
where
    E: Edit,
{
    let mut output = if s.contains(FNAME_VAR) {
        s.replace(FNAME_VAR, fname)
    } else {
        s.to_string()
    };

    // replace newline and tab escapes with their literal equivalents
    output = output.replace("\\n", "\n").replace("\\t", "\t");

    let vars = ["$0", "$1", "$2", "$3", "$4", "$5", "$6", "$7", "$8", "$9"];
    for (n, var) in vars.iter().enumerate() {
        if !s.contains(var) {
            continue;
        }
        match ed.submatch(m, n) {
            Some(sm) => output = output.replace(var, &sm.to_string()),
            None => return Err(Error::InvalidSubstitution(n)),
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffer::Buffer, editor::Action, regex::Regex};
    use simple_test_case::test_case;
    use Expr::*;

    fn re(s: &str) -> Regex {
        Regex::compile(s).unwrap()
    }

    #[test_case(", p/$0/", vec![Print("$0".to_string())]; "print all")]
    #[test_case(", x/^.*$/ s/foo/bar/", vec![LoopMatches(re("^.*$")), Sub(re("foo"), "bar".to_string())]; "simple loop")]
    #[test_case(", x/^.*$/ g/emacs/ d", vec![LoopMatches(re("^.*$")), IfContains(re("emacs")), Delete]; "loop filter")]
    #[test]
    fn parse_program_works(s: &str, expected: Vec<Expr>) {
        let p = Program::try_parse(s).expect("valid input");
        assert_eq!(
            p,
            Program {
                initial_dot: Addr::full(),
                exprs: expected
            }
        );
    }

    #[test_case("", Error::EmptyProgram; "empty program")]
    #[test_case(", x/.*/", Error::MissingAction; "missing action")]
    #[test]
    fn parse_program_errors_correctly(s: &str, expected: Error) {
        let res = Program::try_parse(s);
        assert_eq!(res, Err(expected));
    }

    #[test_case(vec![Insert("X".to_string())], "Xfoo foo foo", (0, 12); "insert")]
    #[test_case(vec![Append("X".to_string())], "foo foo fooX", (0, 12); "append")]
    #[test_case(vec![Change("X".to_string())], "X", (0, 1); "change")]
    #[test_case(vec![Delete], "", (0, 0); "delete")]
    #[test_case(vec![Sub(re("oo"), "X".to_string())], "fX foo foo", (0, 10); "sub single")]
    #[test_case(vec![LoopMatches(re("foo")), Delete], "  ", (2, 2); "loop delete")]
    #[test_case(vec![LoopBetweenMatches(re("foo")), Delete], "foofoofoo", (6, 6); "loop between delete")]
    #[test_case(vec![LoopMatches(re("foo")), Append("X".to_string())], "fooX fooX fooX", (10, 14); "loop change")]
    #[test_case(vec![LoopBetweenMatches(re("foo")), Append("X".to_string())], "foo Xfoo Xfoo", (8, 10); "loop between change")]
    #[test]
    fn step_works(exprs: Vec<Expr>, expected: &str, expected_dot: (usize, usize)) {
        let mut prog = Program {
            initial_dot: Addr::full(),
            exprs,
        };
        let mut b = Buffer::new_unnamed(0, "foo foo foo");
        let dot = prog
            .step(&mut b, &Match::synthetic(0, 11), 0, "test", &mut vec![])
            .unwrap();

        assert_eq!(&b.txt.to_string(), expected);
        assert_eq!(dot.as_char_indices(), expected_dot);
    }

    #[test_case(", x/(t.)/ c/$1X/", "thXis is a teXst XstrXing"; "x c")]
    #[test_case(", x/(t.)/ i/$1/", "ththis is a tetest t strtring"; "x i")]
    #[test_case(", x/(t.)/ a/$1/", "ththis is a tetest t strtring"; "x a")]
    #[test]
    fn substitution_of_submatches_works(s: &str, expected: &str) {
        let mut prog = Program::try_parse(s).unwrap();

        let mut b = Buffer::new_unnamed(0, "this is a test string");
        prog.execute(&mut b, "test", &mut vec![]).unwrap();
        assert_eq!(&b.txt.to_string(), expected);
    }

    #[test]
    fn loop_between_generates_the_correct_blocks() {
        let mut prog = Program::try_parse(", y/ / p/>$0<\n/").unwrap();
        let mut b = Buffer::new_unnamed(0, "this and that");
        let mut output = Vec::new();
        let dot = prog.execute(&mut b, "test", &mut output).unwrap();

        let s = String::from_utf8(output).unwrap();
        assert_eq!(s, ">this<\n>and<\n>that<\n");

        let dot_content = dot.content(&b);
        assert_eq!(dot_content, "that");
    }

    #[test_case(0, "/oo.fo/ d", "fo│foo"; "regex dot delete")]
    #[test_case(2, "-/f/,/f/ d", "oo│foo"; "regex dot range delete")]
    #[test_case(0, ", x/foo/ p/$0/", "foo│foo│foo"; "x print")]
    #[test_case(0, ", x/foo/ i/X/", "Xfoo│Xfoo│Xfoo"; "x insert")]
    #[test_case(0, ", x/foo/ a/X/", "fooX│fooX│fooX"; "x append")]
    #[test_case(0, ", x/foo/ c/X/", "X│X│X"; "x change")]
    #[test_case(0, ", x/foo/ c/XX/", "XX│XX│XX"; "x change 2")]
    #[test_case(0, ", x/foo/ d", "││"; "x delete")]
    #[test_case(0, ", x/foo/ s/o/X/", "fXo│fXo│fXo"; "x substitute")]
    #[test_case(0, ", y/foo/ p/>$0</", "foo│foo│foo"; "y print")]
    #[test_case(0, ", y/foo/ i/X/", "fooX│fooX│fooX"; "y insert")]
    #[test_case(0, ", y/foo/ a/X/", "foo│Xfoo│XfooX"; "y append")]
    #[test_case(0, ", y/foo/ c/X/", "fooXfooXfooX"; "y change")]
    #[test_case(0, ", y/foo/ d", "foofoofoo"; "y delete")]
    #[test_case(0, ", y/│/ d", "││"; "y delete 2")]
    #[test_case(0, ", s/oo/X/", "fX│foo│foo"; "sub single")]
    #[test_case(0, ", s/\\w+/X/", "X│foo│foo"; "sub word single")]
    #[test_case(0, ", s/oo/X/g", "fX│fX│fX"; "sub all")]
    #[test_case(0, ", s/.*/X/g", "X"; "sub all dot star")]
    #[test_case(0, ", x/\\b\\w+\\b/ c/X/", "X│X│X"; "change each word")]
    #[test_case(0, ", x/foo/ s/o/X/g", "fXX│fXX│fXX"; "nested loop x substitute all")]
    #[test_case(0, ", x/oo/ s/.*/X/g", "fX│fX│fX"; "nested loop x sub all dot star")]
    #[test]
    fn execute_produces_the_correct_string(idx: usize, s: &str, expected: &str) {
        let mut prog = Program::try_parse(s).unwrap();
        let mut b = Buffer::new_unnamed(0, "foo│foo│foo");
        b.dot = Cur::new(idx).into();
        prog.execute(&mut b, "test", &mut vec![]).unwrap();

        assert_eq!(&b.txt.to_string(), expected, "buffer");
    }

    #[test]
    fn multiline_file_dot_star_works() {
        let mut prog = Program::try_parse(", x/.*/ c/foo/").unwrap();
        let mut b = Buffer::new_unnamed(0, "this is\na multiline\nfile");
        prog.execute(&mut b, "test", &mut vec![]).unwrap();

        // '.*' will match the null string at the end of lines containing a newline as well
        assert_eq!(&b.txt.to_string(), "foofoo\nfoofoo\nfoo");
    }

    #[test]
    fn multiline_file_dot_plus_works() {
        let mut prog = Program::try_parse(", x/.+/ c/foo/").unwrap();
        let mut b = Buffer::new_unnamed(0, "this is\na multiline\nfile");
        prog.execute(&mut b, "test", &mut vec![]).unwrap();

        assert_eq!(&b.txt.to_string(), "foo\nfoo\nfoo");
    }

    #[test_case(", d"; "delete buffer")]
    #[test_case(", x/th/ d"; "delete each th")]
    #[test_case(", x/ / d"; "delete spaces")]
    #[test_case(", s/ //g"; "sub remove spaces")]
    #[test_case(", x/\\b\\w+\\b/ d"; "delete each word")]
    #[test_case(", x/. / d"; "delete things before a space")]
    #[test_case(", x/\\b\\w+\\b/ c/buffalo/"; "change each word")]
    #[test_case(", x/\\b\\w+\\b/ a/buffalo/"; "append to each word")]
    #[test_case(", x/\\b\\w+\\b/ i/buffalo/"; "insert before each word")]
    #[test]
    fn buffer_execute_undo_all_is_a_noop(s: &str) {
        let mut prog = Program::try_parse(s).unwrap();
        let initial_content = "this is a line\nand another\n- [ ] something to do\n";
        let mut b = Buffer::new_unnamed(0, initial_content);

        prog.execute(&mut b, "test", &mut vec![]).unwrap();
        while b.handle_action(Action::Undo, Source::Keyboard).is_none() {}
        let mut final_content = String::from_utf8(b.contents()).unwrap();
        final_content.pop(); // The newline that we append

        assert_eq!(&final_content, initial_content);
    }

    // regression test: gh#30
    #[test]
    fn regression_edit_landing_on_gap_end() {
        // This is line 1 of the test file
        let s = r#"6,32 x/.*\n/ y/\s+-- / x/(.+)/ c/"$1"/"#;
        let mut prog = Program::try_parse(s).unwrap();
        let mut b = Buffer::new_unnamed(
            0,
            include_str!("../../data/regression/edit-landing-on-gap-end.txt"),
        );
        prog.execute(&mut b, "test", &mut vec![]).unwrap();

        let expected = r#""w | write"             -- "save the current buffer to disk. (Blocked if the file has been modified on disk)""#;
        let final_content = String::from_utf8(b.contents()).unwrap();
        assert_eq!(final_content.lines().nth(29).unwrap(), expected);
    }
}
