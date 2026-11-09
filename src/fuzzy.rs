//! Small, dependency-free fuzzy queries for interactive lists.
//!
//! Space-separated terms must all match. Lowercase queries ignore case; an
//! uppercase character makes the query case-sensitive. A leading apostrophe
//! means substring matching, `!term` excludes a substring, and `^` / `$` anchor
//! a term. This deliberately supports a useful subset of fzf's query syntax;
//! it is not a wrapper around the fzf executable.

use std::borrow::Cow;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Fuzzy,
    Contains,
    Prefix,
    Suffix,
    Exact,
}

#[derive(Clone, Debug)]
struct Term {
    text: String,
    chars: Vec<char>,
    kind: Kind,
    excluded: bool,
}

impl Term {
    fn matches(&self, text: &str) -> bool {
        match self.kind {
            Kind::Fuzzy => {
                let mut actual = text.chars();
                self.chars
                    .iter()
                    .all(|wanted| actual.by_ref().any(|character| character == *wanted))
            }
            Kind::Contains => text.contains(&self.text),
            Kind::Prefix => text.starts_with(&self.text),
            Kind::Suffix => text.ends_with(&self.text),
            Kind::Exact => text == self.text,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Query {
    terms: Vec<Term>,
    case_sensitive: bool,
}

impl Query {
    /// Parse once and reuse when filtering or ranking a large list.
    pub fn new(query: &str) -> Self {
        let case_sensitive = query.chars().any(char::is_uppercase);
        let terms = query
            .split_whitespace()
            .map(|raw| {
                let mut text = raw;
                let excluded = text.starts_with('!') && text.len() > 1;
                if excluded {
                    text = &text[1..];
                }
                let literal = text.starts_with('\'') && text.len() > 1;
                if literal {
                    text = &text[1..];
                }
                let prefix = !literal && text.starts_with('^') && text.len() > 1;
                if prefix {
                    text = &text[1..];
                }
                let suffix = !literal && text.ends_with('$') && text.len() > 1;
                if suffix {
                    text = &text[..text.len() - 1];
                }
                let kind = match (prefix, suffix) {
                    (true, true) => Kind::Exact,
                    (true, false) => Kind::Prefix,
                    (false, true) => Kind::Suffix,
                    (false, false) if excluded || literal => Kind::Contains,
                    _ => Kind::Fuzzy,
                };
                let text = if case_sensitive {
                    text.to_owned()
                } else {
                    text.to_lowercase()
                };
                Term {
                    chars: text.chars().collect(),
                    text,
                    kind,
                    excluded,
                }
            })
            .collect();
        Self {
            terms,
            case_sensitive,
        }
    }

    fn normalize<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.case_sensitive {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(text.to_lowercase())
        }
    }

    /// Filtering does not calculate a ranking: it only scans each term once.
    pub fn is_match(&self, text: &str) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let text = self.normalize(text);
        self.terms
            .iter()
            .all(|term| term.matches(&text) != term.excluded)
    }

    /// Higher scores prefer consecutive matches, word/path boundaries and
    /// earlier matches. `None` means at least one term does not match.
    pub fn score(&self, text: &str) -> Option<i64> {
        if self.terms.is_empty() {
            return Some(0);
        }
        let normalized = self.normalize(text);
        if !self
            .terms
            .iter()
            .all(|term| term.matches(&normalized) != term.excluded)
        {
            return None;
        }
        let mut characters = Vec::new();
        let mut bonuses = Vec::new();
        let mut previous: Option<char> = None;
        for character in text.chars() {
            let bonus = match previous {
                None => 12,
                Some(before) if !before.is_alphanumeric() => 12,
                Some(before) if before.is_lowercase() && character.is_uppercase() => 8,
                _ => 0,
            };
            if self.case_sensitive {
                characters.push(character);
                bonuses.push(bonus);
            } else {
                for (index, lower) in character.to_lowercase().enumerate() {
                    characters.push(lower);
                    bonuses.push(if index == 0 { bonus } else { 0 });
                }
            }
            previous = Some(character);
        }
        let mut total = 0;
        for term in self.terms.iter().filter(|term| !term.excluded) {
            total += match term.kind {
                Kind::Fuzzy => fuzzy_score(&characters, &bonuses, &term.chars),
                _ => {
                    let byte_position = if matches!(term.kind, Kind::Suffix) {
                        normalized.len() - term.text.len()
                    } else {
                        normalized.find(&term.text).unwrap_or(0)
                    };
                    let position = normalized[..byte_position].chars().count();
                    24 * term.chars.len() as i64 + bonuses.get(position).copied().unwrap_or(0)
                        - (position as i64).min(24)
                }
            };
            if normalized.as_ref() == term.text {
                total += 40;
            }
        }
        Some(total - (characters.len() as i64).min(64) / 4)
    }
}

/// Convenience form; use [`Query`] to avoid parsing once per candidate.
pub fn score(text: &str, query: &str) -> Option<i64> {
    Query::new(query).score(text)
}

fn fuzzy_score(text: &[char], bonuses: &[i64], needle: &[char]) -> i64 {
    const MISS: i64 = i64::MIN / 4;
    let mut previous = vec![MISS; text.len()];
    let mut current = vec![MISS; text.len()];
    for (matched, wanted) in needle.iter().enumerate() {
        current.fill(MISS);
        let mut best_gap = MISS;
        for (index, actual) in text.iter().enumerate() {
            if index > 0 {
                best_gap = best_gap.max(previous[index - 1] + index as i64 - 1);
            }
            if actual != wanted {
                continue;
            }
            current[index] = if matched == 0 {
                16 + bonuses[index] - (index as i64).min(24)
            } else {
                let adjacent = if index > 0 {
                    previous[index - 1] + 24
                } else {
                    MISS
                };
                let separated = best_gap - index as i64 + 13;
                adjacent.max(separated) + bonuses[index]
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.into_iter().max().unwrap_or(0)
}
