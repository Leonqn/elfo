use std::{convert::AsRef, pin::Pin, task::Poll};

use futures::{Future, FutureExt, Stream, StreamExt};
use futures_intrusive::{
    buffer::GrowingHeapBuf,
    channel::{
        shared::{channel, Receiver, Sender, SharedStream},
        ChannelStream,
    },
};
use parking_lot::RawMutex;
use tokio::{runtime::Handle, spawn, task::JoinHandle};
use tracing::{debug, error, warn};
use warp::{
    sse::{self},
    Filter,
};

use elfo::{stream::Stream as ElfoStream, Context, Message};
use elfo_core as elfo;

use crate::{
    api::{Update, UpdateResult},
    protocol::{Config, Request, RequestBody, Token},
    values::UpdateError,
};

#[derive(Clone)]
pub(crate) struct InspectorServer {
    config: Config,
    ctx: Context,
}

const CHANNEL_CAPACITY: usize = 32;

impl InspectorServer {
    pub(crate) fn new(
        config: &Config,
        ctx: Context,
    ) -> (
        SharedStream<RawMutex, Request, GrowingHeapBuf<Request>>,
        JoinHandle<()>,
        Self,
    ) {
        let (sender, receiver) = channel::<Request>(CHANNEL_CAPACITY);
        let server_ctx = ctx.clone();
        let routes = warp::path!("api" / "v1" / "topology")
            .and(warp::get())
            //.and(auth_token)
            .map(move || {
                debug!("request");
                warp::sse::reply(request(
                    server_ctx.clone(),
                    //"".clone().into(),
                    RequestBody::GetTopology,
                ))
            });
        let execution = warp::serve(routes).run((config.ip, config.port));
        (
            receiver.into_stream(),
            spawn(execution),
            Self {
                config: config.clone(),
                ctx,
            },
        )
    }
}

fn request(
    ctx: Context,
    // auth_token: Token,
    body: RequestBody,
) -> impl Stream<Item = Result<sse::Event, UpdateError>> {
    let (req, rx): (Request, Receiver<UpdateResult>) = Request::new(body);
    let tx = req.tx().clone();
    if ctx.try_send_to(ctx.addr(), req).is_err() {
        if let Err(err) = Handle::current().block_on(tx.send(Err(UpdateError::TooManyRequests))) {
            error!(?err, "can't send error update");
        }
    }

    rx.into_stream()
        .map(move |update_result| -> Result<sse::Event, UpdateError> {
            update_result
                .and_then(|update| {
                    let event = sse::Event::default().event(update.as_ref());
                    if matches!(update, Update::Heartbeat) {
                        Ok(event)
                    } else {
                        event.json_data(update).map_err(UpdateError::from)
                    }
                })
                .map_err(|err| {
                    warn!(?err, "can't handle connection");
                    err
                })
        })
}
