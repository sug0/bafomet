mod common;

use std::time::Duration;

use bafomet::bft::async_runtime as rt;
use bafomet::bft::communication::channel;
use bafomet::bft::communication::message::Message;
use bafomet::bft::executable::{Reply, Request, State};
use bafomet::bft::timeouts::{TimeoutKind, Timeouts};
use bafomet::bft::{init, InitConfig};

type Sv = common::CalcService;

type S = State<Sv>;
type O = Request<Sv>;
type P = Reply<Sv>;

fn main() {
    let conf = InitConfig {
        async_threads: num_cpus::get(),
    };
    let _guard = unsafe { init(conf).unwrap() };
    rt::block_on(async_main());
}

async fn async_main() {
    let (tx, mut rx) = channel::new_message_channel::<S, O, P>(8);
    let timeouts = Timeouts::<Sv>::new(tx);

    for i in 1..=5 {
        println!("Created timeout of {} seconds", i * 5);
        let dur = Duration::from_secs(i * 5);
        timeouts.timeout(dur, TimeoutKind::Cst(0));
    }

    while let Ok(message) = rx.recv().await {
        if let Message::Timeout(_) = message {
            println!("Received a timeout!");
        }
    }
}
