//! Postgres-backed job worker. Claims queued jobs, prepares the video, then
//! delivers exactly once to every waiter (private or inline).

use crate::render::{self, Selection};
use crate::send;
use common::domain::PreparedVideo;
use common::error::DownloadError;
use common::i18n;
use downloader::VideoRequest;
use std::time::Duration;
use storage::Job;
use teloxide::prelude::*;
use teloxide::types::{
    ChatId, FileId, InputFile, InputMedia, InputMediaVideo, MessageId, ParseMode,
};
use tracing::{info, warn};

pub fn spawn_worker(state: crate::state::AppState, bot: Bot) {
    tokio::spawn(async move {
        loop {
            match state.db.claim_job().await {
                Ok(Some(job)) => process_job(&state, &bot, job).await,
                Ok(None) => tokio::time::sleep(Duration::from_millis(200)).await,
                Err(error) => {
                    warn!(%error, "could not claim job");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });
}

async fn process_job(state: &crate::state::AppState, bot: &Bot, job: Job) {
    let started = std::time::Instant::now();
    info!(job_id = %job.id, key = %job.key, "processing job");
    let request = VideoRequest {
        key: job.key.clone(),
        url: job.url.clone(),
        meta: None,
    };
    let prepared = match state.manager.prepare(request, job.quality).await {
        Ok(prepared) => prepared,
        Err(error) => {
            fail_waiters(state, bot, &job, &error).await;
            let _ = state.db.fail_job(job.id, &error.to_string()).await;
            return;
        }
    };

    let waiters = match state.db.take_waiters(job.id).await {
        Ok(waiters) => waiters,
        Err(error) => {
            warn!(%error, "could not load waiters");
            let _ = state.db.fail_job(job.id, &error.to_string()).await;
            return;
        }
    };

    let mut cached_file = None;
    for waiter in waiters {
        let caption = i18n::caption(
            &prepared.meta.title,
            prepared.meta.uploader.as_deref(),
            &prepared.meta.page_url,
        );
        let selection = Selection {
            key: prepared.meta.key.clone(),
            url: job.url.clone(),
            quality: prepared.quality,
            lang: waiter.lang,
        };
        let markup = match state
            .db
            .create_selection(selection.to_value(), Duration::from_secs(24 * 3600))
            .await
        {
            Ok(id) => Some(render::quality_keyboard(waiter.lang, &id)),
            Err(error) => {
                warn!(%error, "could not create selection");
                None
            }
        };

        if let Some(inline_id) = &waiter.inline_message_id {
            match deliver_inline(state, bot, &prepared, &caption, inline_id, waiter.lang).await {
                Ok(ids) => {
                    if cached_file.is_none() {
                        cached_file = Some(ids);
                    }
                }
                Err(error) => warn!(%error, "inline delivery failed"),
            }
        } else if let Some(chat_id) = waiter.chat_id {
            match send::send_prepared(bot, ChatId(chat_id), &prepared, caption, markup).await {
                Ok(message) => {
                    if cached_file.is_none() {
                        cached_file = send::extract_file_id(&message);
                    }
                    if let Some(placeholder) = waiter.placeholder_message_id {
                        let _ = bot
                            .delete_message(ChatId(chat_id), MessageId(placeholder))
                            .await;
                    }
                    metrics::counter!("video_bot_uploads_total", "result" => "ok").increment(1);
                }
                Err(error) => {
                    warn!(%error, "private delivery failed");
                    metrics::counter!("video_bot_uploads_total", "result" => "error").increment(1);
                    let text = format!(
                        "❌ {}",
                        i18n::error_message(
                            waiter.lang,
                            &DownloadError::SourceUnavailable(error.to_string())
                        )
                    );
                    let _ = send::send_html(bot, ChatId(chat_id), text).await;
                }
            }
        }
    }

    if let Some((file_id, unique_id)) = cached_file
        && let Err(error) = state.manager.record(&prepared, file_id, unique_id).await
    {
        warn!(%error, "could not cache file id");
    }
    let _ = state.db.finish_job(job.id).await;
    metrics::histogram!("video_bot_upload_seconds").record(started.elapsed().as_secs_f64());
    info!(job_id = %job.id, elapsed_ms = started.elapsed().as_millis() as u64, "job done");
}

async fn fail_waiters(state: &crate::state::AppState, bot: &Bot, job: &Job, error: &DownloadError) {
    let waiters = match state.db.take_waiters(job.id).await {
        Ok(waiters) => waiters,
        Err(db_error) => {
            warn!(%db_error, "could not load waiters for failure");
            return;
        }
    };
    for waiter in waiters {
        let text = format!("❌ {}", i18n::error_message(waiter.lang, error));
        if let Some(inline_id) = &waiter.inline_message_id {
            let _ = bot
                .edit_message_text_inline(inline_id, text)
                .parse_mode(ParseMode::Html)
                .await;
        } else if let Some(chat_id) = waiter.chat_id {
            let _ = send::send_html(bot, ChatId(chat_id), text).await;
            if let Some(placeholder) = waiter.placeholder_message_id {
                let _ = bot
                    .delete_message(ChatId(chat_id), MessageId(placeholder))
                    .await;
            }
        }
    }
}

/// Upload to the storage chat to obtain a `file_id`, then swap the inline
/// placeholder for a real video. Retries `MESSAGE_ID_INVALID` three times;
/// falls back to a private-chat link button while still caching the file id.
async fn deliver_inline(
    state: &crate::state::AppState,
    bot: &Bot,
    prepared: &PreparedVideo,
    caption: &str,
    inline_id: &str,
    lang: common::domain::Lang,
) -> Result<(String, String), String> {
    let storage_chat = state
        .config
        .storage_chat_id
        .ok_or_else(|| "STORAGE_CHAT_ID is not configured".to_owned())?;
    let message = send::send_prepared(
        bot,
        ChatId(storage_chat),
        prepared,
        caption.to_owned(),
        None,
    )
    .await
    .map_err(|error| error.to_string())?;
    let (file_id, unique_id) =
        send::extract_file_id(&message).ok_or_else(|| "storage message has no video".to_owned())?;

    let mut last_error = None;
    for attempt in 0..3 {
        let media = InputMedia::Video(
            InputMediaVideo::new(InputFile::file_id(FileId(file_id.clone())))
                .caption(caption.to_owned())
                .parse_mode(ParseMode::Html)
                .duration(u16::try_from(prepared.duration_sec).unwrap_or(u16::MAX))
                .width(u16::try_from(prepared.width).unwrap_or(u16::MAX))
                .height(u16::try_from(prepared.height).unwrap_or(u16::MAX))
                .supports_streaming(true),
        );
        match bot.edit_message_media_inline(inline_id, media).await {
            Ok(_) => return Ok((file_id, unique_id)),
            Err(error) => {
                last_error = Some(error.to_string());
                if send::is_message_id_invalid(&error) && attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(800 * (attempt + 1) as u64)).await;
                    continue;
                }
                break;
            }
        }
    }

    warn!(?last_error, "editMessageMedia failed; using fallback link");
    let open_hint = match lang {
        common::domain::Lang::En => "Open the video in the bot",
        common::domain::Lang::Uk => "Відкрийте відео в боті",
    };
    let fallback_text = format!("{caption}\n\n{open_hint}");
    let markup = render::private_link_keyboard(&state.bot_username, &prepared.meta.key);
    let _ = bot
        .edit_message_text_inline(inline_id, fallback_text)
        .parse_mode(ParseMode::Html)
        .reply_markup(markup)
        .await;
    Ok((file_id, unique_id))
}
