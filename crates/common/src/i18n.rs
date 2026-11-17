//! All user-facing strings live here. English is the fallback language.

use crate::domain::{Lang, QualityTier, Stats};
use crate::error::DownloadError;

fn pick(lang: Lang, en: &'static str, uk: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Uk => uk,
    }
}

/// Escape text for `ParseMode::Html` without pulling in an extra crate.
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn disclaimer(lang: Lang) -> &'static str {
    pick(
        lang,
        "⚖️ <b>Use responsibly.</b> You are responsible for the videos you download. Respect copyright and platform terms. The bot does not bypass DRM, logins or paid access; it stores only metadata and Telegram file identifiers.",
        "⚖️ <b>Користуйтеся відповідально.</b> Ви несете відповідальність за відео, які завантажуєте. Поважайте авторське право й умови платформ. Бот не обходить DRM, вхід чи платний доступ і зберігає лише метадані та ідентифікатори файлів Telegram.",
    )
}

pub fn start(lang: Lang) -> String {
    let intro = pick(
        lang,
        "🎬 <b>Video Bot</b>\n\nSend a link to a video (YouTube, TikTok, Instagram Reels, X, Reddit, Vimeo) and I will send it back as a video with a thumbnail and caption.\n\nInline: type <code>@your_bot &lt;link&gt;</code> in any chat. Use /settings to pick the default quality.",
        "🎬 <b>Відеобот</b>\n\nНадішліть посилання на відео (YouTube, TikTok, Instagram Reels, X, Reddit, Vimeo) — я надішлю його назад як відео з мініатюрою та підписом.\n\nІнлайн: напишіть <code>@your_bot &lt;посилання&gt;</code> у будь-якому чаті. Якість за замовчуванням — у /settings.",
    );
    format!("{intro}\n\n{}", disclaimer(lang))
}

pub fn help(lang: Lang, is_admin: bool) -> String {
    let base = pick(
        lang,
        "<b>How to use</b>\n\n• Private: send a video link.\n• Inline: <code>@your_bot &lt;link&gt;</code> in any chat.\n• Repeat requests are served instantly from cache.\n\n<b>Commands</b>\n/start — about\n/help — this message\n/settings — quality and language",
        "<b>Як користуватися</b>\n\n• Приват: надішліть посилання на відео.\n• Інлайн: <code>@your_bot &lt;посилання&gt;</code> у будь-якому чаті.\n• Повторні запити миттєво віддаються з кешу.\n\n<b>Команди</b>\n/start — про бота\n/help — ця довідка\n/settings — якість і мова",
    );
    if is_admin {
        format!(
            "{base}\n\n<b>Admin</b>\n/stats — counters\n/ban &lt;id&gt;\n/unban &lt;id&gt;\n/purge &lt;video_key&gt;"
        )
    } else {
        base.to_owned()
    }
}

pub fn settings_title(lang: Lang, quality: QualityTier, language: Lang) -> String {
    let title = pick(lang, "<b>Settings</b>", "<b>Налаштування</b>");
    let q = pick(lang, "Quality", "Якість");
    let l = pick(lang, "Language", "Мова");
    format!(
        "{title}\n\n{q}: <b>{quality}</b>\n{l}: <b>{}</b>",
        language_name(language)
    )
}

pub fn language_name(lang: Lang) -> &'static str {
    pick(lang, "English", "Українська")
}

pub fn quality_button(quality: QualityTier) -> String {
    format!("🎚 {quality}")
}

pub fn another_quality(lang: Lang) -> &'static str {
    pick(lang, "Other quality", "Інша якість")
}

pub fn inline_hint(lang: Lang) -> &'static str {
    pick(
        lang,
        "Paste a video link after @your_bot",
        "Встав посилання на відео після @your_bot",
    )
}

pub fn inline_loading(lang: Lang) -> &'static str {
    pick(lang, "⏳ Downloading video…", "⏳ Завантажую відео…")
}

pub fn status_downloading(lang: Lang) -> &'static str {
    pick(lang, "⬇️ Downloading…", "⬇️ Завантажую…")
}

pub fn status_processing(lang: Lang) -> &'static str {
    pick(lang, "⚙️ Processing…", "⚙️ Обробляю…")
}

pub fn status_uploading(lang: Lang) -> &'static str {
    pick(lang, "⬆️ Uploading…", "⬆️ Надсилаю…")
}

pub fn done(lang: Lang) -> &'static str {
    pick(lang, "✅ Done", "✅ Готово")
}

pub fn caption(title: &str, uploader: Option<&str>, page_url: &str) -> String {
    let mut lines = vec![escape_html(title)];
    if let Some(uploader) = uploader.filter(|value| !value.trim().is_empty()) {
        lines.push(format!("👤 {}", escape_html(uploader)));
    }
    lines.push(escape_html(page_url));
    lines.join("\n")
}

/// Truncate to a number of characters (not bytes) so UTF-8 never breaks.
pub fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub fn error_message(lang: Lang, error: &DownloadError) -> String {
    let text = match error {
        DownloadError::Private => pick(
            lang,
            "This video is private or requires a login. I can't download it.",
            "Це відео приватне або потребує входу. Я не можу його завантажити.",
        ),
        DownloadError::RegionLocked => pick(
            lang,
            "This video is not available in my region.",
            "Це відео недоступне в моєму регіоні.",
        ),
        DownloadError::AgeRestricted => pick(
            lang,
            "This video has an age restriction and requires a login.",
            "Це відео має вікове обмеження й потребує входу.",
        ),
        DownloadError::TooLong { limit, actual } => {
            return pick(
                lang,
                "This video is too long ({actual}s, limit {limit}s).",
                "Це відео занадто довге ({actual} с, ліміт {limit} с).",
            )
            .replace("{actual}", &actual.to_string())
            .replace("{limit}", &limit.to_string());
        }
        DownloadError::TooLarge { limit, actual } => {
            return pick(
                lang,
                "This file is too large ({actual} MB, limit {limit} MB).",
                "Цей файл занадто великий ({actual} МБ, ліміт {limit} МБ).",
            )
            .replace("{actual}", &(actual / 1_000_000).to_string())
            .replace("{limit}", &(limit / 1_000_000).to_string());
        }
        DownloadError::NotFound => pick(
            lang,
            "Video not found — it may have been deleted.",
            "Відео не знайдено — можливо, його видалили.",
        ),
        DownloadError::Playlist => pick(
            lang,
            "Playlists and channels are not supported yet. Send a single video link.",
            "Плейлисти й канали поки не підтримуються. Надішліть посилання на одне відео.",
        ),
        DownloadError::SourceDisabled(source) => {
            return format!(
                "{}.",
                pick(
                    lang,
                    "Source {source} is disabled",
                    "Джерело {source} вимкнено",
                )
                .replace("{source}", source.display_name())
            );
        }
        DownloadError::Timeout => pick(
            lang,
            "The source took too long to respond. Please try again later.",
            "Джерело занадто довго не відповідає. Спробуйте пізніше.",
        ),
        DownloadError::SourceUnavailable(reason) => {
            return format!(
                "{}: {}",
                pick(
                    lang,
                    "The source is temporarily unavailable",
                    "Джерело тимчасово недоступне",
                ),
                escape_html(reason)
            );
        }
        DownloadError::Unsupported(_) | DownloadError::Media(_) | DownloadError::Internal(_) => {
            pick(
                lang,
                "Something went wrong while processing this video. Please try again.",
                "Під час обробки відео сталася помилка. Спробуйте ще раз.",
            )
        }
    };
    format!("{text}.")
}

pub fn stats(lang: Lang, value: &Stats) -> String {
    format!(
        "{}\n📹 {}\n👥 {}\n📤 {}",
        pick(lang, "<b>Stats</b>", "<b>Статистика</b>"),
        value.videos,
        value.users,
        value.sends
    )
}

pub fn banned(lang: Lang) -> &'static str {
    pick(lang, "User banned.", "Користувача забанено.")
}

pub fn unbanned(lang: Lang) -> &'static str {
    pick(lang, "User unbanned.", "Бан знято.")
}

pub fn purged(lang: Lang) -> &'static str {
    pick(lang, "Cache entry removed.", "Запис кешу видалено.")
}

pub fn not_found_key(lang: Lang) -> &'static str {
    pick(lang, "Cache key not found.", "Ключ кешу не знайдено.")
}

pub fn invalid_link(lang: Lang) -> &'static str {
    pick(
        lang,
        "I couldn't recognize this link. Supported: YouTube, TikTok, Instagram Reels, X, Reddit, Vimeo.",
        "Не вдалося розпізнати це посилання. Підтримуються: YouTube, TikTok, Instagram Reels, X, Reddit, Vimeo.",
    )
}

pub fn rate_limited(lang: Lang) -> &'static str {
    pick(
        lang,
        "Too many requests. Please slow down a bit.",
        "Занадто багато запитів. Трохи зачекайте.",
    )
}

pub fn overloaded(lang: Lang) -> &'static str {
    pick(
        lang,
        "The bot is busy right now. Please try again in a minute.",
        "Бот зараз перевантажений. Спробуйте за хвилину.",
    )
}

pub fn queue_position(lang: Lang, position: usize) -> String {
    pick(
        lang,
        "Your video is queued (#{position}).",
        "Ваше відео в черзі (#{position}).",
    )
    .replace("{position}", &position.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_escapes_all_special_chars() {
        assert_eq!(
            escape_html("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn caption_builds_three_lines() {
        let text = caption("A & B", Some("Chan"), "https://x/y");
        assert!(text.contains("A &amp; B"));
        assert!(text.contains("Chan"));
        assert!(text.contains("https://x/y"));
    }

    #[test]
    fn too_long_message_interpolates() {
        let msg = error_message(
            Lang::En,
            &DownloadError::TooLong {
                limit: 900,
                actual: 1000,
            },
        );
        assert!(msg.contains("1000s") && msg.contains("900s"));
    }
}
