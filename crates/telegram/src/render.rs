//! Inline keyboards and short selection payloads (stored in Postgres JSONB).

use common::domain::{Lang, QualityTier, VideoKey};
use common::i18n;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub key: VideoKey,
    pub url: String,
    pub quality: QualityTier,
    pub lang: Lang,
}

impl Selection {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_value(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

/// Button under a sent video: opens a quality picker. The callback carries
/// only the short selection id, never the URL or metadata.
pub fn quality_keyboard(lang: Lang, selection_id: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        i18n::another_quality(lang),
        format!("q:{selection_id}"),
    )]])
}

pub fn settings_keyboard(lang: Lang, current: QualityTier) -> InlineKeyboardMarkup {
    let mut quality_row = Vec::new();
    for quality in QualityTier::ALL {
        let marker = if quality == current { "✅ " } else { "" };
        quality_row.push(InlineKeyboardButton::callback(
            format!("{marker}{quality}"),
            format!("setq:{}", quality.as_str()),
        ));
    }
    let lang_button = InlineKeyboardButton::callback(
        i18n::language_name(lang),
        format!("setlang:{}", lang.code()),
    );
    InlineKeyboardMarkup::new(vec![quality_row, vec![lang_button]])
}

/// Fallback for inline when `editMessageMedia` keeps failing: a link that
/// opens a private chat with the bot and triggers delivery via `/start v_<id>`.
pub fn private_link_keyboard(bot_username: &str, key: &VideoKey) -> InlineKeyboardMarkup {
    let start_param = format!("v_{}", key.as_str());
    let url = Url::parse_with_params(
        &format!("https://t.me/{bot_username}"),
        [("start", start_param.as_str())],
    )
    .unwrap_or_else(|_| {
        Url::parse(&format!("https://t.me/{bot_username}")).expect("valid bot link")
    });
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url("▶️ Open in bot", url)]])
}

/// The single button attached to an uncached inline article. Telegram only
/// provides `inline_message_id` in `chosen_inline_result` when the result has
/// an inline keyboard, so this button is required for the whole flow.
pub fn inline_placeholder_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback("⏳", "noop")]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_roundtrip() {
        let selection = Selection {
            key: VideoKey::new(common::domain::SourceKind::YouTube, "abc"),
            url: "https://youtu.be/abc".into(),
            quality: QualityTier::P720,
            lang: Lang::Uk,
        };
        let value = selection.to_value();
        let parsed = Selection::from_value(&value).expect("selection");
        assert_eq!(parsed.key.id, "abc");
        assert_eq!(parsed.quality, QualityTier::P720);
        assert_eq!(parsed.lang, Lang::Uk);
    }
}
