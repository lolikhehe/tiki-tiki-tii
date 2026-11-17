//! Private-chat commands and link messages.

use crate::render::{self, Selection};
use crate::send;
use crate::state::AppState;
use common::domain::{Lang, QualityTier, SourceKind, UserRecord, VideoKey};
use common::error::{DownloadError, HandlerResult};
use common::i18n;
use teloxide::prelude::*;
use teloxide::types::{Message, ParseMode, ReplyMarkup};
use tracing::warn;
use url::Url;

#[derive(Debug, Clone)]
enum Command {
    Start(Option<String>),
    Help,
    Settings,
    Stats,
    Ban(Option<i64>),
    Unban(Option<i64>),
    Purge(Option<String>),
    Unknown,
}

fn parse_command(text: &str, bot_username: &str) -> Option<Command> {
    let rest = text.trim().strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let head = parts.next()?;
    let (name, target) = head
        .split_once('@')
        .map(|(name, target)| (name, Some(target)))
        .unwrap_or((head, None));
    if let Some(target) = target
        && !target.eq_ignore_ascii_case(bot_username)
    {
        return None;
    }
    let args = parts.collect::<Vec<_>>().join(" ");
    Some(match name.to_ascii_lowercase().as_str() {
        "start" => Command::Start((!args.is_empty()).then_some(args)),
        "help" => Command::Help,
        "settings" => Command::Settings,
        "stats" => Command::Stats,
        "ban" => Command::Ban(args.parse().ok()),
        "unban" => Command::Unban(args.parse().ok()),
        "purge" => Command::Purge((!args.is_empty()).then_some(args)),
        _other => Command::Unknown,
    })
}

pub async fn handle_message(state: AppState, bot: Bot, message: Message) -> HandlerResult {
    let Some(from) = message.from.as_ref() else {
        return Ok(());
    };
    let user_id = from.id.0 as i64;
    let lang_hint = from.language_code.as_deref();
    let lang = Lang::from_telegram_hint(lang_hint, state.config.default_lang);
    state
        .db
        .ensure_user(user_id, lang, state.config.default_max_height)
        .await?;
    let user = state.db.get_user(user_id).await?.unwrap_or(UserRecord {
        lang,
        max_height: state.config.default_max_height,
        banned: false,
    });
    if user.banned {
        return Ok(());
    }

    let Some(text) = message.text() else {
        return Ok(());
    };
    if let Some(command) = parse_command(text, &state.bot_username) {
        return handle_command(&state, &bot, user_id, &user, command).await;
    }
    if !state.rate_limiter.allow(user_id) {
        send::send_html(&bot, message.chat.id, i18n::rate_limited(user.lang)).await?;
        return Ok(());
    }
    handle_url(&state, &bot, &message, text.trim(), &user).await
}

async fn handle_command(
    state: &AppState,
    bot: &Bot,
    user_id: i64,
    user: &UserRecord,
    command: Command,
) -> HandlerResult {
    let chat = ChatId(user_id);
    match command {
        Command::Start(Some(payload)) if payload.starts_with("v_") => {
            deliver_start_payload(state, bot, chat, &payload[2..], user).await?;
        }
        Command::Start(_) => {
            send::send_html(bot, chat, i18n::start(user.lang)).await?;
        }
        Command::Help => {
            send::send_html(
                bot,
                chat,
                i18n::help(user.lang, state.config.is_admin(user_id)),
            )
            .await?;
        }
        Command::Settings => {
            let quality = QualityTier::from_max_height(user.max_height);
            bot.send_message(chat, i18n::settings_title(user.lang, quality, user.lang))
                .parse_mode(ParseMode::Html)
                .reply_markup(ReplyMarkup::InlineKeyboard(render::settings_keyboard(
                    user.lang, quality,
                )))
                .await?;
        }
        Command::Stats if state.config.is_admin(user_id) => {
            let stats = state.db.stats().await?;
            send::send_html(bot, chat, i18n::stats(user.lang, &stats)).await?;
        }
        Command::Ban(Some(target)) if state.config.is_admin(user_id) => {
            state.db.set_banned(target, true).await?;
            send::send_html(bot, chat, i18n::banned(user.lang)).await?;
        }
        Command::Unban(Some(target)) if state.config.is_admin(user_id) => {
            state.db.set_banned(target, false).await?;
            send::send_html(bot, chat, i18n::unbanned(user.lang)).await?;
        }
        Command::Purge(Some(key_text)) if state.config.is_admin(user_id) => {
            let key = parse_video_key(&key_text);
            match key {
                Some(key) if state.manager.purge(&key).await? => {
                    send::send_html(bot, chat, i18n::purged(user.lang)).await?;
                }
                _ => {
                    send::send_html(bot, chat, i18n::not_found_key(user.lang)).await?;
                }
            }
        }
        Command::Stats | Command::Ban(_) | Command::Unban(_) | Command::Purge(_) => {
            // Non-admins get no admin output at all.
        }
        Command::Unknown => {
            send::send_html(bot, chat, i18n::help(user.lang, false)).await?;
        }
    }
    Ok(())
}

async fn deliver_start_payload(
    state: &AppState,
    bot: &Bot,
    chat: ChatId,
    key_text: &str,
    user: &UserRecord,
) -> HandlerResult {
    let Some(key) = parse_video_key(key_text) else {
        return Ok(());
    };
    let quality = QualityTier::from_max_height(user.max_height);
    if let Some(cached) = state.manager.get_cached(&key, quality).await? {
        let caption = i18n::caption(
            &cached.meta.title,
            cached.meta.uploader.as_deref(),
            &cached.meta.page_url,
        );
        if send::send_cached(bot, chat, &cached, caption, None)
            .await
            .is_ok()
        {
            state.manager.touch_cached(&key, quality).await;
            return Ok(());
        }
    }
    send::send_html(bot, chat, i18n::invalid_link(user.lang)).await?;
    Ok(())
}

fn parse_video_key(text: &str) -> Option<VideoKey> {
    let (source, id) = text.trim().split_once(':')?;
    Some(VideoKey::new(source.parse::<SourceKind>().ok()?, id))
}

async fn handle_url(
    state: &AppState,
    bot: &Bot,
    message: &Message,
    text: &str,
    user: &UserRecord,
) -> HandlerResult {
    let mut url = match Url::parse(text) {
        Ok(url) => url,
        Err(_) => match Url::parse(&format!("https://{text}")) {
            Ok(url) => url,
            Err(_) => {
                send::send_html(bot, message.chat.id, i18n::invalid_link(user.lang)).await?;
                return Ok(());
            }
        },
    };

    let parsed = match state.manager.registry().parse(&url) {
        Ok(parsed) => parsed,
        Err(error) => {
            let message_error: DownloadError = error.into();
            send::send_html(
                bot,
                message.chat.id,
                i18n::error_message(user.lang, &message_error),
            )
            .await?;
            return Ok(());
        }
    };
    if parsed.needs_resolve {
        url = match state.manager.registry().resolve(url.clone()).await {
            Ok(resolved) => resolved,
            Err(error) => {
                let message_error: DownloadError = error.into();
                send::send_html(
                    bot,
                    message.chat.id,
                    i18n::error_message(user.lang, &message_error),
                )
                .await?;
                return Ok(());
            }
        };
    }
    if parsed.is_playlist {
        send::send_html(
            bot,
            message.chat.id,
            i18n::error_message(user.lang, &DownloadError::Playlist),
        )
        .await?;
        return Ok(());
    }

    let quality = QualityTier::from_max_height(user.max_height);
    let url_string = url.to_string();

    // Fast path: cached file id, no download at all.
    if let Some(cached) = state.manager.get_cached(&parsed.key, quality).await? {
        let caption = i18n::caption(
            &cached.meta.title,
            cached.meta.uploader.as_deref(),
            &cached.meta.page_url,
        );
        let selection = Selection {
            key: parsed.key.clone(),
            url: url_string.clone(),
            quality,
            lang: user.lang,
        };
        let markup = state
            .db
            .create_selection(
                selection.to_value(),
                std::time::Duration::from_secs(24 * 3600),
            )
            .await
            .ok()
            .map(|id| render::quality_keyboard(user.lang, &id));
        match send::send_cached(bot, message.chat.id, &cached, caption, markup).await {
            Ok(_) => {
                state.manager.touch_cached(&parsed.key, quality).await;
                metrics::counter!("video_bot_cache_hits_total").increment(1);
                return Ok(());
            }
            Err(error) if send::is_invalid_file_id(&error) => {
                warn!(%error, key = %parsed.key, "stale file id; purging cache");
                state.manager.purge(&parsed.key).await?;
            }
            Err(error) => return Err(Box::new(error)),
        }
    }
    metrics::counter!("video_bot_cache_misses_total").increment(1);

    // Cold path: a progress message, then a durable job + waiter.
    let placeholder = send::send_html(bot, message.chat.id, i18n::status_downloading(user.lang))
        .await
        .ok();
    let (job_id, _created) = state
        .db
        .enqueue_job(&parsed.key, quality, &url_string)
        .await?;
    state
        .db
        .add_waiter(
            job_id,
            message.chat.id.0,
            Some(message.chat.id.0),
            placeholder.map(|message| message.id.0),
            None,
            user.lang,
        )
        .await?;
    Ok(())
}
