//! Find in thread's matching: case- and accent-insensitive.
//! "cafe" finds "Café", "É" finds "e".

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
    if query.is_empty() {
        return false;
    }
    if text.is_ascii() && query.is_ascii() {
        return contains_ascii(text.as_bytes(), query.as_bytes());
    }
    contains_unicode(text, query)
}

/// Where the folded `query` is in `text`, as byte ranges of `text`, left to right, not overlapping.
pub fn ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return vec![];
    }
    if text.is_ascii() && query.is_ascii() {
        return ranges_ascii(text.as_bytes(), query.as_bytes());
    }
    ranges_unicode(text, query)
}

fn contains_ascii(tb: &[u8], qb: &[u8]) -> bool {
    let qlen = qb.len();
    if qlen > tb.len() || qb.iter().any(u8::is_ascii_uppercase) {
        return false;
    }
    let lo = qb[0];
    let hi = lo.to_ascii_uppercase();
    let last = tb.len() - qlen;
    let mut i = 0;
    while i <= last {
        let b = tb[i];
        if (b == lo || b == hi) && tb[i..i + qlen].eq_ignore_ascii_case(qb) {
            return true;
        }
        i += 1;
    }
    false
}

fn ranges_ascii(tb: &[u8], qb: &[u8]) -> Vec<Range<usize>> {
    let qlen = qb.len();
    if qlen > tb.len() || qb.iter().any(u8::is_ascii_uppercase) {
        return vec![];
    }
    let lo = qb[0];
    let hi = lo.to_ascii_uppercase();
    let last = tb.len() - qlen;
    let mut out = vec![];
    let mut i = 0;
    while i <= last {
        let b = tb[i];
        if (b == lo || b == hi) && tb[i..i + qlen].eq_ignore_ascii_case(qb) {
            out.push(i..i + qlen);
            i += qlen;
        } else {
            i += 1;
        }
    }
    out
}

fn contains_unicode(text: &str, query: &str) -> bool {
    let wanted: Vec<char> = query.chars().collect();
    if wanted.is_empty() {
        return false;
    }
    if wanted.len() == 1 {
        let w = wanted[0];
        return text.chars().any(|c| fold_char(c) == w);
    }
    // KMP over folded chars: O(N + M) worst-case with zero allocation for `text`.
    let mut lps = vec![0usize; wanted.len()];
    let mut len = 0;
    let mut i = 1;
    while i < wanted.len() {
        if wanted[i] == wanted[len] {
            len += 1;
            lps[i] = len;
            i += 1;
        } else if len > 0 {
            len = lps[len - 1];
        } else {
            i += 1;
        }
    }
    let mut j = 0;
    for c in text.chars() {
        let fc = fold_char(c);
        while j > 0 && fc != wanted[j] {
            j = lps[j - 1];
        }
        if fc == wanted[j] {
            j += 1;
            if j == wanted.len() {
                return true;
            }
        }
    }
    false
}

fn ranges_unicode(text: &str, query: &str) -> Vec<Range<usize>> {
    let wanted: Vec<char> = query.chars().collect();
    let wlen = wanted.len();
    if wlen == 0 {
        return vec![];
    }
    let chars: Vec<(usize, char)> = text.char_indices().map(|(i, c)| (i, fold_char(c))).collect();
    if wlen > chars.len() {
        return vec![];
    }
    let w0 = wanted[0];
    let last = chars.len() - wlen;
    let mut out = vec![];
    let mut s = 0;
    while s <= last {
        if chars[s].1 == w0 && (1..wlen).all(|k| chars[s + k].1 == wanted[k]) {
            let end = chars.get(s + wlen).map_or(text.len(), |(i, _)| *i);
            out.push(chars[s].0..end);
            s += wlen;
        } else {
            s += 1;
        }
    }
    out
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

    #[test]
    fn ascii_and_unicode_paths_match_edge_cases() {
        // Pure ASCII fast path: case-insensitivity, non-overlapping, uppercase query rejected.
        assert_eq!(ranges("Foo fOO FOO bar foo", "foo"), vec![0..3, 4..7, 8..11, 16..19]);
        assert!(contains("Foo BAR", "bar"));
        assert!(!contains("Foo BAR", "BAR"));
        assert!(ranges("Foo BAR", "BAR").is_empty());
        // Unicode KMP fallback (repeating prefix) and multi-byte folding.
        assert!(contains("aaaaéaaaaab", "aaaab"));
        assert!(!contains("aaaaéaaaaac", "aaaab"));
        assert!(contains("café", "e"));
        assert_eq!(ranges("éÉe", "e"), vec![0..2, 2..4, 4..5]);
    }
}
