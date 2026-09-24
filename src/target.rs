use crate::harness::{Family, Harness};
use crate::model::Encoding;

/// A vendor's model-name prefixes and the prose format its prompt guide recommends.
/// Supporting a new vendor is one row here plus a [`Family`] variant.
struct Vendor {
    family: Family,
    prefixes: &'static [&'static str],
    format: Encoding,
    reason: &'static str,
}

const VENDORS: &[Vendor] = &[
    Vendor {
        family: Family::Anthropic,
        prefixes: &["claude"],
        format: Encoding::Xml,
        reason: "XML tags: Anthropic's prompting guide recommends one tag per kind of content",
    },
    Vendor {
        family: Family::OpenAi,
        prefixes: &["gpt", "codex", "o1", "o3", "o4"],
        format: Encoding::Markdown,
        reason: "Markdown sections with an XML notes block: OpenAI's prompt guide (GPT-6 examples) uses Markdown headers for sections and XML tags around content, as Codex does for GPT-6",
    },
];

const UNKNOWN_REASON: &str = "XML tags: no target model known; both Anthropic and OpenAI models follow XML-delimited sections";
const PLAIN_REASON: &str = "Plain text: no two parts of the draft hold different kinds of content Jev confirmed, so it keeps its own layout";
const FALLBACK_REASON: &str =
    "the target's native format is unavailable because the text contains a colliding closing tag";

/// Who the payload is for. The model name decides the vendor; the harness is the
/// fallback when the model is unknown or unrecognized.
#[derive(Clone, Copy)]
pub(crate) struct Target<'a> {
    pub model: Option<&'a str>,
    pub harness: Option<&'static Harness>,
}

impl Target<'_> {
    pub(crate) fn family(self) -> Option<Family> {
        let model = self.model.unwrap_or_default().to_ascii_lowercase();
        VENDORS
            .iter()
            .find(|vendor| {
                vendor
                    .prefixes
                    .iter()
                    .any(|prefix| model.starts_with(prefix))
            })
            .map(|vendor| vendor.family)
            .or_else(|| self.harness.map(|harness| harness.family))
    }

    fn vendor(self) -> Option<&'static Vendor> {
        let family = self.family()?;
        VENDORS.iter().find(|vendor| vendor.family == family)
    }

    /// The standardized prose format for this target.
    pub(crate) fn native(self) -> Encoding {
        self.vendor().map_or(Encoding::Xml, |vendor| vendor.format)
    }

    pub(crate) fn reason(self, selected: Encoding) -> String {
        let why = match self.vendor() {
            _ if selected == Encoding::Plain => PLAIN_REASON,
            Some(vendor) if vendor.format == selected => vendor.reason,
            None if selected == Encoding::Xml => UNKNOWN_REASON,
            _ => FALLBACK_REASON,
        };
        let model = self.model.unwrap_or("unknown model");
        let harness = self
            .harness
            .map_or("unknown harness", |harness| harness.name);
        format!("{why} (target {model} via {harness}).")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::find;

    fn target(model: Option<&'static str>, harness: &str) -> Target<'static> {
        Target {
            model,
            harness: find(harness),
        }
    }

    #[test]
    fn the_model_name_wins_over_the_harness() {
        assert_eq!(
            target(Some("claude-opus-5-5"), "codex").native(),
            Encoding::Xml
        );
        assert_eq!(
            target(Some("gpt-6-sol"), "claude").native(),
            Encoding::Markdown
        );
        assert_eq!(
            target(Some("mystery-1"), "codex").native(),
            Encoding::Markdown
        );
        assert_eq!(target(None, "none").native(), Encoding::Xml);
        assert_eq!(target(None, "none").family(), None);
    }

    #[test]
    fn reasons_name_the_target() {
        let reason = target(Some("claude-opus-5-5"), "claude").reason(Encoding::Xml);
        assert!(reason.contains("Anthropic"));
        assert!(reason.contains("claude-opus-5-5 via Claude Code"));
        let fallback = target(Some("gpt-6-sol"), "codex").reason(Encoding::Xml);
        assert!(fallback.contains("unavailable"));
    }
}
