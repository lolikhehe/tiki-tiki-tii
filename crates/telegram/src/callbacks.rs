//! Callback queries: quality picker, re-download at another quality, settings.

use crate::render::{self, Selection};
use crate::send;
use crate::state::AppState;
use common::domain::{Lang, QualityTier};
use common::error::HandlerResult;
use common::i18n;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};
use tracing::warn;

pub async fn handle_callback(
    state: AppState,
    bot: Bot,
    query: teloxide::types::CallbackQuery,
) -> HandlerResult {
    // Must come first, otherwise Telegram shows an endless spinner on the button.
    bot.answer_callback_query(query.id.clone()).await?;
    let Some(data) = query.data.clone() else {
        return Ok(());
    };
    let user_id = query.from.id.0 as i64;

    if let Some(id) = data.strip_prefix("q:") {
        let value = match state.db.get_selection(id).await {
            Ok(Some(value)) => value,
            _ => return Ok(()),
        };
        if Selection::from_value(&value).is_none() {
            return Ok(());
        }
        let buttons: Vec<Vec<InlineKeyboardButton>> = QualityTier::ALL
            .iter()
            .map(|quality| {
                vec![InlineKeyboardButton::callback(
                    format!("{quality}"),
                    format!("dl:{id}:{}", quality.as_str()),
                )]
            })
            .collect();
        if let Some(message) = &query.message {
            bot.edit_message_reply_markup(message.chat().id, message.id())
                .reply_markup(InlineKeyboardMarkup::new(buttons))
                .await?;
        }
    } else if let Some(rest) = data.strip_prefix("dl:") {
        let Some((id, quality_text)) = rest.split_once(':') else {
            return Ok(());
        };
        let Some(quality) = QualityTier::from_label(quality_text) else {
            return Ok(());
        };
        let value = match state.db.get_selection(id).await {
            Ok(Some(value)) => value,
            _ => return Ok(()),
        };
        let Some(selection) = Selection::from_value(&value) else {
            return Ok(());
        };
        let Some(message) = &query.message else {
            return Ok(());
        };
        let placeholder = send::send_html(
            &bot,
            message.chat().id,
            i18n::status_downloading(selection.lang),
        )
        .await
        .ok();
        let (job_id, _created) = state
            .db
            .enqueue_job(&selection.key, quality, &selection.url)
            .await?;
        state
            .db
            .add_waiter(
                job_id,
                user_id,
                Some(message.chat().id.0),
                placeholder.map(|message| message.id.0),
                None,
                selection.lang,
            )
            .await?;
    } else if let Some(quality_text) = data.strip_prefix("setq:") {
        let Some(quality) = QualityTier::from_label(quality_text) else {
            return Ok(());
        };
        state.db.set_user_quality(user_id, quality.height()).await?;
        let user = state
            .db
            .get_user(user_id)
            .await?
            .unwrap_or(common::domain::UserRecord {
                lang: state.config.default_lang,
                max_height: quality.height(),
                banned: false,
            });
        if let Some(message) = &query.message {
            bot.edit_message_text(
                message.chat().id,
                message.id(),
                i18n::settings_title(user.lang, quality, user.lang),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(render::settings_keyboard(user.lang, quality))
            .await?;
        }
    } else if let Some(lang_text) = data.strip_prefix("setlang:") {
        let Some(lang) = Lang::from_code(lang_text) else {
            return Ok(());
        };
        state.db.set_user_lang(user_id, lang).await?;
        let quality = QualityTier::from_max_height(state.config.default_max_height);
        if let Some(message) = &query.message {
            bot.edit_message_text(
                message.chat().id,
                message.id(),
                i18n::settings_title(lang, quality, lang),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(render::settings_keyboard(lang, quality))
            .await?;
        }
    } else if data == "noop" {
        // The inline placeholder button intentionally does nothing.
    } else {
        warn!(%data, "unknown callback data");
    }
    Ok(())
}
