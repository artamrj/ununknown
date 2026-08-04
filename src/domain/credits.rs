use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtistCredit {
    pub name: String,
    #[serde(default)]
    pub join_phrase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musicbrainz_id: Option<String>,
}

impl ArtistCredit {
    pub fn new(name: impl Into<String>, join_phrase: impl Into<String>) -> Self {
        Self {
            name: clean_text(&name.into()),
            join_phrase: join_phrase.into(),
            musicbrainz_id: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Credits {
    pub artist: String,
    pub title: String,
    pub artists: Vec<ArtistCredit>,
}

pub fn clean_text(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn identity_key(value: &str) -> String {
    clean_text(value)
        .chars()
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn prefer_latin_alias(value: &str) -> String {
    let value = clean_text(value);
    let has_latin = value
        .chars()
        .any(|character| character.is_ascii_alphabetic());
    let has_arabic = value.chars().any(is_arabic_script);
    if !has_latin || !has_arabic {
        return value;
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

/// Normalize a display credit while retaining the individual artist identities.
/// Only separators recognized by Navidrome are parsed. In particular, `&`,
/// `and`, `x`, commas, and bare slashes are deliberately left inside a name.
pub fn normalize_featured(artist: &str, title: &str) -> Credits {
    let artist = clean_text(artist);
    let (title, featured) = remove_feature_clause(title);
    let mut artists = parse_supported_credit(&artist);

    if let Some(featured) = featured {
        let featured = parse_supported_credit(&featured);
        for credit in featured {
            if artists
                .iter()
                .any(|existing| same_name(&existing.name, &credit.name))
            {
                continue;
            }
            if let Some(last) = artists.last_mut() {
                last.join_phrase = " feat. ".into();
            }
            artists.push(ArtistCredit::new(credit.name, ""));
        }
    }
    if artists.is_empty() && !artist.is_empty() {
        artists.push(ArtistCredit::new(&artist, ""));
    }
    let artist = display_artist(&artists);
    Credits {
        artist,
        title: clean_text(&title),
        artists,
    }
}

pub fn normalize_structured(display: &str, title: &str, mut artists: Vec<ArtistCredit>) -> Credits {
    for credit in &mut artists {
        credit.name = prefer_latin_alias(&credit.name);
    }
    artists.retain(|credit| !credit.name.is_empty());
    apply_display_joins(display, &mut artists);
    for credit in &mut artists {
        credit.join_phrase = normalize_join_phrase(&credit.join_phrase);
    }
    dedupe_credits(&mut artists);
    if artists.len() > 1 && artists.iter().all(|credit| credit.join_phrase.is_empty()) {
        let last = artists.len() - 1;
        for credit in &mut artists[..last] {
            credit.join_phrase = " / ".into();
        }
    }

    let (title, featured) = remove_feature_clause(title);
    if let Some(featured) = featured {
        for featured in parse_supported_credit(&featured) {
            if artists
                .iter()
                .any(|existing| same_name(&existing.name, &featured.name))
            {
                continue;
            }
            if let Some(last) = artists.last_mut() {
                last.join_phrase = " feat. ".into();
            }
            artists.push(ArtistCredit::new(featured.name, ""));
        }
    }
    if artists.is_empty() {
        return normalize_featured(display, &title);
    }
    Credits {
        artist: display_artist(&artists),
        title: clean_text(&title),
        artists,
    }
}

fn apply_display_joins(display: &str, artists: &mut [ArtistCredit]) {
    if artists.len() < 2 {
        return;
    }
    let display = clean_text(display);
    let Some(mut cursor) = display.find(&artists[0].name) else {
        return;
    };
    cursor += artists[0].name.len();
    let mut joins = Vec::with_capacity(artists.len() - 1);
    for next in &artists[1..] {
        let Some(relative) = display[cursor..].find(&next.name) else {
            return;
        };
        joins.push(display[cursor..cursor + relative].to_owned());
        cursor += relative + next.name.len();
    }
    if !display[cursor..].trim().is_empty() || joins.iter().any(|join| join.trim().is_empty()) {
        return;
    }
    for (credit, join) in artists.iter_mut().zip(joins) {
        credit.join_phrase = join;
    }
    if let Some(last) = artists.last_mut() {
        last.join_phrase.clear();
    }
}

pub fn display_artist(artists: &[ArtistCredit]) -> String {
    artists
        .iter()
        .map(|credit| format!("{}{}", clean_text(&credit.name), credit.join_phrase))
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn individual_names(artists: &[ArtistCredit]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for credit in artists {
        let name = clean_text(&credit.name);
        if !name.is_empty() && !names.iter().any(|other| same_name(other, &name)) {
            names.push(name);
        }
    }
    names
}

/// Remove performer credits from a release title without changing legitimate
/// title/version text. Release-type policy (for example, mapping a confirmed
/// standalone single to `Single`) is handled by metadata completion.
pub fn release_title_without_featured(value: &str) -> String {
    remove_feature_clause(value).0
}

fn dedupe_credits(artists: &mut Vec<ArtistCredit>) {
    let mut out: Vec<ArtistCredit> = Vec::new();
    for credit in artists.drain(..) {
        if let Some(existing) = out
            .iter_mut()
            .find(|existing| same_name(&existing.name, &credit.name))
        {
            if existing.musicbrainz_id.is_none() {
                existing.musicbrainz_id = credit.musicbrainz_id;
            }
            continue;
        }
        out.push(credit);
    }
    if let Some(last) = out.last_mut() {
        last.join_phrase.clear();
    }
    *artists = out;
}

fn parse_supported_credit(value: &str) -> Vec<ArtistCredit> {
    let mut rest = clean_text(value);
    let mut out = Vec::new();
    while !rest.is_empty() {
        let lower = rest.to_lowercase();
        let next = [" / ", " feat. ", " feat ", " ft. ", " ft ", "; "]
            .into_iter()
            .filter_map(|separator| lower.find(separator).map(|index| (index, separator)))
            .min_by_key(|(index, _)| *index);
        let Some((index, separator)) = next else {
            out.push(ArtistCredit::new(rest, ""));
            break;
        };
        let name = rest[..index].to_owned();
        let join = normalize_join_phrase(separator);
        out.push(ArtistCredit::new(name, join));
        rest = rest[index + separator.len()..].trim().to_owned();
    }
    out.retain(|credit| !credit.name.is_empty());
    out
}

fn normalize_join_phrase(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "feat" | "feat." | "ft" | "ft." | "featuring" => " feat. ".into(),
        "/" => " / ".into(),
        ";" => "; ".into(),
        _ => value.to_owned(),
    }
}

fn remove_feature_clause(title: &str) -> (String, Option<String>) {
    let title = clean_text(title);
    let lower = title.to_lowercase();
    for marker in ["(feat. ", "(feat ", "(ft. ", "(ft ", "(featuring "] {
        if let Some(start) = lower.find(marker)
            && let Some(relative_close) = title[start + marker.len()..].find(')')
        {
            let close = start + marker.len() + relative_close;
            let featured = clean_text(&title[start + marker.len()..close]);
            let cleaned = clean_text(&format!("{} {}", &title[..start], &title[close + 1..]));
            let (cleaned, _) = remove_feature_clause(&cleaned);
            return (cleaned, (!featured.is_empty()).then_some(featured));
        }
    }
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        if let Some(start) = lower.rfind(marker) {
            let featured = clean_text(&title[start + marker.len()..]);
            if !featured.is_empty() {
                return (clean_text(&title[..start]), Some(featured));
            }
        }
    }
    (title, None)
}

fn same_name(left: &str, right: &str) -> bool {
    identity_key(left) == identity_key(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_featured_artist_from_title_to_artist_credit() {
        let credits = normalize_featured("Arta", "Mi Amor (ft. Saaren)");
        assert_eq!(credits.title, "Mi Amor");
        assert_eq!(credits.artist, "Arta feat. Saaren");
        assert_eq!(individual_names(&credits.artists), ["Arta", "Saaren"]);
    }

    #[test]
    fn supported_navidrome_separators_are_parsed() {
        for artist in [
            "Alice / Bob",
            "Alice feat. Bob",
            "Alice feat Bob",
            "Alice ft. Bob",
            "Alice ft Bob",
            "Alice; Bob",
        ] {
            assert_eq!(
                individual_names(&normalize_featured(artist, "Song").artists),
                ["Alice", "Bob"],
                "{artist}"
            );
        }
    }

    #[test]
    fn unsupported_ambiguous_separators_are_not_parsed() {
        for artist in [
            "Simon & Garfunkel",
            "Earth, Wind & Fire",
            "A and B",
            "A x B",
            "AC/DC",
        ] {
            assert_eq!(
                normalize_featured(artist, "Song").artists.len(),
                1,
                "{artist}"
            );
        }
    }

    #[test]
    fn preserves_structured_co_primary_join() {
        let credits = normalize_structured(
            "Alice & Bob",
            "Song",
            vec![
                ArtistCredit::new("Alice", " & "),
                ArtistCredit::new("Bob", ""),
            ],
        );
        assert_eq!(credits.artist, "Alice & Bob");
        assert_eq!(individual_names(&credits.artists), ["Alice", "Bob"]);
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
        let credits = normalize_featured(
            "Arta",
            "Hanooz Yadame (feat. Koorosh, Sami Low & Raha) feat. Koorosh,Sami Low,Raha",
        );
        assert_eq!(credits.title, "Hanooz Yadame");
        assert_eq!(credits.artist, "Arta feat. Koorosh, Sami Low & Raha");
    }

    #[test]
    fn unicode_and_whitespace_are_canonicalized() {
        assert_eq!(clean_text("  Cafe\u{301}   Song "), "Café Song");
        assert_eq!(identity_key(" ANDY "), identity_key("Andy"));
    }

    #[test]
    fn removes_featured_credit_from_release_title_but_keeps_version_text() {
        assert_eq!(
            release_title_without_featured("Moorche (feat. Mehrad Hidden) - EP"),
            "Moorche - EP"
        );
        assert_eq!(
            release_title_without_featured("Midnight (Live)"),
            "Midnight (Live)"
        );
    }
}
