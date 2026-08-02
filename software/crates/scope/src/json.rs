//! Just enough JSON to embed a capture in a page.
//!
//! Hand-written rather than pulled in. The whole output is numbers, short
//! identifiers and a handful of strings this crate produced itself, so a
//! serialisation framework would be a dependency bought to escape twenty lines —
//! and `beam402-protocol` next door is dependency-free on purpose.

/// A JSON string literal. Escapes the five things that can appear in a label,
/// an event line or a mapping name, and `\u`-escapes the rest of the control
/// range so no output can break the page it is embedded in.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // A capture is embedded inside a <script> block. `</script>` inside a
            // string would end it, and `<script` inside one can push the HTML
            // parser into its double-escaped state, where the *next* `</script>`
            // stops meaning what it looks like. Escaping the angle brackets
            // outright removes both, and JSON reads `\u003c` as `<` regardless —
            // so nothing in a mapping file can be markup here even before the
            // page escapes it again on the way into the DOM.
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '/' => out.push_str("\\/"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn str_(key: &str, value: &str) -> String {
    format!("{}:{}", quote(key), quote(value))
}

pub fn num(key: &str, value: f64) -> String {
    // Finite by construction here, but a NaN would emit a token JSON has no word
    // for and break the page silently. Null is the honest stand-in.
    if value.is_finite() {
        format!("{}:{}", quote(key), trim(value))
    } else {
        format!("{}:null", quote(key))
    }
}

pub fn arr(key: &str, items: Vec<String>) -> String {
    format!("{}:[{}]", quote(key), items.join(","))
}

pub fn obj(fields: &[String]) -> String {
    format!("{{{}}}", fields.join(","))
}

fn trim(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_cannot_break_out_of_the_page_it_is_embedded_in() {
        assert_eq!(quote("</script>"), "\"\\u003c\\/script\\u003e\"");
        assert_eq!(quote("<script"), "\"\\u003cscript\"");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quote("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(quote("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn numbers_lose_their_trailing_zeroes_and_never_emit_nan() {
        assert_eq!(num("a", 1.5), "\"a\":1.5");
        assert_eq!(num("a", 402.336), "\"a\":402.336");
        assert_eq!(num("a", 7.0), "\"a\":7");
        assert_eq!(num("a", 0.0), "\"a\":0");
        assert_eq!(num("a", f64::NAN), "\"a\":null");
        assert_eq!(num("a", f64::INFINITY), "\"a\":null");
    }
}
