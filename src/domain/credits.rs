#[derive(Debug, PartialEq, Eq)]
pub struct Credits {
    pub artist: String,
    pub title: String,
}

pub fn prefer_latin_alias(value: &str) -> String {
    let has_latin = value
        .chars()
        .any(|character| character.is_ascii_alphabetic());
    let has_arabic = value.chars().any(is_arabic_script);
    if !has_latin || !has_arabic {
        return value.trim().to_owned();
    }
    let without_arabic = value
        .chars()
        .map(|character| {
            if is_arabic_script(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    without_arabic
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '&' | ',' | ';' | '-' | '–' | '—')
        })
        .trim()
        .to_owned()
}

fn is_arabic_script(character: char) -> bool {
    matches!(
        character,
        '\u{0600}'..='\u{06ff}'
            | '\u{0750}'..='\u{077f}'
            | '\u{08a0}'..='\u{08ff}'
            | '\u{fb50}'..='\u{fdff}'
            | '\u{fe70}'..='\u{feff}'
    )
}

pub fn normalize_featured(artist: &str, title: &str) -> Credits {
    let Some((start, end, featured)) = feature_clause(title) else {
        return Credits {
            artist: artist.trim().to_owned(),
            title: title.trim().to_owned(),
        };
    };
    let mut normalized_title = title[..start].trim_end().to_owned();
    normalized_title.push_str(" (feat. ");
    normalized_title.push_str(featured.trim());
    normalized_title.push(')');
    let suffix = title[end..].trim();
    if !is_duplicate_feature_suffix(featured, suffix) && !suffix.is_empty() {
        normalized_title.push(' ');
        normalized_title.push_str(suffix);
    }

    let featured_names = split_names(featured);
    let artist_parts = split_names(artist);
    let has_duplicated_feature = artist_parts.iter().any(|part| {
        featured_names
            .iter()
            .any(|featured| same_name(part, featured))
    });
    let normalized_artist = if has_duplicated_feature {
        artist_parts
            .into_iter()
            .filter(|part| {
                !featured_names
                    .iter()
                    .any(|featured| same_name(part, featured))
            })
            .collect::<Vec<_>>()
            .join(" & ")
    } else {
        artist.trim().to_owned()
    };
    Credits {
        artist: if normalized_artist.is_empty() {
            artist.trim().to_owned()
        } else {
            normalized_artist
        },
        title: normalized_title,
    }
}

fn is_duplicate_feature_suffix(featured: &str, suffix: &str) -> bool {
    let lower = suffix.to_ascii_lowercase();
    let Some(offset) = ["feat. ", "feat ", "ft. ", "ft ", "featuring "]
        .into_iter()
        .find_map(|marker| lower.starts_with(marker).then_some(marker.len()))
    else {
        return false;
    };
    let expected = split_names(featured);
    let trailing = split_names(&suffix[offset..]);
    expected.len() == trailing.len()
        && expected
            .iter()
            .all(|name| trailing.iter().any(|other| same_name(name, other)))
}

fn feature_clause(title: &str) -> Option<(usize, usize, &str)> {
    let lower = title.to_ascii_lowercase();
    for marker in ["(feat. ", "(feat ", "(ft. ", "(ft ", "(featuring "] {
        if let Some(start) = lower.find(marker)
            && let Some(relative_close) = title[start + marker.len()..].find(')')
        {
            let close = relative_close + start + marker.len();
            return Some((start, close + 1, &title[start + marker.len()..close]));
        }
    }
    None
}

fn split_names(value: &str) -> Vec<String> {
    value
        .replace(" feat. ", ";")
        .replace(" ft. ", ";")
        .replace(" featuring ", ";")
        .split([';', '&', ',', '؛', '،'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn same_name(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_featured_artist_from_semicolon_credit() {
        assert_eq!(
            normalize_featured(
                "Ali Azimi;Golshifteh Farahani",
                "Marze Por Gohar (feat. Golshifteh Farahani)"
            ),
            Credits {
                artist: "Ali Azimi".into(),
                title: "Marze Por Gohar (feat. Golshifteh Farahani)".into()
            }
        );
    }

    #[test]
    fn keeps_feature_in_title_instead_of_artist_credit() {
        assert_eq!(
            normalize_featured("Arta", "Mi Amor (ft. Saaren)"),
            Credits {
                artist: "Arta".into(),
                title: "Mi Amor (feat. Saaren)".into()
            }
        );
    }

    #[test]
    fn bilingual_alias_prefers_the_latin_name() {
        assert_eq!(prefer_latin_alias("Hayedeh هايده"), "Hayedeh");
        assert_eq!(prefer_latin_alias("هایده Hayedeh"), "Hayedeh");
        assert_eq!(prefer_latin_alias("Ali علی & Reza رضا"), "Ali & Reza");
    }

    #[test]
    fn persian_only_artist_is_preserved() {
        assert_eq!(prefer_latin_alias("هایده"), "هایده");
    }

    #[test]
    fn removes_a_duplicate_trailing_feature_credit() {
        assert_eq!(
            normalize_featured(
                "Arta",
                "Hanooz Yadame (feat. Koorosh, Sami Low & Raha) feat. Koorosh,Sami Low,Raha"
            )
            .title,
            "Hanooz Yadame (feat. Koorosh, Sami Low & Raha)"
        );
    }
}
