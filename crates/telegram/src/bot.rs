//! Dispatcher setup: manual update routing, polling/webhook switch, and
//! graceful shutdown together with the background job worker.

use crate::{handlers, inline, state::AppState, worker};
use common::error::HandlerResult;
use common::shutdown;
use std::sync::Arc;
use teloxide::dispatching::Dispatcher;
use teloxide::error_handlers::IgnoringErrorHandler;
use teloxide::prelude::*;
use teloxide::types::{Update, UpdateKind};
use teloxide::update_listeners::webhooks::Options;
use tracing::{error, info, warn};

async fn handle_update(bot: Bot, update: Update, state: AppState) -> HandlerResult {
    match update.kind {
        UpdateKind::Message(message) => handlers::handle_message(state, bot, message).await,
        UpdateKind::CallbackQuery(query) => {
            crate::callbacks::handle_callback(state, bot, query).await
        }
        UpdateKind::InlineQuery(query) => inline::handle_inline(state, bot, query).await,
        UpdateKind::ChosenInlineResult(result) => inline::handle_chosen(state, bot, result).await,
        _ => Ok(()),
    }
}

pub async fn run(bot: Bot, state: AppState) -> anyhow::Result<()> {
    worker::spawn_worker(state.clone(), bot.clone());

    let handler = teloxide::dptree::entry().endpoint(handle_update);
    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(teloxide::dptree::deps![state.clone()])
        .build();
    let shutdown_token = dispatcher.shutdown_token();

    let config = state.config.clone();
    let mut dispatcher_task = tokio::spawn(async move {
        if let Some(webhook_url) = &config.webhook_url {
            info!(%webhook_url, "starting webhook listener");
            let address = format!("0.0.0.0:{}", config.webhook_port)
                .parse()
                .expect("valid webhook address");
            let public_url = url::Url::parse(webhook_url).expect("valid WEBHOOK_URL");
            let options = Options::new(address, public_url);
            match teloxide::update_listeners::webhooks::axum(bot.clone(), options).await {
                Ok(listener) => {
                    dispatcher
                        .dispatch_with_listener(listener, Arc::new(IgnoringErrorHandler))
                        .await;
                }
                Err(error) => error!(%error, "could not set up webhook"),
            }
        } else {
            info!("starting polling listener");
            dispatcher.dispatch().await;
        }
    });

    shutdown::wait_for_shutdown().await;
    info!("shutdown requested");
    match shutdown_token.shutdown() {
        Ok(future) => future.await,
        Err(error) => warn!(%error, "dispatcher was already idle"),
    }
    match tokio::time::timeout(std::time::Duration::from_secs(20), &mut dispatcher_task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if !error.is_cancelled() => error!(%error, "dispatcher task failed"),
        Ok(Err(_)) => {}
        Err(_) => {
            warn!("dispatcher did not stop in time; aborting");
            dispatcher_task.abort();
            let _ = dispatcher_task.await;
        }
    }
    Ok(())
}
