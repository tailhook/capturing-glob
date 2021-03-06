// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Support for matching file paths against Unix shell style patterns.
//!
//! The `glob` and `glob_with` functions allow querying the filesystem for all
//! files that match a particular pattern (similar to the libc `glob` function).
//! The methods on the `Pattern` type provide functionality for checking if
//! individual paths match a particular pattern (similar to the libc `fnmatch`
//! function).
//!
//! For consistency across platforms, and for Windows support, this module
//! is implemented entirely in Rust rather than deferring to the libc
//! `glob`/`fnmatch` functions.
//!
//! # Examples
//!
//! To print all jpg files in `/media/` and all of its subdirectories,
//! extracting stem and a directory name while matching.
//!
//! ```rust,no_run
//! use capturing_glob::glob;
//!
//! for entry in glob("/media/(**/*).jpg").expect("Failed to read glob pattern") {
//!     match entry {
//!         Ok(entry) => {
//!             println!("{:?} -> {:?}", entry.path().display(),
//!                 entry.group(1).unwrap());
//!         }
//!         Err(e) => eprintln!("{:?}", e),
//!     }
//! }
//! ```
//!
//! To print all files containing the letter "a", case insensitive, in a `local`
//! directory relative to the current working directory. This ignores errors
//! instead of printing them.
//!
//! ```rust,no_run
//! use capturing_glob::glob_with;
//! use capturing_glob::MatchOptions;
//!
//! let options = MatchOptions {
//!     case_sensitive: false,
//!     require_literal_separator: false,
//!     require_literal_leading_dot: false,
//! };
//! for entry in glob_with("local/*a*", &options).unwrap() {
//!     if let Ok(entry) = entry {
//!         println!("{:?}", entry.path().display())
//!     }
//! }
//! ```
//!
//! # Substitute Names
//!
//! Reverse conversion where you have a name and pattern and want to get
//! a full path is also possible:
//!
//! ```rust
//! # use std::error::Error;
//! use capturing_glob::Pattern;
//!
//! # fn run() -> Result<(), Box<Error>> {
//! assert_eq!(Pattern::new("images/(*).jpg")?.substitute(&["cat"])?,
//!            "images/cat.jpg");
//! assert_eq!(Pattern::new("images/(*.jpg)")?.substitute(&["cat.jpg"])?,
//!            "images/cat.jpg");
//! # Ok(())
//! # }
//! # fn main() { run().unwrap() }
//! ```
//!
//! Note: we don't check substituted pattern. So the following is possible:
//!
//! ```rust
//! # use std::error::Error;
//! use capturing_glob::Pattern;
//!
//! # fn run() -> Result<(), Box<Error>> {
//! let pattern = Pattern::new("images/(*.jpg)")?;
//! assert_eq!(pattern.substitute(&["cat.png"])?, "images/cat.png");
//! assert!(!pattern.matches(&pattern.substitute(&["cat.png"])?));
//! # Ok(())
//! # }
//! # fn main() { run().unwrap() }
//! ```
//!

#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![cfg_attr(all(test, windows), feature(std_misc))]

mod entry;

pub use entry::Entry;

use std::ascii::AsciiExt;
use std::cmp;
use std::fmt;
use std::fs;
use std::io;
use std::path::{self, Path, PathBuf, Component};
use std::str::FromStr;
use std::error::Error;

use CharSpecifier::{SingleChar, CharRange};
use MatchResult::{Match, SubPatternDoesntMatch, EntirePatternDoesntMatch};

/// An iterator that yields Entry'ies that match a particular pattern.
///
/// Each entry conains matching filename and also capture groups.
///
/// Note that it yields `GlobResult` in order to report any `IoErrors` that may
/// arise during iteration. If a directory matches but is unreadable,
/// thereby preventing its contents from being checked for matches, a
/// `GlobError` is returned to express this.
///
/// See the `glob` function for more details.
#[derive(Debug)]
pub struct Entries {
    whole_pattern: Pattern,
    dir_patterns: Vec<Pattern>,
    require_dir: bool,
    options: MatchOptions,
    todo: Vec<Result<(PathBuf, usize), GlobError>>,
    scope: Option<PathBuf>,
}

/// Return an iterator that produces all the paths and capture groups that
/// match the given pattern using default match options, which may be absolute
/// or relative to the current working directory.
///
/// This may return an error if the pattern is invalid.
///
/// This method uses the default match options and is equivalent to calling
/// `glob_with(pattern, MatchOptions::new())`. Use `glob_with` directly if you
/// want to use non-default match options.
///
/// When iterating, each result is a `GlobResult` which expresses the
/// possibility that there was an `IoError` when attempting to read the contents
/// of the matched path.  In other words, each item returned by the iterator
/// will either be an `Ok(Path)` if the path matched, or an `Err(GlobError)` if
/// the path (partially) matched _but_ its contents could not be read in order
/// to determine if its contents matched.
///
/// See the `Entries` documentation for more information.
///
/// # Examples
///
/// Consider a directory `/media/pictures` containing only the files
/// `kittens.jpg`, `puppies.jpg` and `hamsters.gif`:
///
/// ```rust,no_run
/// use capturing_glob::glob;
///
/// for entry in glob("/media/pictures/(*).jpg").unwrap() {
///     match entry {
///         Ok(entry) => {
///             println!("{:?} -> {:?}",
///                 entry.path().display(),
///                 entry.group(1).unwrap());
///         }
///
///         // if the path matched but was unreadable,
///         // thereby preventing its contents from matching
///         Err(e) => println!("{:?}", e),
///     }
/// }
/// ```
///
/// The above code will print:
///
/// ```ignore
/// /media/pictures/kittens.jpg -> kittens
/// /media/pictures/puppies.jpg -> puppies
/// ```
///
/// If you want to ignore unreadable paths, you can use something like
/// `filter_map`:
///
/// ```rust
/// use capturing_glob::glob;
/// use std::result::Result;
///
/// for entry in glob("/media/pictures/*.jpg").unwrap().filter_map(Result::ok) {
///     println!("{}", entry.path().display());
/// }
/// ```
/// Entries are yielded in alphabetical order.
pub fn glob(pattern: &str) -> Result<Entries, PatternError> {
    glob_with(pattern, &MatchOptions::new())
}

/// Return an iterator that produces all the paths with capture groups that
/// match the given pattern using the specified match options, which may be
/// absolute or relative to the current working directory.
///
/// This may return an error if the pattern is invalid.
///
/// This function accepts Unix shell style patterns as described by
/// `Pattern::new(..)`.  The options given are passed through unchanged to
/// `Pattern::matches_with(..)` with the exception that
/// `require_literal_separator` is always set to `true` regardless of the value
/// passed to this function.
///
/// Entries are yielded in alphabetical order.
pub fn glob_with(pattern: &str, options: &MatchOptions)
                 -> Result<Entries, PatternError> {
    let last_is_separator = pattern.chars().next_back().map(path::is_separator);
    let require_dir = last_is_separator == Some(true);

    let mut txt = pattern;
    if require_dir {
        // Need to strip last slash.
        // I.e. pattern `*/` means we match a directory,
        // but the real path of a directory is `something` (without slash)
        txt = &txt[..pattern.len()-1];
    };
    if txt.starts_with(".") &&
        txt[1..].chars().next().map(path::is_separator) == Some(true)
    {
        // Similarly a pattern `./*` means we match at current path
        // but the real path is `something` without dotslash
        txt = &txt[2..];
    }
    // TODO(tailhook) This may mess up error offsets
    let compiled = Pattern::new(txt)?;

    #[cfg(windows)]
    fn check_windows_verbatim(p: &Path) -> bool {
        use std::path::Prefix;
        match p.components().next() {
            Some(Component::Prefix(ref p)) => p.kind().is_verbatim(),
            _ => false,
        }
    }
    #[cfg(not(windows))]
    fn check_windows_verbatim(_: &Path) -> bool {
        false
    }

    #[cfg(windows)]
    fn to_scope(p: &Path) -> PathBuf {
        // FIXME handle volume relative paths here
        p.to_path_buf()
    }
    #[cfg(not(windows))]
    fn to_scope(p: &Path) -> PathBuf {
        p.to_path_buf()
    }

    let mut components = Path::new(pattern).components().peekable();
    loop {
        match components.peek() {
            Some(&Component::Prefix(..)) |
            Some(&Component::RootDir) => {
                components.next();
            }
            _ => break,
        }
    }
    let rest = components.map(|s| s.as_os_str()).collect::<PathBuf>();
    let normalized_pattern = Path::new(pattern).iter().collect::<PathBuf>();
    let root_len = normalized_pattern.to_str().unwrap().len() - rest.to_str().unwrap().len();
    let root = if root_len > 0 {
        Some(Path::new(&pattern[..root_len]))
    } else {
        None
    };

    if root_len > 0 && check_windows_verbatim(root.unwrap()) {
        // FIXME: How do we want to handle verbatim paths? I'm inclined to
        // return nothing, since we can't very well find all UNC shares with a
        // 1-letter server name.
        return Ok(Entries {
            dir_patterns: Vec::new(),
            whole_pattern: compiled,
            require_dir: false,
            options: options.clone(),
            todo: Vec::new(),
            scope: None,
        });
    }

    let scope = root.map(to_scope).unwrap_or_else(|| PathBuf::from("."));

    let mut dir_patterns = Vec::new();
    let components = pattern[cmp::min(root_len, pattern.len())..]
                         .split_terminator(path::is_separator);

    for component in components {
        let compiled = Pattern::new_options(component, true)?;
        dir_patterns.push(compiled);
    }

    if root_len == pattern.len() {
        dir_patterns.push(Pattern {
            original: "".to_string(),
            tokens: Vec::new(),
            is_recursive: false,
        });
    }

    let todo = Vec::new();

    Ok(Entries {
        dir_patterns: dir_patterns,
        whole_pattern: compiled,
        require_dir: require_dir,
        options: options.clone(),
        todo: todo,
        scope: Some(scope),
    })
}

/// A glob iteration error.
///
/// This is typically returned when a particular path cannot be read
/// to determine if its contents match the glob pattern. This is possible
/// if the program lacks the appropriate permissions, for example.
#[derive(Debug)]
pub struct GlobError {
    path: PathBuf,
    error: io::Error,
}

impl GlobError {
    /// The Path that the error corresponds to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The error in question.
    pub fn error(&self) -> &io::Error {
        &self.error
    }
}

impl Error for GlobError {
    fn description(&self) -> &str {
        self.error.description()
    }
    fn cause(&self) -> Option<&Error> {
        Some(&self.error)
    }
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "attempting to read `{}` resulted in an error: {}",
               self.path.display(),
               self.error)
    }
}

fn is_dir(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// An alias for a glob iteration result.
///
/// This represents either a matched path or a glob iteration error,
/// such as failing to read a particular directory's contents.
pub type GlobResult = Result<Entry, GlobError>;

impl Iterator for Entries {
    type Item = GlobResult;

    fn next(&mut self) -> Option<GlobResult> {
        // the todo buffer hasn't been initialized yet, so it's done at this
        // point rather than in glob() so that the errors are unified that is,
        // failing to fill the buffer is an iteration error construction of the
        // iterator (i.e. glob()) only fails if it fails to compile the Pattern
        if let Some(scope) = self.scope.take() {
            if self.dir_patterns.len() > 0 {
                // Shouldn't happen, but we're using -1 as a special index.
                assert!(self.dir_patterns.len() < !0 as usize);

                fill_todo(&mut self.todo,
                          &self.dir_patterns,
                          0,
                          &scope,
                          &self.options);
            }
        }

        loop {
            if self.dir_patterns.is_empty() || self.todo.is_empty() {
                return None;
            }

            let (path, mut idx) = match self.todo.pop().unwrap() {
                Ok(pair) => pair,
                Err(e) => return Some(Err(e)),
            };

            // idx -1: was already checked by fill_todo, maybe path was '.' or
            // '..' that we can't match here because of normalization.
            if idx == !0 as usize {
                if self.require_dir && !is_dir(&path) {
                    continue;
                }
                return Some(Ok(Entry::new(path)));
            }

            if self.dir_patterns[idx].is_recursive {
                let mut next = idx;

                // collapse consecutive recursive patterns
                while (next + 1) < self.dir_patterns.len() &&
                      self.dir_patterns[next + 1].is_recursive {
                    next += 1;
                }

                if is_dir(&path) {
                    // the path is a directory, so it's a match

                    // push this directory's contents
                    fill_todo(&mut self.todo,
                              &self.dir_patterns,
                              next,
                              &path,
                              &self.options);

                    if next == self.dir_patterns.len() - 1 {
                        // pattern ends in recursive pattern, so return this
                        // directory as a result
                        return Some(Ok(Entry::new(path)));
                    } else {
                        // advanced to the next pattern for this path
                        idx = next + 1;
                    }
                } else if next != self.dir_patterns.len() - 1 {
                    // advanced to the next pattern for this path
                    idx = next + 1;
                } else {
                    // not a directory and it's the last pattern, meaning no
                    // match
                    continue;
                }
            }

            // not recursive, so match normally
            if self.dir_patterns[idx].matches_with({
                match path.file_name().and_then(|s| s.to_str()) {
                    // FIXME (#9639): How do we handle non-utf8 filenames?
                    // Ignore them for now; ideally we'd still match them
                    // against a *
                    None => continue,
                    Some(x) => x
                }
            }, &self.options) {
                if idx == self.dir_patterns.len() - 1 {
                    // it is not possible for a pattern to match a directory
                    // *AND* its children so we don't need to check the
                    // children

                    if !self.require_dir || is_dir(&path) {
                        let entry = self.whole_pattern
                            .captures_path_with(&path, &self.options)
                            .expect("dir patterns consistent with whole pat");
                        return Some(Ok(entry));
                    }
                } else {
                    fill_todo(&mut self.todo, &self.dir_patterns,
                              idx + 1, &path, &self.options);
                }
            }
        }
    }
}

/// A pattern parsing error.
#[derive(Debug)]
#[allow(missing_copy_implementations)]
pub struct PatternError {
    /// The approximate character index of where the error occurred.
    pub pos: usize,

    /// A message describing the error.
    pub msg: &'static str,
}

impl Error for PatternError {
    fn description(&self) -> &str {
        self.msg
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "Pattern syntax error near position {}: {}",
               self.pos,
               self.msg)
    }
}

/// A pattern substitution error
#[derive(Debug)]
#[allow(missing_copy_implementations)]
pub enum SubstitutionError {
    /// No value supplied for capture group
    MissingGroup(usize),
    /// Wildcard char `*?[..]` is outside of the capture group
    UnexpectedWildcard,
}

impl Error for SubstitutionError {
    fn description(&self) -> &str {
        "substitution error"
    }
}

impl fmt::Display for SubstitutionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::SubstitutionError::*;
        match *self {
            MissingGroup(g) => {
                write!(f, "substitution error: missing group {}", g)
            }
            UnexpectedWildcard => {
                write!(f, "unexpected wildcard")
            }
        }
    }
}

/// A compiled Unix shell style pattern.
///
/// - `?` matches any single character.
///
/// - `*` matches any (possibly empty) sequence of characters.
///
/// - `**` matches the current directory and arbitrary subdirectories. This
///   sequence **must** form a single path component, so both `**a` and `b**`
///   are invalid and will result in an error.  A sequence of more than two
///   consecutive `*` characters is also invalid.
///
/// - `[...]` matches any character inside the brackets.  Character sequences
///   can also specify ranges of characters, as ordered by Unicode, so e.g.
///   `[0-9]` specifies any character between 0 and 9 inclusive. An unclosed
///   bracket is invalid.
///
/// - `[!...]` is the negation of `[...]`, i.e. it matches any characters
///   **not** in the brackets.
///
/// - The metacharacters `?`, `*`, `[`, `]` can be matched by using brackets
///   (e.g. `[?]`).  When a `]` occurs immediately following `[` or `[!` then it
///   is interpreted as being part of, rather then ending, the character set, so
///   `]` and NOT `]` can be matched by `[]]` and `[!]]` respectively.  The `-`
///   character can be specified inside a character sequence pattern by placing
///   it at the start or the end, e.g. `[abc-]`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct Pattern {
    original: String,
    tokens: Vec<PatternToken>,
    is_recursive: bool,
}

/// Show the original glob pattern.
impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.original.fmt(f)
    }
}

impl FromStr for Pattern {
    type Err = PatternError;

    fn from_str(s: &str) -> Result<Pattern, PatternError> {
        Pattern::new(s)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum PatternToken {
    Char(char),
    AnyChar,
    AnySequence,
    AnyRecursiveSequence,
    AnyWithin(Vec<CharSpecifier>),
    AnyExcept(Vec<CharSpecifier>),
    StartCapture(usize, bool),
    EndCapture(usize, bool),
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum CharSpecifier {
    SingleChar(char),
    CharRange(char, char),
}

#[derive(Copy, Clone, PartialEq)]
enum MatchResult {
    Match,
    SubPatternDoesntMatch,
    EntirePatternDoesntMatch,
}

#[derive(Clone, PartialEq)]
enum CaptureResult {
    Match(()),
    SubPatternDoesntMatch,
    EntirePatternDoesntMatch,
}

const ERROR_WILDCARDS: &'static str = "wildcards are either regular `*` or recursive `**`";
const ERROR_RECURSIVE_WILDCARDS: &'static str = "recursive wildcards must form a single path \
                                                 component";
const ERROR_INVALID_RANGE: &'static str = "invalid range pattern";

fn ends_with_sep(s: &[char]) -> bool {
    for &c in s.iter().rev() {
        if c == '(' || c == ')' {
            continue;
        } else if path::is_separator(c) {
            return true;
        } else {
            return false;
        }
    }
    return true;
}

impl Pattern {
    /// This function compiles Unix shell style patterns.
    ///
    /// An invalid glob pattern will yield a `PatternError`.
    pub fn new(pattern: &str) -> Result<Pattern, PatternError> {
        Pattern::new_options(pattern, false)
    }
    /// The `skip_groups` of `true` is needed to compile partial patterns in
    /// glob directory scanner
    fn new_options(pattern: &str, skip_groups: bool)
        -> Result<Pattern, PatternError>
    {
        use self::PatternToken::*;

        let chars = pattern.chars().collect::<Vec<_>>();
        let mut tokens = Vec::new();
        let mut is_recursive = false;
        let mut i = 0;
        let mut last_capture = 0;
        let mut captures_stack = Vec::new();

        while i < chars.len() {
            match chars[i] {
                '?' => {
                    tokens.push(AnyChar);
                    i += 1;
                }
                '*' => {
                    let old = i;

                    while i < chars.len() && chars[i] == '*' {
                        i += 1;
                    }

                    let count = i - old;

                    if count > 2 {
                        return Err(PatternError {
                            pos: old + 2,
                            msg: ERROR_WILDCARDS,
                        });
                    } else if count == 2 {
                        // collapse consecutive AnyRecursiveSequence to a
                        // single one
                        let tokens_len = tokens.len();
                        if !(tokens_len > 1 && tokens[tokens_len - 1] == AnyRecursiveSequence) {
                            is_recursive = true;
                            tokens.push(AnyRecursiveSequence);
                        }
                        // ** can only be an entire path component
                        // i.e. a/**/b is valid, but a**/b or a/**b is not
                        // invalid matches are treated literally
                        if ends_with_sep(&chars[..i - count]) {
                            // it ends in a '/' sans parenthesis
                            while i < chars.len() &&
                                (chars[i] == '(' || chars[i] == ')')
                            {
                                if !skip_groups {
                                    if chars[i] == '(' {
                                        captures_stack.push((last_capture, i));
                                        tokens.push(StartCapture(last_capture, true));
                                        last_capture += 1;
                                    } else if chars[i] == ')' {
                                        if let Some((c, _)) = captures_stack.pop()
                                        {
                                            tokens.push(EndCapture(c, true));
                                        } else {
                                            return Err(PatternError {
                                                pos: i,
                                                msg: "Unmatched closing paren",
                                            });
                                        }
                                    }
                                }
                                i += 1;
                            }
                            if i < chars.len() && path::is_separator(chars[i]) {
                                i += 1;
                                // or the pattern ends here
                                // this enables the existing globbing mechanism
                            } else if i == chars.len() {
                                // `**` ends in non-separator
                            } else {
                                return Err(PatternError {
                                    pos: i,
                                    msg: ERROR_RECURSIVE_WILDCARDS,
                                });
                            }
                            // `**` begins with non-separator
                        } else {
                            return Err(PatternError {
                                pos: old - 1,
                                msg: ERROR_RECURSIVE_WILDCARDS,
                            });
                        }
                    } else {
                        tokens.push(AnySequence);
                    }
                }
                '[' => {

                    if i + 4 <= chars.len() && chars[i + 1] == '!' {
                        match chars[i + 3..].iter().position(|x| *x == ']') {
                            None => (),
                            Some(j) => {
                                let chars = &chars[i + 2..i + 3 + j];
                                let cs = parse_char_specifiers(chars);
                                tokens.push(AnyExcept(cs));
                                i += j + 4;
                                continue;
                            }
                        }
                    } else if i + 3 <= chars.len() && chars[i + 1] != '!' {
                        match chars[i + 2..].iter().position(|x| *x == ']') {
                            None => (),
                            Some(j) => {
                                let cs = parse_char_specifiers(&chars[i + 1..i + 2 + j]);
                                tokens.push(AnyWithin(cs));
                                i += j + 3;
                                continue;
                            }
                        }
                    }

                    // if we get here then this is not a valid range pattern
                    return Err(PatternError {
                        pos: i,
                        msg: ERROR_INVALID_RANGE,
                    });
                }
                '(' => {
                    if !skip_groups {
                        captures_stack.push((last_capture, i));
                        tokens.push(StartCapture(last_capture, false));
                        last_capture += 1;
                    }
                    i += 1;
                }
                ')' => {
                    if !skip_groups {
                        if let Some((c, _)) = captures_stack.pop() {
                            tokens.push(EndCapture(c, false));
                        } else {
                            return Err(PatternError {
                                pos: i,
                                msg: "Unmatched closing paren",
                            });
                        }
                    }
                    i += 1;
                }
                c => {
                    tokens.push(Char(c));
                    i += 1;
                }
            }
        }

        for (_, i) in captures_stack {
            return Err(PatternError {
                pos: i,
                msg: "Unmatched opening paren",
            })
        }

        Ok(Pattern {
            tokens: tokens,
            original: pattern.to_string(),
            is_recursive: is_recursive,
        })
    }

    /// Escape metacharacters within the given string by surrounding them in
    /// brackets. The resulting string will, when compiled into a `Pattern`,
    /// match the input string and nothing else.
    pub fn escape(s: &str) -> String {
        let mut escaped = String::new();
        for c in s.chars() {
            match c {
                // note that ! does not need escaping because it is only special
                // inside brackets
                '?' | '*' | '[' | ']' => {
                    escaped.push('[');
                    escaped.push(c);
                    escaped.push(']');
                }
                c => {
                    escaped.push(c);
                }
            }
        }
        escaped
    }

    /// Return if the given `str` matches this `Pattern` using the default
    /// match options (i.e. `MatchOptions::new()`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use capturing_glob::Pattern;
    ///
    /// assert!(Pattern::new("c?t").unwrap().matches("cat"));
    /// assert!(Pattern::new("k[!e]tteh").unwrap().matches("kitteh"));
    /// assert!(Pattern::new("d*g").unwrap().matches("doog"));
    /// ```
    pub fn matches(&self, str: &str) -> bool {
        self.matches_with(str, &MatchOptions::new())
    }

    /// Return if the given `Path`, when converted to a `str`, matches this
    /// `Pattern` using the default match options (i.e. `MatchOptions::new()`).
    pub fn matches_path(&self, path: &Path) -> bool {
        // FIXME (#9639): This needs to handle non-utf8 paths
        path.to_str().map_or(false, |s| self.matches(s))
    }

    /// Return if the given `str` matches this `Pattern` using the specified
    /// match options.
    pub fn matches_with(&self, str: &str, options: &MatchOptions) -> bool {
        self.matches_from(true, str.chars(), 0, options) == Match
    }

    /// Return if the given `Path`, when converted to a `str`, matches this
    /// `Pattern` using the specified match options.
    pub fn matches_path_with(&self, path: &Path, options: &MatchOptions) -> bool {
        // FIXME (#9639): This needs to handle non-utf8 paths
        path.to_str().map_or(false, |s| self.matches_with(s, options))
    }

    /// Access the original glob pattern.
    pub fn as_str<'a>(&'a self) -> &'a str {
        &self.original
    }

    /// Return entry if filename matches pattern
    ///
    /// Then you can extract capture groups from entry
    ///
    /// # Examples
    ///
    /// ```rust
    /// use capturing_glob::Pattern;
    ///
    /// assert_eq!(Pattern::new("(*).txt").unwrap()
    ///     .captures("some.txt").unwrap()
    ///     .group(1).unwrap(),
    ///     "some");
    /// ```
    pub fn captures(&self, str: &str) -> Option<Entry> {
        self.captures_with(str, &MatchOptions::new())
    }

    /// Return an entry if filename converted to str matches pattern
    pub fn captures_path(&self, path: &Path)
        -> Option<Entry>
    {
        self.captures_path_with(path, &MatchOptions::new())
    }

    /// Return an entry if filename converted to str matches pattern
    pub fn captures_path_with(&self, path: &Path, options: &MatchOptions)
        -> Option<Entry>
    {
        // FIXME (#9639): This needs to handle non-utf8 paths
        path.to_str().map_or(None, |s| self.captures_with(s, options))
    }

    /// Return entry if filename matches pattern
    pub fn captures_with(&self, str: &str, options: &MatchOptions)
        -> Option<Entry>
    {
        use self::CaptureResult::Match;
        let mut buf = Vec::new();
        let iter = str.chars();
        match self.captures_from(true, iter, 0, str, &mut buf, options) {
            Match(()) => {
                Some(Entry::with_captures(str, buf))
            }
            _ => None,
        }
    }

    fn matches_from(&self,
                    mut follows_separator: bool,
                    mut file: std::str::Chars,
                    i: usize,
                    options: &MatchOptions)
                    -> MatchResult
    {
        use self::PatternToken::*;

        for (ti, token) in self.tokens[i..].iter().enumerate() {
            match *token {
                AnySequence | AnyRecursiveSequence => {
                    // ** must be at the start.
                    debug_assert!(match *token {
                        AnyRecursiveSequence => follows_separator,
                        _ => true,
                    });

                    // Empty match
                    match self.matches_from(follows_separator, file.clone(), i + ti + 1, options) {
                        SubPatternDoesntMatch => (), // keep trying
                        m => return m,
                    };

                    while let Some(c) = file.next() {
                        if follows_separator && options.require_literal_leading_dot && c == '.' {
                            return SubPatternDoesntMatch;
                        }
                        follows_separator = path::is_separator(c);
                        match *token {
                            AnyRecursiveSequence if !follows_separator => continue,
                            AnySequence if options.require_literal_separator &&
                                           follows_separator => return SubPatternDoesntMatch,
                            _ => (),
                        }
                        match self.matches_from(follows_separator,
                                                file.clone(),
                                                i + ti + 1,
                                                options) {
                            SubPatternDoesntMatch => (), // keep trying
                            m => return m,
                        }
                    }
                }
                StartCapture(..) | EndCapture(..) => {}
                _ => {
                    let c = match file.next() {
                        Some(c) => c,
                        None => return EntirePatternDoesntMatch,
                    };

                    let is_sep = path::is_separator(c);

                    if !match *token {
                        AnyChar | AnyWithin(..) | AnyExcept(..)
                            if (options.require_literal_separator && is_sep) ||
                            (follows_separator && options.require_literal_leading_dot &&
                             c == '.') => false,
                        AnyChar => true,
                        AnyWithin(ref specifiers) => in_char_specifiers(&specifiers, c, options),
                        AnyExcept(ref specifiers) => !in_char_specifiers(&specifiers, c, options),
                        Char(c2) => chars_eq(c, c2, options.case_sensitive),
                        AnySequence | AnyRecursiveSequence => unreachable!(),
                        StartCapture(..) | EndCapture(..) => unreachable!(),
                    } {
                        return SubPatternDoesntMatch;
                    }
                    follows_separator = is_sep;
                }
            }
        }

        // Iter is fused.
        if file.next().is_none() {
            Match
        } else {
            SubPatternDoesntMatch
        }
    }

    fn captures_from(&self,
                    mut follows_separator: bool,
                    mut file: std::str::Chars,
                    i: usize, fname: &str,
                    captures: &mut Vec<(usize, usize)>,
                    options: &MatchOptions)
        -> CaptureResult
    {
        use self::PatternToken::*;
        use self::CaptureResult::*;

        for (ti, token) in self.tokens[i..].iter().enumerate() {
            match *token {
                AnySequence | AnyRecursiveSequence => {
                    // ** must be at the start.
                    debug_assert!(match *token {
                        AnyRecursiveSequence => follows_separator,
                        _ => true,
                    });

                    // Empty match
                    match self.captures_from(follows_separator, file.clone(),
                        i + ti + 1, fname, captures, options)
                    {
                        SubPatternDoesntMatch => (), // keep trying
                        m => return m,
                    };

                    while let Some(c) = file.next() {
                        if follows_separator && options.require_literal_leading_dot && c == '.' {
                            return SubPatternDoesntMatch;
                        }
                        follows_separator = path::is_separator(c);
                        match *token {
                            AnyRecursiveSequence if !follows_separator => continue,
                            AnySequence if options.require_literal_separator &&
                                           follows_separator => return SubPatternDoesntMatch,
                            _ => (),
                        }
                        match self.captures_from(follows_separator,
                                                file.clone(),
                                                i + ti + 1,
                                                fname, captures,
                                                options) {
                            SubPatternDoesntMatch => (), // keep trying
                            m => return m,
                        }
                    }
                }
                StartCapture(n, flag) => {
                    let mut off = fname.len() - file.as_str().len();
                    if flag && fname[..off].ends_with('/') {
                        off -= 1;
                    }
                    while captures.len() < n+1 {
                        captures.push((0, 0));
                    }
                    captures[n] = (off, off);
                }
                EndCapture(n, flag) => {
                    let mut off = fname.len() - file.as_str().len();
                    if flag && fname[..off].ends_with('/') {
                        off -= 1;
                    }
                    if off < captures[n].0 {
                        // if "a/**/b" matches "a/b"
                        off = captures[n].0;
                    }
                    captures[n].1 = off;
                }
                _ => {
                    let c = match file.next() {
                        Some(pair) => pair,
                        None => return EntirePatternDoesntMatch,
                    };

                    let is_sep = path::is_separator(c);

                    if !match *token {
                        AnyChar | AnyWithin(..) | AnyExcept(..)
                            if (options.require_literal_separator && is_sep) ||
                            (follows_separator && options.require_literal_leading_dot &&
                             c == '.') => false,
                        AnyChar => true,
                        AnyWithin(ref specifiers) => in_char_specifiers(&specifiers, c, options),
                        AnyExcept(ref specifiers) => !in_char_specifiers(&specifiers, c, options),
                        Char(c2) => chars_eq(c, c2, options.case_sensitive),
                        AnySequence | AnyRecursiveSequence => unreachable!(),
                        StartCapture(..) | EndCapture(..) => unreachable!(),
                    } {
                        return SubPatternDoesntMatch;
                    }
                    follows_separator = is_sep;
                }
            }
        }

        // Iter is fused.
        if file.next().is_none() {
            Match(())
        } else {
            SubPatternDoesntMatch
        }
    }
    /// Substitute values back into patterns replacing capture groups
    ///
    /// ```rust
    /// # use std::error::Error;
    /// use capturing_glob::Pattern;
    ///
    /// # fn run() -> Result<(), Box<Error>> {
    /// assert_eq!(Pattern::new("images/(*).jpg")?.substitute(&["cat"])?,
    ///            "images/cat.jpg");
    /// # Ok(())
    /// # }
    /// # fn main() { run().unwrap() }
    /// ```
    ///
    /// Note: we check neither result so it matches pattern.
    pub fn substitute(&self, capture_groups: &[&str])
        -> Result<String, SubstitutionError>
    {
        use self::PatternToken::*;

        let mut result = String::with_capacity(self.original.len());
        let mut iter = self.tokens.iter();
        while let Some(tok) = iter.next() {
            match *tok {
                Char(c) => result.push(c),
                AnyChar | AnySequence | AnyRecursiveSequence |
                AnyWithin(..) | AnyExcept(..)
                => {
                    return Err(SubstitutionError::UnexpectedWildcard);
                }
                StartCapture(idx, _) => {
                    if let Some(val) = capture_groups.get(idx) {
                        result.push_str(val);
                    } else {
                        return Err(SubstitutionError::MissingGroup(idx));
                    }
                    for tok in iter.by_ref() {
                        match *tok {
                            EndCapture(i, _) if idx == i => break,
                            _ => {}
                        }
                    }
                }
                EndCapture(_, _) => unreachable!(),
            }
        }
        return Ok(result)
    }
}

// Fills `todo` with paths under `path` to be matched by `patterns[idx]`,
// special-casing patterns to match `.` and `..`, and avoiding `readdir()`
// calls when there are no metacharacters in the pattern.
fn fill_todo(todo: &mut Vec<Result<(PathBuf, usize), GlobError>>,
             patterns: &[Pattern],
             idx: usize,
             path: &Path,
             options: &MatchOptions) {
    // convert a pattern that's just many Char(_) to a string
    fn pattern_as_str(pattern: &Pattern) -> Option<String> {
        let mut s = String::new();
        for token in pattern.tokens.iter() {
            match *token {
                PatternToken::Char(c) => s.push(c),
                _ => return None,
            }
        }
        return Some(s);
    }

    let add = |todo: &mut Vec<_>, next_path: PathBuf| {
        if idx + 1 == patterns.len() {
            // We know it's good, so don't make the iterator match this path
            // against the pattern again. In particular, it can't match
            // . or .. globs since these never show up as path components.
            todo.push(Ok((next_path, !0 as usize)));
        } else {
            fill_todo(todo, patterns, idx + 1, &next_path, options);
        }
    };

    let pattern = &patterns[idx];
    let is_dir = is_dir(path);
    let curdir = path == Path::new(".");
    match pattern_as_str(pattern) {
        Some(s) => {
            // This pattern component doesn't have any metacharacters, so we
            // don't need to read the current directory to know where to
            // continue. So instead of passing control back to the iterator,
            // we can just check for that one entry and potentially recurse
            // right away.
            let special = "." == s || ".." == s;
            let next_path = if curdir {
                PathBuf::from(s)
            } else {
                path.join(&s)
            };
            if (special && is_dir) || (!special && fs::metadata(&next_path).is_ok()) {
                add(todo, next_path);
            }
        }
        None if is_dir => {
            let dirs = fs::read_dir(path).and_then(|d| {
                d.map(|e| {
                     e.map(|e| {
                         if curdir {
                             PathBuf::from(e.path().file_name().unwrap())
                         } else {
                             e.path()
                         }
                     })
                 })
                 .collect::<Result<Vec<_>, _>>()
            });
            match dirs {
                Ok(mut children) => {
                    children.sort_by(|p1, p2| p2.file_name().cmp(&p1.file_name()));
                    todo.extend(children.into_iter().map(|x| Ok((x, idx))));

                    // Matching the special directory entries . and .. that
                    // refer to the current and parent directory respectively
                    // requires that the pattern has a leading dot, even if the
                    // `MatchOptions` field `require_literal_leading_dot` is not
                    // set.
                    if pattern.tokens.len() > 0 && pattern.tokens[0] == PatternToken::Char('.') {
                        for &special in [".", ".."].iter() {
                            if pattern.matches_with(special, options) {
                                add(todo, path.join(special));
                            }
                        }
                    }
                }
                Err(e) => {
                    todo.push(Err(GlobError {
                        path: path.to_path_buf(),
                        error: e,
                    }));
                }
            }
        }
        None => {
            // not a directory, nothing more to find
        }
    }
}

fn parse_char_specifiers(s: &[char]) -> Vec<CharSpecifier> {
    let mut cs = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if i + 3 <= s.len() && s[i + 1] == '-' {
            cs.push(CharRange(s[i], s[i + 2]));
            i += 3;
        } else {
            cs.push(SingleChar(s[i]));
            i += 1;
        }
    }
    cs
}

fn in_char_specifiers(specifiers: &[CharSpecifier], c: char, options: &MatchOptions) -> bool {

    for &specifier in specifiers.iter() {
        match specifier {
            SingleChar(sc) => {
                if chars_eq(c, sc, options.case_sensitive) {
                    return true;
                }
            }
            CharRange(start, end) => {

                // FIXME: work with non-ascii chars properly (issue #1347)
                if !options.case_sensitive && c.is_ascii() && start.is_ascii() && end.is_ascii() {

                    let start = start.to_ascii_lowercase();
                    let end = end.to_ascii_lowercase();

                    let start_up = start.to_uppercase().next().unwrap();
                    let end_up = end.to_uppercase().next().unwrap();

                    // only allow case insensitive matching when
                    // both start and end are within a-z or A-Z
                    if start != start_up && end != end_up {
                        let c = c.to_ascii_lowercase();
                        if c >= start && c <= end {
                            return true;
                        }
                    }
                }

                if c >= start && c <= end {
                    return true;
                }
            }
        }
    }

    false
}

/// A helper function to determine if two chars are (possibly case-insensitively) equal.
fn chars_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if cfg!(windows) && path::is_separator(a) && path::is_separator(b) {
        true
    } else if !case_sensitive && a.is_ascii() && b.is_ascii() {
        // FIXME: work with non-ascii chars properly (issue #9084)
        a.to_ascii_lowercase() == b.to_ascii_lowercase()
    } else {
        a == b
    }
}


/// Configuration options to modify the behaviour of `Pattern::matches_with(..)`.
#[allow(missing_copy_implementations)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct MatchOptions {
    /// Whether or not patterns should be matched in a case-sensitive manner.
    /// This currently only considers upper/lower case relationships between
    /// ASCII characters, but in future this might be extended to work with
    /// Unicode.
    pub case_sensitive: bool,

    /// Whether or not path-component separator characters (e.g. `/` on
    /// Posix) must be matched by a literal `/`, rather than by `*` or `?` or
    /// `[...]`.
    pub require_literal_separator: bool,

    /// Whether or not paths that contain components that start with a `.`
    /// will require that `.` appears literally in the pattern; `*`, `?`, `**`,
    /// or `[...]` will not match. This is useful because such files are
    /// conventionally considered hidden on Unix systems and it might be
    /// desirable to skip them when listing files.
    pub require_literal_leading_dot: bool,
}

impl MatchOptions {
    /// Constructs a new `MatchOptions` with default field values. This is used
    /// when calling functions that do not take an explicit `MatchOptions`
    /// parameter.
    ///
    /// This function always returns this value:
    ///
    /// ```rust,ignore
    /// MatchOptions {
    ///     case_sensitive: true,
    ///     require_literal_separator: false,
    ///     require_literal_leading_dot: false
    /// }
    /// ```
    pub fn new() -> MatchOptions {
        MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        }
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;
    use super::{glob, Pattern, MatchOptions};

    #[test]
    fn test_pattern_from_str() {
        assert!("a*b".parse::<Pattern>().unwrap().matches("a_b"));
        assert!("a/**b".parse::<Pattern>().unwrap_err().pos == 4);
    }

    #[test]
    fn test_wildcard_errors() {
        assert!(Pattern::new("a/**b").unwrap_err().pos == 4);
        assert!(Pattern::new("a/bc**").unwrap_err().pos == 3);
        assert!(Pattern::new("a/*****").unwrap_err().pos == 4);
        assert!(Pattern::new("a/b**c**d").unwrap_err().pos == 2);
        assert!(Pattern::new("a**b").unwrap_err().pos == 0);
    }

    #[test]
    fn test_unclosed_bracket_errors() {
        assert!(Pattern::new("abc[def").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[!def").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[!").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[d").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[!d").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[]").unwrap_err().pos == 3);
        assert!(Pattern::new("abc[!]").unwrap_err().pos == 3);
    }

    #[test]
    fn test_glob_errors() {
        assert!(glob("a/**b").err().unwrap().pos == 4);
        assert!(glob("abc[def").err().unwrap().pos == 3);
    }

    // this test assumes that there is a /root directory and that
    // the user running this test is not root or otherwise doesn't
    // have permission to read its contents
    #[cfg(unix)]
    #[test]
    fn test_iteration_errors() {
        use std::io;
        let mut iter = glob("/root/*").unwrap();

        // GlobErrors shouldn't halt iteration
        let next = iter.next();
        assert!(next.is_some());

        let err = next.unwrap();
        assert!(err.is_err());

        let err = err.err().unwrap();
        assert!(err.path() == Path::new("/root"));
        assert!(err.error().kind() == io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn test_absolute_pattern() {
        assert!(glob("/").unwrap().next().is_some());
        assert!(glob("//").unwrap().next().is_some());

        // assume that the filesystem is not empty!
        assert!(glob("/*").unwrap().next().is_some());

        #[cfg(not(windows))]
        fn win() {}

        #[cfg(windows)]
        fn win() {
            use std::env::current_dir;
            use std::ffi::AsOsStr;

            // check windows absolute paths with host/device components
            let root_with_device = current_dir()
                                       .ok()
                                       .and_then(|p| p.prefix().map(|p| p.join("*")))
                                       .unwrap();
            // FIXME (#9639): This needs to handle non-utf8 paths
            assert!(glob(root_with_device.as_os_str().to_str().unwrap()).unwrap().next().is_some());
        }
        win()
    }

    #[test]
    fn test_wildcards() {
        assert!(Pattern::new("a*b").unwrap().matches("a_b"));
        assert!(Pattern::new("a*b*c").unwrap().matches("abc"));
        assert!(!Pattern::new("a*b*c").unwrap().matches("abcd"));
        assert!(Pattern::new("a*b*c").unwrap().matches("a_b_c"));
        assert!(Pattern::new("a*b*c").unwrap().matches("a___b___c"));
        assert!(Pattern::new("abc*abc*abc").unwrap().matches("abcabcabcabcabcabcabc"));
        assert!(!Pattern::new("abc*abc*abc").unwrap().matches("abcabcabcabcabcabcabca"));
        assert!(Pattern::new("a*a*a*a*a*a*a*a*a")
                    .unwrap()
                    .matches("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(Pattern::new("a*b[xyz]c*d").unwrap().matches("abxcdbxcddd"));
        assert!(Pattern::new("some/only-(*).txt").unwrap().matches("some/only-file1.txt"));
    }

    #[test]
    fn test_recursive_wildcards() {
        let pat = Pattern::new("some/**/needle.txt").unwrap();
        assert!(pat.matches("some/needle.txt"));
        assert!(pat.matches("some/one/needle.txt"));
        assert!(pat.matches("some/one/two/needle.txt"));
        assert!(pat.matches("some/other/needle.txt"));
        assert!(!pat.matches("some/other/notthis.txt"));

        // a single ** should be valid, for globs
        // Should accept anything
        let pat = Pattern::new("**").unwrap();
        assert!(pat.is_recursive);
        assert!(pat.matches("abcde"));
        assert!(pat.matches(""));
        assert!(pat.matches(".asdf"));
        assert!(pat.matches("/x/.asdf"));


        // collapse consecutive wildcards
        let pat = Pattern::new("some/**/**/needle.txt").unwrap();
        assert!(pat.matches("some/needle.txt"));
        assert!(pat.matches("some/one/needle.txt"));
        assert!(pat.matches("some/one/two/needle.txt"));
        assert!(pat.matches("some/other/needle.txt"));
        assert!(!pat.matches("some/other/notthis.txt"));

        // ** can begin the pattern
        let pat = Pattern::new("**/test").unwrap();
        assert!(pat.matches("one/two/test"));
        assert!(pat.matches("one/test"));
        assert!(pat.matches("test"));

        // /** can begin the pattern
        let pat = Pattern::new("/**/test").unwrap();
        assert!(pat.matches("/one/two/test"));
        assert!(pat.matches("/one/test"));
        assert!(pat.matches("/test"));
        assert!(!pat.matches("/one/notthis"));
        assert!(!pat.matches("/notthis"));

        // Only start sub-patterns on start of path segment.
        let pat = Pattern::new("**/.*").unwrap();
        assert!(pat.matches(".abc"));
        assert!(pat.matches("abc/.abc"));
        assert!(!pat.matches("ab.c"));
        assert!(!pat.matches("abc/ab.c"));
    }

    #[test]
    fn test_lots_of_files() {
        // this is a good test because it touches lots of differently named files
        glob("/*/*/*/*").unwrap().skip(10000).next();
    }

    #[test]
    fn test_range_pattern() {

        let pat = Pattern::new("a[0-9]b").unwrap();
        for i in 0..10 {
            assert!(pat.matches(&format!("a{}b", i)));
        }
        assert!(!pat.matches("a_b"));

        let pat = Pattern::new("a[!0-9]b").unwrap();
        for i in 0..10 {
            assert!(!pat.matches(&format!("a{}b", i)));
        }
        assert!(pat.matches("a_b"));

        let pats = ["[a-z123]", "[1a-z23]", "[123a-z]"];
        for &p in pats.iter() {
            let pat = Pattern::new(p).unwrap();
            for c in "abcdefghijklmnopqrstuvwxyz".chars() {
                assert!(pat.matches(&c.to_string()));
            }
            for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
                let options = MatchOptions { case_sensitive: false, ..MatchOptions::new() };
                assert!(pat.matches_with(&c.to_string(), &options));
            }
            assert!(pat.matches("1"));
            assert!(pat.matches("2"));
            assert!(pat.matches("3"));
        }

        let pats = ["[abc-]", "[-abc]", "[a-c-]"];
        for &p in pats.iter() {
            let pat = Pattern::new(p).unwrap();
            assert!(pat.matches("a"));
            assert!(pat.matches("b"));
            assert!(pat.matches("c"));
            assert!(pat.matches("-"));
            assert!(!pat.matches("d"));
        }

        let pat = Pattern::new("[2-1]").unwrap();
        assert!(!pat.matches("1"));
        assert!(!pat.matches("2"));

        assert!(Pattern::new("[-]").unwrap().matches("-"));
        assert!(!Pattern::new("[!-]").unwrap().matches("-"));
    }

    #[test]
    fn test_pattern_matches() {
        let txt_pat = Pattern::new("*hello.txt").unwrap();
        assert!(txt_pat.matches("hello.txt"));
        assert!(txt_pat.matches("gareth_says_hello.txt"));
        assert!(txt_pat.matches("some/path/to/hello.txt"));
        assert!(txt_pat.matches("some\\path\\to\\hello.txt"));
        assert!(txt_pat.matches("/an/absolute/path/to/hello.txt"));
        assert!(!txt_pat.matches("hello.txt-and-then-some"));
        assert!(!txt_pat.matches("goodbye.txt"));

        let dir_pat = Pattern::new("*some/path/to/hello.txt").unwrap();
        assert!(dir_pat.matches("some/path/to/hello.txt"));
        assert!(dir_pat.matches("a/bigger/some/path/to/hello.txt"));
        assert!(!dir_pat.matches("some/path/to/hello.txt-and-then-some"));
        assert!(!dir_pat.matches("some/other/path/to/hello.txt"));
    }

    #[test]
    fn test_pattern_escape() {
        let s = "_[_]_?_*_!_";
        assert_eq!(Pattern::escape(s), "_[[]_[]]_[?]_[*]_!_".to_string());
        assert!(Pattern::new(&Pattern::escape(s)).unwrap().matches(s));
    }

    #[test]
    fn test_pattern_matches_case_insensitive() {

        let pat = Pattern::new("aBcDeFg").unwrap();
        let options = MatchOptions {
            case_sensitive: false,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };

        assert!(pat.matches_with("aBcDeFg", &options));
        assert!(pat.matches_with("abcdefg", &options));
        assert!(pat.matches_with("ABCDEFG", &options));
        assert!(pat.matches_with("AbCdEfG", &options));
    }

    #[test]
    fn test_pattern_matches_case_insensitive_range() {

        let pat_within = Pattern::new("[a]").unwrap();
        let pat_except = Pattern::new("[!a]").unwrap();

        let options_case_insensitive = MatchOptions {
            case_sensitive: false,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        let options_case_sensitive = MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };

        assert!(pat_within.matches_with("a", &options_case_insensitive));
        assert!(pat_within.matches_with("A", &options_case_insensitive));
        assert!(!pat_within.matches_with("A", &options_case_sensitive));

        assert!(!pat_except.matches_with("a", &options_case_insensitive));
        assert!(!pat_except.matches_with("A", &options_case_insensitive));
        assert!(pat_except.matches_with("A", &options_case_sensitive));
    }

    #[test]
    fn test_pattern_matches_require_literal_separator() {

        let options_require_literal = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let options_not_require_literal = MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };

        assert!(Pattern::new("abc/def").unwrap().matches_with("abc/def", &options_require_literal));
        assert!(!Pattern::new("abc?def")
                     .unwrap()
                     .matches_with("abc/def", &options_require_literal));
        assert!(!Pattern::new("abc*def")
                     .unwrap()
                     .matches_with("abc/def", &options_require_literal));
        assert!(!Pattern::new("abc[/]def")
                     .unwrap()
                     .matches_with("abc/def", &options_require_literal));

        assert!(Pattern::new("abc/def")
                    .unwrap()
                    .matches_with("abc/def", &options_not_require_literal));
        assert!(Pattern::new("abc?def")
                    .unwrap()
                    .matches_with("abc/def", &options_not_require_literal));
        assert!(Pattern::new("abc*def")
                    .unwrap()
                    .matches_with("abc/def", &options_not_require_literal));
        assert!(Pattern::new("abc[/]def")
                    .unwrap()
                    .matches_with("abc/def", &options_not_require_literal));
    }

    #[test]
    fn test_pattern_matches_require_literal_leading_dot() {

        let options_require_literal_leading_dot = MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: true,
        };
        let options_not_require_literal_leading_dot = MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };

        let f = |options| Pattern::new("*.txt").unwrap().matches_with(".hello.txt", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(!f(&options_require_literal_leading_dot));

        let f = |options| Pattern::new(".*.*").unwrap().matches_with(".hello.txt", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(f(&options_require_literal_leading_dot));

        let f = |options| Pattern::new("aaa/bbb/*").unwrap().matches_with("aaa/bbb/.ccc", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(!f(&options_require_literal_leading_dot));

        let f = |options| {
            Pattern::new("aaa/bbb/*").unwrap().matches_with("aaa/bbb/c.c.c.", options)
        };
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(f(&options_require_literal_leading_dot));

        let f = |options| Pattern::new("aaa/bbb/.*").unwrap().matches_with("aaa/bbb/.ccc", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(f(&options_require_literal_leading_dot));

        let f = |options| Pattern::new("aaa/?bbb").unwrap().matches_with("aaa/.bbb", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(!f(&options_require_literal_leading_dot));

        let f = |options| Pattern::new("aaa/[.]bbb").unwrap().matches_with("aaa/.bbb", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(!f(&options_require_literal_leading_dot));

        let f = |options| Pattern::new("**/*").unwrap().matches_with(".bbb", options);
        assert!(f(&options_not_require_literal_leading_dot));
        assert!(!f(&options_require_literal_leading_dot));
    }

    #[test]
    fn test_matches_path() {
        // on windows, (Path::new("a/b").as_str().unwrap() == "a\\b"), so this
        // tests that / and \ are considered equivalent on windows
        assert!(Pattern::new("a/b").unwrap().matches_path(&Path::new("a/b")));
    }

    #[test]
    fn test_path_join() {
        let pattern = Path::new("one").join(&Path::new("**/*.rs"));
        assert!(Pattern::new(pattern.to_str().unwrap()).is_ok());
    }

    #[test]
    fn test_capture_two_stars() {
        let pat = Pattern::new("some/(**)/needle.txt").unwrap();
        assert_eq!(pat.captures("some/one/two/needle.txt").unwrap()
            .group(1).unwrap(), "one/two");
        assert_eq!(pat.captures("some/other/needle.txt").unwrap()
            .group(1).unwrap(), "other");
        assert!(pat.captures("some/other/not_this.txt").is_none());
        assert_eq!(pat.captures("some/needle.txt").unwrap().group(1).unwrap(), "");
        assert_eq!(pat.captures("some/one/needle.txt").unwrap()
            .group(1).unwrap(), "one");
    }

    #[test]
    fn test_capture_star() {
        let opt = MatchOptions {
            require_literal_separator: true,
            .. MatchOptions::new()
        };
        let pat = Pattern::new("some/(*)/needle.txt").unwrap();
        assert!(pat.captures("some/needle.txt").is_none());
        assert_eq!(pat.captures("some/one/needle.txt").unwrap()
            .group(1).unwrap(), "one");
        assert!(pat.captures_with("some/one/two/needle.txt", &opt).is_none());
        assert_eq!(pat.captures("some/other/needle.txt").unwrap()
            .group(1).unwrap(), "other");
        assert!(pat.captures("some/other/not_this.txt").is_none());
    }

    #[test]
    fn test_capture_name_start() {
        let opt = MatchOptions {
            require_literal_separator: true,
            .. MatchOptions::new()
        };
        let pat = Pattern::new("some/only-(*).txt").unwrap();
        assert!(pat.captures("some/needle.txt").is_none());
        assert!(pat.captures("some/one/only-x.txt").is_none());
        assert_eq!(pat.captures("some/only-file1.txt").unwrap()
            .group(1).unwrap(), "file1");
        assert_eq!(pat.captures("some/only-file2.txt").unwrap()
            .group(1).unwrap(), "file2");
        assert!(pat.captures_with("some/only-dir1/some.txt", &opt).is_none());
    }

    #[test]
    fn test_capture_end() {
        let pat = Pattern::new("some/only-(*)").unwrap();
        assert!(pat.captures("some/needle.txt").is_none());
        assert_eq!(pat.captures("some/only-file1.txt").unwrap()
            .group(1).unwrap(), "file1.txt");
        assert_eq!(pat.captures("some/only-").unwrap()
            .group(1).unwrap(), "");
    }

    #[test]
    fn test_capture_char() {
        let pat = Pattern::new("some/file(?).txt").unwrap();
        assert_eq!(pat.captures("some/file1.txt").unwrap()
            .group(1).unwrap(), "1");
        assert_eq!(pat.captures("some/file2.txt").unwrap()
            .group(1).unwrap(), "2");
        assert!(pat.captures("some/file12.txt").is_none());
        assert!(pat.captures("some/file.txt").is_none());
    }

    #[test]
    fn test_paren_two_stars() {
        let pat = Pattern::new("some/(**)/needle.txt").unwrap();
        assert!(pat.matches("some/one/needle.txt"));
        assert!(pat.matches("some/one/two/needle.txt"));
        assert!(pat.matches("some/other/needle.txt"));
        assert!(!pat.matches("some/other/not_this.txt"));
        assert!(pat.matches("some/needle.txt"));
    }

    #[test]
    fn test_paren_star() {
        let opt = MatchOptions {
            require_literal_separator: true,
            .. MatchOptions::new()
        };
        let pat = Pattern::new("some/(*)/needle.txt").unwrap();
        assert!(!pat.matches("some/needle.txt"));
        assert!(pat.matches("some/one/needle.txt"));
        assert!(!pat.matches_with("some/one/two/needle.txt", &opt));
        assert!(pat.matches("some/other/needle.txt"));
        assert!(!pat.matches("some/other/not_this.txt"));
    }

    #[test]
    fn test_paren_name_start() {
        let opt = MatchOptions {
            require_literal_separator: true,
            .. MatchOptions::new()
        };
        let pat = Pattern::new("some/only-(*).txt").unwrap();
        assert!(!pat.matches("some/needle.txt"));
        assert!(!pat.matches("some/one/only-x.txt"));
        assert!(pat.matches("some/only-file1.txt"));
        assert!(pat.matches("some/only-file2.txt"));
        assert!(!pat.matches_with("some/only-dir1/some.txt", &opt));
    }

    #[test]
    fn test_paren_end() {
        let pat = Pattern::new("some/only-(*)").unwrap();
        assert!(!pat.matches("some/needle.txt"));
        assert!(pat.matches("some/only-file1.txt"));
        assert!(pat.matches("some/only-"));
    }

    #[test]
    fn test_paren_char() {
        let pat = Pattern::new("some/file(?).txt").unwrap();
        assert!(pat.matches("some/file1.txt"));
        assert!(pat.matches("some/file2.txt"));
        assert!(!pat.matches("some/file12.txt"));
        assert!(!pat.matches("some/file.txt"));
    }
}
