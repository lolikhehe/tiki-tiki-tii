//! Inline mode: cached video results, uncached placeholder articles, and the
//! `chosen_inline_result` hand-off to the Postgres job queue.

use crate::render::{self, Selection};
use crate::state::AppState;
use common::domain::{Lang, QualityTier, UserRecord};
use common::error::HandlerResult;
use common::i18n;
use teloxide::prelude::*;
use teloxide::types::{
    ChosenInlineResult, FileId, InlineQueryResult, InlineQueryResultArticle,
    InlineQueryResultCachedVideo, InputMessageContent, InputMessageContentText, ParseMode,
};
use tracing::warn;
use url::Url;

pub async fn handle_inline(
    state: AppState,
    bot: Bot,
    query: teloxide::types::InlineQuery,
) -> HandlerResult {
    let started = std::time::Instant::now();
    let user_id = query.from.id.0 as i64;
    let lang = Lang::from_telegram_hint(
        query.from.language_code.as_deref(),
        state.config.default_lang,
    );
    state
        .db
        .ensure_user(user_id, lang, state.config.default_max_height)
        .await?;
    let user = state.db.get_user(user_id).await?.unwrap_or(UserRecord {
        lang,
        max_height: state.config.default_max_height,
        banned: false,
    });

    let results = build_results(&state, &query.query, &user).await;
    bot.answer_inline_query(query.id, results)
        .cache_time(30)
        .is_personal(true)
        .await?;
    metrics::histogram!("video_bot_inline_answer_seconds").record(started.elapsed().as_secs_f64());
    Ok(())
}

async fn build_results(state: &AppState, text: &str, user: &UserRecord) -> Vec<InlineQueryResult> {
    let hint = || {
        vec![InlineQueryResult::Article(
            InlineQueryResultArticle::new(
                "hint",
                i18n::inline_hint(user.lang),
                InputMessageContent::Text(InputMessageContentText::new(i18n::inline_hint(
                    user.lang,
                ))),
            )
            .description(
                i18n::disclaimer(user.lang)
                    .replace("<b>", "")
                    .replace("</b>", ""),
            ),
        )]
    };

    let text = text.trim();
    if text.is_empty() {
        return hint();
    }
    let mut url = match Url::parse(text).or_else(|_| Url::parse(&format!("https://{text}"))) {
        Ok(url) => url,
        Err(_) => return hint(),
    };
    let parsed = match state.manager.registry().parse(&url) {
        Ok(parsed) => parsed,
        Err(_) => return hint(),
    };
    if parsed.needs_resolve {
        match state.manager.registry().resolve(url.clone()).await {
            Ok(resolved) => url = resolved,
            Err(_) => return hint(),
        }
    }
    if parsed.is_playlist {
        return hint();
    }

    let quality = QualityTier::from_max_height(user.max_height);
    let url_string = url.to_string();

    // Cached: a real video result, delivered instantly after selection.
    if let Some(cached) = match state.manager.get_cached(&parsed.key, quality).await {
        Ok(cached) => cached,
        Err(error) => {
            warn!(%error, "inline cache lookup failed");
            None
        }
    } {
        let caption = i18n::caption(
            &cached.meta.title,
            cached.meta.uploader.as_deref(),
            &cached.meta.page_url,
        );
        let mut result = InlineQueryResultCachedVideo::new(
            format!("c_{}", quality.as_str()),
            FileId(cached.file_id),
            i18n::truncate(&cached.meta.title, 128),
        )
        .caption(caption)
        .parse_mode(ParseMode::Html)
        .description(format!("{} · cached", quality));
        if let Some(id) = &cached.meta.uploader {
            result = result.description(i18n::truncate(id, 60));
        }
        return vec![InlineQueryResult::CachedVideo(result)];
    }

    // Uncached: an article with a keyboard (required for inline_message_id).
    let meta = state
        .manager
        .registry()
        .quick_meta(&parsed, user.lang)
        .await
        .unwrap_or_else(|_| common::domain::VideoMeta::fallback(&parsed.key, &url));
    let selection = Selection {
        key: parsed.key.clone(),
        url: url_string,
        quality,
        lang: user.lang,
    };
    let selection_id = match state
        .db
        .create_selection(selection.to_value(), std::time::Duration::from_secs(3600))
        .await
    {
        Ok(id) => id,
        Err(error) => {
            warn!(%error, "could not store inline selection");
            return hint();
        }
    };

    let mut article = InlineQueryResultArticle::new(
        selection_id,
        i18n::truncate(&meta.title, 128),
        InputMessageContent::Text(InputMessageContentText::new(i18n::inline_loading(
            user.lang,
        ))),
    )
    .description(format!(
        "{} · {}",
        parsed.key.source.display_name(),
        quality
    ))
    .reply_markup(render::inline_placeholder_keyboard());
    if let Some(thumb) = meta
        .thumbnail_url
        .as_deref()
        .and_then(|value| Url::parse(value).ok())
    {
        article = article.thumbnail_url(thumb);
    }

    let mut results = vec![InlineQueryResult::Article(article)];
    // Optional faster variant: 480p, only when the default is 1080p.
    if quality == QualityTier::P1080 {
        let mut faster = selection.clone();
        faster.quality = QualityTier::P480;
        if let Ok(id) = state
            .db
            .create_selection(faster.to_value(), std::time::Duration::from_secs(3600))
            .await
        {
            results.push(InlineQueryResult::Article(
                InlineQueryResultArticle::new(
                    id,
                    "480p (faster)".to_owned(),
                    InputMessageContent::Text(InputMessageContentText::new(i18n::inline_loading(
                        user.lang,
                    ))),
                )
                .description("480p · faster".to_owned())
                .reply_markup(render::inline_placeholder_keyboard()),
            ));
        }
    }
    results
}

pub async fn handle_chosen(state: AppState, bot: Bot, result: ChosenInlineResult) -> HandlerResult {
    // Cached results need no work — Telegram already sent the video.
    if result.result_id.starts_with("c_") || result.result_id == "hint" {
        return Ok(());
    }
    let Some(inline_message_id) = result.inline_message_id.clone() else {
        warn!("chosen inline result has no inline_message_id");
        return Ok(());
    };
    let value = match state.db.get_selection(&result.result_id).await {
        Ok(Some(value)) => value,
        Ok(None) => return Ok(()),
        Err(error) => {
            warn!(%error, "selection lookup failed");
            return Ok(());
        }
    };
    let Some(selection) = Selection::from_value(&value) else {
        return Ok(());
    };
    let user_id = result.from.id.0 as i64;
    let lang = selection.lang;
    let (job_id, _created) = state
        .db
        .enqueue_job(&selection.key, selection.quality, &selection.url)
        .await?;
    state
        .db
        .add_waiter(job_id, user_id, None, None, Some(&inline_message_id), lang)
        .await?;
    let _ = &bot;
    metrics::counter!("video_bot_inline_chosen_total").increment(1);
    Ok(())
}
