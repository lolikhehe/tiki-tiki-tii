//! Telegram video upload helpers.

use common::domain::{CachedVideo, PreparedVideo};
use teloxide::prelude::*;
use teloxide::types::{FileId, InlineKeyboardMarkup, InputFile, Message, ParseMode, ReplyMarkup};

pub async fn send_html(
    bot: &Bot,
    chat: ChatId,
    text: impl Into<String>,
) -> Result<Message, teloxide::RequestError> {
    bot.send_message(chat, text)
        .parse_mode(ParseMode::Html)
        .await
}

pub async fn send_cached(
    bot: &Bot,
    chat: ChatId,
    video: &CachedVideo,
    caption: String,
    reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<Message, teloxide::RequestError> {
    let mut request = bot
        .send_video(chat, InputFile::file_id(FileId(video.file_id.clone())))
        .caption(caption)
        .parse_mode(ParseMode::Html)
        .duration(video.duration_sec)
        .width(video.width)
        .height(video.height)
        .supports_streaming(true);
    if let Some(markup) = reply_markup {
        request = request.reply_markup(ReplyMarkup::InlineKeyboard(markup));
    }
    request.await
}

pub async fn send_prepared(
    bot: &Bot,
    chat: ChatId,
    prepared: &PreparedVideo,
    caption: String,
    reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<Message, teloxide::RequestError> {
    let input = InputFile::file(prepared.path.clone()).file_name("video.mp4");
    let mut request = bot
        .send_video(chat, input)
        .caption(caption)
        .parse_mode(ParseMode::Html)
        .duration(prepared.duration_sec)
        .width(prepared.width)
        .height(prepared.height)
        .supports_streaming(true);
    if let Some(path) = &prepared.thumbnail {
        request = request.thumbnail(InputFile::file(path.clone()));
    }
    if let Some(markup) = reply_markup {
        request = request.reply_markup(ReplyMarkup::InlineKeyboard(markup));
    }
    request.await
}

pub fn extract_file_id(message: &Message) -> Option<(String, String)> {
    message
        .video()
        .map(|video| (video.file.id.0.clone(), video.file.unique_id.0.clone()))
}

pub fn is_invalid_file_id(error: &teloxide::RequestError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("file_id")
        || text.contains("file identifier")
        || text.contains("wrong file")
        || text.contains("invalid file")
}

pub fn is_message_id_invalid(error: &teloxide::RequestError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("message_id_invalid") || text.contains("message id invalid")
}
