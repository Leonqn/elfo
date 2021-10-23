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
use tokio::{runtime::Handle, task::JoinHandle};
use tracing::{debug, error, warn};
use warp::{
    sse::{self},
    Filter,
};

use elfo::{scope, stream::Stream as ElfoStream, trace_id, Context, Local, Message};
use elfo_core as elfo;

use crate::{
    api::{Update, UpdateResult},
    protocol::{Config, Request, RequestBody, ServerFailed, Token},
    values::UpdateError,
};

#[derive(Clone)]
struct InspectorServer {
    config: Config,
    ctx: Context,
}

const CHANNEL_CAPACITY: usize = 32;

pub(crate) fn start(ctx: &Context<Config>) -> JoinHandle<()> {
    let ctx = ctx.clone();
    let ctx1 = ctx.clone();
    let scope = scope::expose();
    let scope1 = scope.clone();
    let config = ctx.config();
    let binding = (config.ip, config.port);
    let routes = warp::path!("api" / "v1" / "topology")
        .and(warp::get())
        //.and(auth_token)
        .map(move || {
            let ctx = ctx1.clone();
            // let scope = scope.clone();
            debug!("request");

            let notify = move || request(ctx.pruned(), RequestBody::GetTopology);

            // let output = ctx.request();
            // let response = scope.sync_within(notify);
            warp::sse::reply(notify())
        });

    tokio::spawn(async move {
        warp::serve(routes).run(binding).await;
        let scope = scope1.clone();
        let report = async {
            let _ = ctx.send(ServerFailed).await;
        };

        scope.set_trace_id(trace_id::generate());
        scope.within(report).await
    })
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
