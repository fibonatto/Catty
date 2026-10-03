//! Minimal JSON helpers: encode a string, and pull one string field out of a document.


pub(crate) fn jstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn hex4(it: &mut std::str::Chars) -> Option<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        v = v * 16 + it.next()?.to_digit(16)?;
    }
    Some(v)
}

// `s` starts right after the opening quote.
fn decode_json_string(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                '/' => out.push('/'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' => {
                    let hi = hex4(&mut it)?;
                    if (0xD800..0xDC00).contains(&hi) {
                        if it.next()? != '\\' || it.next()? != 'u' {
                            return None;
                        }
                        let lo = hex4(&mut it)?;
                        let cp =
                            0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                    } else {
                        out.push(char::from_u32(hi).unwrap_or('\u{FFFD}'));
                    }
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

pub(crate) fn json_string_field(src: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0;
    while let Some(pos) = src[from..].find(&needle) {
        let after = from + pos + needle.len();
        let rest = src[after..].trim_start();
        if let Some(r) = rest.strip_prefix(':') {
            if let Some(r) = r.trim_start().strip_prefix('"') {
                return decode_json_string(r);
            }
        }
        from = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_decode() {
        let j = r#"{"model":"x","response":"hi \"there\"\nok \u00e9 \ud83d\ude00","done":true}"#;
        assert_eq!(
            json_string_field(j, "response").as_deref(),
            Some("hi \"there\"\nok é 😀")
        );
    }
}
