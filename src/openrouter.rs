//! OpenRouter's models in the app: telling a model of OpenRouter's by its
//! name.

/// How `/model` names a model of OpenRouter's: `openrouter:vendor/slug`.
pub const PREFIX: &str = "openrouter:";

/// The slug of `name` when it names one of OpenRouter's models
/// (`openrouter:moonshotai/kimi-k3`), as arko takes it: any slug that
/// isn't empty and has no spaces.
pub fn slug(name: &str) -> Option<&str> {
    name.strip_prefix(PREFIX)
        .filter(|slug| !slug.is_empty() && !slug.contains(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn a_slug_is_what_follows_the_prefix() {
        assert_eq!(slug("openrouter:z-ai/glm-5"), Some("z-ai/glm-5"));
        assert_eq!(slug("openrouter:"), None);
        assert_eq!(slug("claude-opus-5-5"), None);
        assert_eq!(slug("openrouter:a b"), None);
    }
}
