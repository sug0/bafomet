mod common;

use common::*;

use std::time::Duration;

use futures_timer::Delay;
use rand_core::{OsRng, RngCore};

use bafomet::bft::async_runtime as rt;
use bafomet::bft::collections::HashMap;
use bafomet::bft::communication::message::{RequestMessage, SystemMessage};
use bafomet::bft::communication::NodeId;
use bafomet::bft::crypto::signature::{KeyPair, PublicKey};
use bafomet::bft::threadpool;
use bafomet::bft::{init, InitConfig};

fn main() {
    let conf = InitConfig {
        async_threads: num_cpus::get(),
    };
    let _guard = unsafe { init(conf).unwrap() };
    rt::block_on(async_main());
}

async fn async_main() {
    let mut secret_keys: HashMap<NodeId, KeyPair> = sk_stream()
        .take(4)
        .enumerate()
        .map(|(id, sk)| (NodeId::from(id), sk))
        .collect();
    let public_keys: HashMap<NodeId, PublicKey> = secret_keys
        .iter()
        .map(|(id, sk)| (*id, sk.public_key().into()))
        .collect();

    let pool = threadpool::Builder::new().num_threads(4).build();

    for id in NodeId::targets(0..4) {
        let addrs = map! {
            NodeId::from(0u32) => addr!("cop01" => "127.0.0.1:10001"),
            NodeId::from(1u32) => addr!("cop02" => "127.0.0.1:10002"),
            NodeId::from(2u32) => addr!("cop03" => "127.0.0.1:10003"),
            NodeId::from(3u32) => addr!("cop04" => "127.0.0.1:10004")
        };
        let sk = secret_keys.remove(&id).unwrap();
        let fut = setup_node(pool.clone(), id, sk, addrs, public_keys.clone());
        rt::spawn(async move {
            println!("Bootstrapping node #{}", u32::from(id));
            let (mut node, rogue) = fut.await.unwrap();
            println!("Spawned node #{}", u32::from(id));
            println!("Rogue on node #{} => {}", u32::from(id), debug_rogue(rogue));
            let m = SystemMessage::Request(RequestMessage::new(Action::Sqrt));
            node.broadcast(m, NodeId::targets(0..4));
            for _ in 0..4 {
                let m = node.receive().await.unwrap();
                let peer: u32 = m
                    .header()
                    .expect(&format!("on node {}", u32::from(id)))
                    .from()
                    .into();
                println!(
                    "Node #{} received message {} from #{}",
                    u32::from(id),
                    debug_msg(m),
                    peer
                );
            }
            // avoid early drop of node
            rt::spawn(async move {
                let _node = node;
                let () = std::future::pending().await;
            });
        });
    }
    drop(pool);

    // wait 3 seconds then exit
    Delay::new(Duration::from_secs(3)).await;
}

fn sk_stream() -> impl Iterator<Item = KeyPair> {
    std::iter::repeat_with(|| {
        // only valid for ed25519!
        let mut buf = [0; 32];

        // gen key
        OsRng.fill_bytes(&mut buf[..]);
        KeyPair::from_bytes(&buf[..]).unwrap()
    })
}
