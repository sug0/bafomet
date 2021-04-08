mod common;

use common::*;

use std::time::Duration;
use std::collections::HashMap;

use febft::bft::threadpool;
use febft::bft::communication::NodeId;
use febft::bft::async_runtime as rt;
use febft::bft::{
    init,
    InitConfig,
};
use febft::bft::communication::message::{
    SystemMessage,
    RequestMessage,
};
use febft::bft::crypto::signature::{
    KeyPair,
    PublicKey,
};

fn main() {
    let arg = std::env::args_os()
        .skip(1)
        .next()
        .unwrap();
    let id: u32 = arg
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let conf = InitConfig {
        async_threads: 40,
    };
    let _guard = unsafe { init(conf).unwrap() };
    rt::block_on(async_main(NodeId::from(id)));
}

macro_rules! ip {
    ($self:expr, $peer:expr) => {
        if $self == $peer {
            let i = u32::from($self);
            format!("0.0.0.0:{}", 10000 + i)
        } else {
            let i = u32::from($peer);
            format!("192.168.70.{}:{}", 16 + i, 10000 + i)
        }
    }
}

async fn async_main(id: NodeId) {
    let sk = KeyPair::from_bytes(&SECRET_KEYS[usize::from(id)][..]).unwrap();
    let public_keys: HashMap<NodeId, PublicKey> = SECRET_KEYS
        .iter()
        .map(|sk| KeyPair::from_bytes(&sk[..]).unwrap().public_key().into())
        .enumerate()
        .map(|(id, sk)| (NodeId::from(id), sk))
        .collect();

    let pool = threadpool::Builder::new()
        .num_threads(4)
        .build();

    let mut node = {
        let peers: Vec<_> = NodeId::targets(0..4).collect();
        let addrs= map! {
            peers[0] => addr!("cop01" => ip!(id, peers[0])),
            peers[1] => addr!("cop02" => ip!(id, peers[1])),
            peers[2] => addr!("cop03" => ip!(id, peers[2])),
            peers[3] => addr!("cop04" => ip!(id, peers[3]))
        };
        let fut = setup_node(
            pool,
            id,
            sk,
            addrs,
            public_keys,
            Duration::from_secs(1),
        );
        println!("Bootstrapping node #{}", usize::from(id));
        let (node, rogue) = fut.await.unwrap();
        println!("Spawned node #{}; len(rogue) => {}", usize::from(node.id()), rogue.len());
        node
    };

    // broadcast message
    let m = SystemMessage::Request(RequestMessage::new(()));
    node.broadcast(m, NodeId::targets(0..4));

    // receive peer messages
    for _ in 0..4 {
        let _m = node.receive();
        println!("Node #{} received message", usize::from(id));
    }
}

const KEY_SIZE: usize = 32;

pub static SECRET_KEYS: [[u8; KEY_SIZE]; 4] = [
    [1; KEY_SIZE], [2; KEY_SIZE], [3; KEY_SIZE], [4; KEY_SIZE],
];