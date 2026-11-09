//! The source texts a compiled program's `kgen.source_loc` sites are
//! placed against.
//!
//! Each site reports its line, column, and file
//! ([`crate::mir::resolve_source_locations`]). Only the driver holds the
//! entry's in-memory text; every other file is read from disk when a site
//! first names it.

use crate::lexer::Lexer;
use crate::mir::{MirFunction, SourcePoint, resolve_source_locations};
use crate::token::{SourceSpan, Token, line_column};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The texts sites are placed against, by the source label a span carries.
#[derive(Debug, Default)]
pub struct SourceTexts {
    /// The texts the compilation was handed rather than read: an entry
    /// compiled from memory.
    given: HashMap<String, Arc<str>>,
    /// Each file read from disk, or `None` when it could not be read.
    read: Mutex<HashMap<String, Option<Arc<str>>>>,
}

impl SourceTexts {
    /// The texts of a compilation whose entry, labelled `label`, is `text`.
    pub fn with_entry(label: String, text: &str) -> Self {
        Self {
            given: HashMap::from([(label, Arc::from(text))]),
            read: Mutex::default(),
        }
    }

    /// Resolve every `kgen.source_loc` of `functions` against these texts
    /// ([`Self::locate`]): the indexes of the bodies that held one.
    pub fn resolve(&self, functions: &mut [(String, MirFunction)]) -> Vec<usize> {
        functions
            .iter_mut()
            .enumerate()
            .filter_map(|(index, (_, function))| {
                resolve_source_locations(function, &|site| self.locate(site)).then_some(index)
            })
            .collect()
    }

    /// The point the site `site` reports, as the pin
    /// places a call: at its argument list's `(`, a subscript's `[`, and
    /// otherwise at the expression's start. The file is the absolute path
    /// of the site's label. `None` when no text holds the site.
    pub fn locate(&self, site: &SourceSpan) -> Option<SourcePoint> {
        let label = site.source.as_deref()?;
        let text = self.text(label)?;
        let fragment = text.get(site.span.0..site.span.1)?;
        let point = site.span.0 + call_point(fragment);
        let (line, column) = line_column(&text, point)?;
        Some(SourcePoint {
            line: i64::try_from(line).ok()?,
            column: i64::try_from(column).ok()?,
            file: absolute_file(label),
        })
    }

    fn text(&self, label: &str) -> Option<Arc<str>> {
        if let Some(text) = self.given.get(label) {
            return Some(text.clone());
        }
        let mut read = self
            .read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read.entry(label.to_string())
            .or_insert_with(|| std::fs::read_to_string(label).ok().map(Arc::from))
            .clone()
    }
}

/// The absolute path `label` names, its `.` and `..` steps resolved
/// lexically, as the pin spells a file it reports.
fn absolute_file(label: &str) -> String {
    let Ok(path) = std::path::absolute(label) else {
        return label.to_string();
    };
    let mut normal = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normal.pop();
            }
            component => normal.push(component),
        }
    }
    normal.display().to_string()
}

/// The offset in an expression's text `fragment` the pin reports a call
/// of it at: the opener of the bracket list that closes it, when that is
/// not its first token, so a call's `(` and a subscript's `[`; otherwise
/// its start.
fn call_point(fragment: &str) -> usize {
    let brackets: Vec<(Token, usize)> = Lexer::new(fragment)
        .map_while(Result::ok)
        .filter(|(token, _)| {
            !matches!(
                token,
                Token::Newline | Token::Indent | Token::Dedent | Token::Eof
            )
        })
        .map(|(token, span)| (token, span.0))
        .collect();
    let Some((last, _)) = brackets.last() else {
        return 0;
    };
    let opener = match last {
        Token::RParen => Token::LParen,
        Token::RBracket => Token::LBracket,
        _ => return 0,
    };
    let mut depth = 0usize;
    for (index, (token, start)) in brackets.iter().enumerate().rev() {
        if *token == brackets[brackets.len() - 1].0 {
            depth += 1;
        } else if *token == opener {
            depth -= 1;
            if depth == 0 {
                return if index == 0 { 0 } else { *start };
            }
        }
    }
    0
}
