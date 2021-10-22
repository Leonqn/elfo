#![warn(rust_2018_idioms, unreachable_pub)]

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use futures::{FutureExt, StreamExt};
use futures_intrusive::{
    buffer::GrowingHeapBuf,
    channel::{
        shared::{channel, Receiver, Sender, SharedStream},
        ChannelStream,
    },
};
use priority_queue::PriorityQueue;
use slotmap::{new_key_type, SlotMap};
use tracing::error;

use elfo::{stream::Stream, time::Stopwatch, ActorGroup, Context, Schema, Topology};
use elfo_core as elfo;
use elfo_macros::msg_raw as msg;

mod actors;
mod api;
mod channel;
mod protocol;
mod values;

use crate::protocol::Config;
use actors::Inspector;
use api::{Update, UpdateKey, UpdateResult};

pub fn new(topology: Topology) -> Schema {
    let topology = Arc::new(topology);
    ActorGroup::new().config::<Config>().exec(move |ctx| {
        let topology = topology.as_ref().clone();
        let inspector = Inspector::new(&ctx, topology);
        inspector.main(ctx)
    })
}
