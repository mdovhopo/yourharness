//! Find in thread's matching: case- and accent-insensitive, as the Swift app's
//! (`.caseInsensitive, .diacriticInsensitive`). "cafe" finds "Café", "É" finds "e".

use std::ops::Range;

/// A character without case or accent: lower case, then Latin letters' accents dropped.
pub fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    if c.is_ascii() {
        return c;
    }
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' | 'ȧ' | 'ạ' | 'ả' | 'ấ' | 'ầ' | 'ẩ' | 'ẫ' | 'ậ' | 'ắ' | 'ằ' | 'ẳ' | 'ẵ' | 'ặ' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'ẹ' | 'ẻ' | 'ẽ' | 'ế' | 'ề' | 'ể' | 'ễ' | 'ệ' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' | 'ỉ' | 'ị' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ǒ' | 'ọ' | 'ỏ' | 'ố' | 'ồ' | 'ổ' | 'ỗ' | 'ộ' | 'ớ' | 'ờ' | 'ở' | 'ỡ' | 'ợ' | 'ơ' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
        'ţ' | 'ť' | 'ŧ' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ǔ' | 'ư' | 'ụ' | 'ủ' | 'ứ' | 'ừ' | 'ử' | 'ữ' | 'ự' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' | 'ỳ' | 'ỵ' | 'ỷ' | 'ỹ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        'й' => 'и',
        'ё' => 'е',
        other => other,
    }
}

/// What you typed, ready to match: folded, trimmed. Empty: nothing to find.
pub fn query(text: &str) -> String {
    text.trim().chars().map(fold_char).collect()
}

/// Whether `text` has the folded `query` anywhere.
pub fn contains(text: &str, query: &str) -> bool {
    !query.is_empty() && first(text, query, 0).is_some()
}

/// Where the folded `query` is in `text`, as byte ranges of `text`, left to right, not overlapping.
pub fn ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut out = vec![];
    if query.is_empty() {
        return out;
    }
    let mut from = 0;
    while let Some(r) = first(text, query, from) {
        from = r.end;
        out.push(r);
    }
    out
}

/// The first match at or after byte `from`.
fn first(text: &str, query: &str, from: usize) -> Option<Range<usize>> {
    let wanted: Vec<char> = query.chars().collect();
    let chars: Vec<(usize, char)> = text[from..].char_indices().map(|(i, c)| (i + from, fold_char(c))).collect();
    if wanted.len() > chars.len() {
        return None;
    }
    (0..=chars.len() - wanted.len()).find_map(|s| {
        wanted.iter().enumerate().all(|(k, w)| chars[s + k].1 == *w).then(|| {
            let end = chars.get(s + wanted.len()).map_or(text.len(), |(i, _)| *i);
            chars[s].0..end
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_case_and_accents() {
        assert!(contains("Le Café du Coin", &query("cafe")));
        assert!(contains("resume", &query("RÉSUMÉ")));
        assert!(contains("x", &query("X")));
        assert!(!contains("anything", &query("  ")));
        assert!(!contains("abc", &query("abd")));
    }

    #[test]
    fn finds_every_place_in_bytes() {
        let text = "Ça va, ça VA bien";
        let r = ranges(text, &query("ça"));
        assert_eq!(r.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["Ça", "ça"]);
        assert_eq!(ranges("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(ranges("naïve", &query("i")), vec![2..4]);
        assert!(ranges("short", "longer query").is_empty());
    }
}
