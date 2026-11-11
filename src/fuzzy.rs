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

#[derive(Default)]
pub(crate) struct ScoreScratch {
    normalized: String,
    characters: Vec<char>,
    bonuses: Vec<i64>,
    previous: Vec<i64>,
    current: Vec<i64>,
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
        if self.case_sensitive
            || (text.is_ascii() && !text.bytes().any(|byte| byte.is_ascii_uppercase()))
        {
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

    /// Original Unicode character indices matched by the query, sorted and
    /// deduplicated. Exclusions filter candidates but never highlight text.
    /// Fuzzy terms use the same best-scoring alignment as [`Self::score`].
    pub fn positions(&self, text: &str) -> Option<Vec<usize>> {
        if self.terms.is_empty() {
            return Some(Vec::new());
        }
        let normalized = self.normalize(text);
        if !self
            .terms
            .iter()
            .all(|term| term.matches(&normalized) != term.excluded)
        {
            return None;
        }
        let (characters, bonuses, original) = self.characters(text, &normalized, true);
        let mut result = Vec::new();
        for term in self.terms.iter().filter(|term| !term.excluded) {
            let positions = match term.kind {
                Kind::Fuzzy => fuzzy_positions(&characters, &bonuses, &term.chars),
                _ => {
                    let start = if matches!(term.kind, Kind::Suffix) {
                        normalized.len() - term.text.len()
                    } else {
                        normalized.find(&term.text).unwrap_or(0)
                    };
                    let start = normalized[..start].chars().count();
                    (start..start + term.chars.len()).collect()
                }
            };
            result.extend(
                positions
                    .into_iter()
                    .filter_map(|index| original.get(index).copied()),
            );
        }
        result.sort_unstable();
        result.dedup();
        Some(result)
    }

    fn characters(
        &self,
        text: &str,
        normalized: &str,
        map_original: bool,
    ) -> (Vec<char>, Vec<i64>, Vec<usize>) {
        let mut bonuses = Vec::new();
        let mut original = Vec::new();
        self.fill_bonuses(text, &mut bonuses, map_original.then_some(&mut original));
        (normalized.chars().collect(), bonuses, original)
    }

    fn fill_bonuses(
        &self,
        text: &str,
        bonuses: &mut Vec<i64>,
        mut original: Option<&mut Vec<usize>>,
    ) {
        bonuses.clear();
        if let Some(original) = original.as_mut() {
            original.clear();
        }
        let mut previous: Option<char> = None;
        for (source, character) in text.chars().enumerate() {
            let bonus = match previous {
                None => 12,
                Some(before) if !before.is_alphanumeric() => 12,
                Some(before) if before.is_lowercase() && character.is_uppercase() => 8,
                _ => 0,
            };
            let count = if self.case_sensitive {
                1
            } else {
                character.to_lowercase().count()
            };
            for index in 0..count {
                bonuses.push(if index == 0 { bonus } else { 0 });
                if let Some(original) = original.as_mut() {
                    original.push(source);
                }
            }
            previous = Some(character);
        }
    }

    /// Higher scores prefer consecutive matches, word/path boundaries and
    /// earlier matches. `None` means at least one term does not match.
    pub fn score(&self, text: &str) -> Option<i64> {
        self.score_with_scratch(text, &mut ScoreScratch::default())
    }

    pub(crate) fn score_with_scratch(&self, text: &str, scratch: &mut ScoreScratch) -> Option<i64> {
        if self.terms.is_empty() {
            return Some(0);
        }
        let ScoreScratch {
            normalized,
            characters,
            bonuses,
            previous,
            current,
        } = scratch;
        let normalized: &str = if self.case_sensitive {
            text
        } else {
            if text.is_ascii() {
                normalized.clear();
                normalized.push_str(text);
                normalized.make_ascii_lowercase();
            } else {
                *normalized = text.to_lowercase();
            }
            normalized
        };
        if !self
            .terms
            .iter()
            .all(|term| term.matches(normalized) != term.excluded)
        {
            return None;
        }
        characters.clear();
        characters.extend(normalized.chars());
        self.fill_bonuses(text, bonuses, None);
        let mut total = 0;
        for term in self.terms.iter().filter(|term| !term.excluded) {
            total += match term.kind {
                Kind::Fuzzy => fuzzy_score(characters, bonuses, &term.chars, previous, current),
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
            if normalized == term.text {
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

fn fuzzy_score(
    text: &[char],
    bonuses: &[i64],
    needle: &[char],
    previous: &mut Vec<i64>,
    current: &mut Vec<i64>,
) -> i64 {
    const MISS: i64 = i64::MIN / 4;
    previous.resize(text.len(), MISS);
    previous.fill(MISS);
    current.resize(text.len(), MISS);
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
        std::mem::swap(previous, current);
    }
    previous.iter().copied().max().unwrap_or(0)
}

fn fuzzy_positions(text: &[char], bonuses: &[i64], needle: &[char]) -> Vec<usize> {
    const MISS: i64 = i64::MIN / 4;
    if needle.is_empty() {
        return Vec::new();
    }
    // Backtracking is only used for visible labels. Keep unusual long subjects
    // or pasted queries from allocating an unbounded matrix in the renderer.
    if needle.len().saturating_mul(text.len()) > 1_000_000 {
        let mut next = 0;
        return needle
            .iter()
            .filter_map(|wanted| {
                let found = text[next..].iter().position(|actual| actual == wanted)? + next;
                next = found + 1;
                Some(found)
            })
            .collect();
    }
    let mut previous = vec![MISS; text.len()];
    let mut current = vec![MISS; text.len()];
    let mut trace = vec![vec![usize::MAX; text.len()]; needle.len()];
    for (matched, wanted) in needle.iter().enumerate() {
        current.fill(MISS);
        let mut best_gap = MISS;
        let mut gap_index = usize::MAX;
        for (index, actual) in text.iter().enumerate() {
            if index > 0 {
                let candidate = previous[index - 1] + index as i64 - 1;
                if candidate > best_gap {
                    best_gap = candidate;
                    gap_index = index - 1;
                }
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
                trace[matched][index] = if adjacent >= separated {
                    index.saturating_sub(1)
                } else {
                    gap_index
                };
                adjacent.max(separated) + bonuses[index]
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let Some(mut index) = previous
        .iter()
        .enumerate()
        .max_by(|(a_index, a), (b_index, b)| a.cmp(b).then_with(|| b_index.cmp(a_index)))
        .map(|(index, _)| index)
    else {
        return Vec::new();
    };
    let mut positions = Vec::with_capacity(needle.len());
    for matched in (0..needle.len()).rev() {
        positions.push(index);
        if matched > 0 {
            index = trace[matched][index];
        }
    }
    positions.reverse();
    positions
}
