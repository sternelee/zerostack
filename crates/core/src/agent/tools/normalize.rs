pub fn normalize_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_count = 0u32;

    for line in s.lines() {
        let trimmed = line.trim_end().replace('\t', "    ");
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                out.push('\n');
            }
        } else {
            blank_count = 0;
            out.push_str(&trimmed);
            out.push('\n');
        }
    }

    out
}

pub fn normalize_unicode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\u{2010}'..='\u{2015}' | '\u{2212}' => out.push('-'),
            '\u{2018}'..='\u{201B}' => out.push('\''),
            '\u{201C}'..='\u{201F}' => out.push('"'),
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => {
                out.push(' ')
            }
            '\u{200B}'..='\u{200D}' | '\u{FEFF}' => {}
            '\u{2260}' => out.push_str("!="),
            '\u{00BD}' => out.push_str("1/2"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn strip_comment_prefixes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let trimmed = line.trim();
        let mut stripped = trimmed;
        for p in ["//", "-- ", "# ", "; ", "% "] {
            if stripped.starts_with(p) {
                stripped = stripped[p.len()..].trim_start();
                break;
            }
        }
        out.push_str(stripped);
        out.push('\n');
    }
    out
}

pub fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    let max_len = a_len.max(b_len);
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_distance(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}
