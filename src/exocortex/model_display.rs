//! Model display names (WP-C2, PAR-118): the short labels Exocortex's
//! `shared/src/model-display.ts` derives from canonical model ids, for the
//! metadata footer and anywhere else a model is named in passing.

/// `claude-opus-5-5` → `Opus-5.5`, `deepseek-v4-pro` → `DeepSeek V4 Pro`,
/// `gpt-6-astra` → `GPT-6-Astra`; anything else with its first letter
/// capitalised.
pub fn display_name(id: &str) -> String {
    match id {
        "ox-alpha" => return "Ox Alpha".into(),
        "gpt-6-astra" => return "GPT-6-Astra".into(),
        "gpt-6.1-sol" => return "GPT-6.1-Sol".into(),
        "gpt-6-sol" => return "GPT-6-Sol".into(),
        "gpt-6-sol-daybreak" => return "GPT-6-Sol-Daybreak".into(),
        "gpt-6-luna" => return "GPT-6-Luna".into(),
        "gpt-daybreak-blue-latest" => return "Daybreak Blue".into(),
        _ => {}
    }
    deepseek(id)
        .or_else(|| claude(id))
        .unwrap_or_else(|| capitalized(id))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn digits(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_digit).count()
}

/// `^deepseek-v(\d+)-(.+)$` (case-insensitive).
fn deepseek(id: &str) -> Option<String> {
    let lower = id.to_ascii_lowercase();
    let rest = lower.strip_prefix("deepseek-v")?;
    let n = digits(rest);
    let tier = rest.get(n..)?.strip_prefix('-')?;
    (n > 0 && !tier.is_empty()).then(|| format!("DeepSeek V{} {}", &rest[..n], capitalized(tier)))
}

/// `^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?=$|[-[])` (case-insensitive): the
/// family and version, without a date, build or `[1m]` suffix.
fn claude(id: &str) -> Option<String> {
    let lower = id.to_ascii_lowercase();
    let rest = lower.strip_prefix("claude-")?;
    let family_len = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    let (family, rest) = rest.split_at(family_len);
    let rest = rest.strip_prefix('-')?;
    let major_len = digits(rest);
    if family.is_empty() || major_len == 0 {
        return None;
    }
    let (major, rest) = rest.split_at(major_len);
    let ends = |tail: &str| tail.is_empty() || tail.starts_with('-') || tail.starts_with('[');
    let minor = rest.strip_prefix('-').and_then(|after| {
        let n = digits(after);
        ((1..=2).contains(&n) && ends(&after[n..])).then(|| &after[..n])
    });
    if minor.is_none() && !ends(rest) {
        return None;
    }
    Some(match minor {
        Some(minor) => format!("{}-{major}.{minor}", capitalized(family)),
        None => format!("{}-{major}", capitalized(family)),
    })
}

#[cfg(test)]
mod tests {
    use super::display_name;

    /// The cases of Exocortex's `model-display.ts` and its tests.
    #[test]
    fn labels_follow_exocortex() {
        for (id, label) in [
            ("claude-opus-5-5", "Opus-5.5"),
            ("claude-sonnet-5-5", "Sonnet-5.5"),
            ("claude-haiku-5-5", "Haiku-5.5"),
            ("claude-fable-5-1", "Fable-5.1"),
            ("claude-sonnet-4-20250514", "Sonnet-4"),
            ("claude-opus-4-1-20250805", "Opus-4.1"),
            ("claude-opus-5-5[1m]", "Opus-5.5"),
            ("claude-sonnet-4[1m]", "Sonnet-4"),
            ("gpt-5.4", "Gpt-5.4"),
            ("gpt-5.4-mini", "Gpt-5.4-mini"),
            ("deepseek-v4-pro", "DeepSeek V4 Pro"),
            ("DeepSeek-V3-FLASH", "DeepSeek V3 Flash"),
            ("gpt-6-astra", "GPT-6-Astra"),
            ("gpt-6.1-sol", "GPT-6.1-Sol"),
            ("gpt-daybreak-blue-latest", "Daybreak Blue"),
            ("ox-alpha", "Ox Alpha"),
            ("claude", "Claude"),
            ("claude-opus", "Claude-opus"),
            ("claude-opus-5x", "Claude-opus-5x"),
            ("", ""),
        ] {
            assert_eq!(display_name(id), label, "{id}");
        }
    }
}
