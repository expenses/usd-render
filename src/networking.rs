//use crate::ALPN;
use futures_util::future::Shared;
use futures_util::FutureExt;
use futures_util::StreamExt;
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::state::TopicId;
use iroh_net::key::PublicKey;
use iroh_net::magic_endpoint::AddrInfo;
use iroh_net::magic_endpoint::{accept_conn, MagicEndpoint};
use iroh_net::NodeAddr;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::collections::{hash_map::Entry, HashMap};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::oneshot;

pub type ApprovedNodes = Arc<tokio::sync::RwLock<HashSet<PublicKey>>>;

#[derive(Clone)]
pub struct GossipLayer {
    gossip: Gossip,
    endpoint: MagicEndpoint,
    node_addr_timestamps: Arc<tokio::sync::Mutex<HashMap<PublicKey, u64>>>,
}

impl GossipLayer {
    pub async fn new(endpoint: MagicEndpoint) -> anyhow::Result<Self> {
        let addr = endpoint.my_addr().await?;

        let gossip = iroh_gossip::net::Gossip::from_endpoint(
            endpoint.clone(),
            Default::default(),
            &addr.info,
        );

        let join_topic_fut = gossip.join(TOPIC_ID, vec![]).await?;

        tokio::spawn(async move {
            if let Err(err) = join_topic_fut.await {
                log::error!("{}", err);
            }
        });

        let gossip_layer = Self {
            gossip,
            endpoint,
            node_addr_timestamps: Default::default(),
        };

        tokio::spawn({
            let gossip_layer = gossip_layer.clone();
            async move {
                let mut events = gossip_layer.gossip.subscribe(TOPIC_ID).await.unwrap();

                while let Ok(item) = events.recv().await {
                    if let iroh_gossip::net::Event::Received(
                        iroh_gossip::proto::topic::GossipEvent { content, .. },
                    ) = item
                    {
                        if let Err(err) = gossip_layer.handle_gossip_message(&content).await {
                            dbg!(err);
                        }
                    }
                }
            }
        });

        Ok(gossip_layer)
    }

    async fn handle_gossip_message(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let message = TimestampedMessage::verify(bytes)?;

        dbg!(&message);

        self.add_node_addr(message.timestamp, message.message)
            .await?;

        Ok(())
    }

    async fn add_node_addr(&self, timestamp: u64, node_addr: NodeAddr) -> anyhow::Result<()> {
        let updated_timestamp = {
            let mut node_addr_timestamps = self.node_addr_timestamps.lock().await;

            let new = match node_addr_timestamps.entry(node_addr.node_id) {
                Entry::Vacant(vacancy) => {
                    vacancy.insert(timestamp);
                    true
                }
                Entry::Occupied(mut occupancy) => {
                    if timestamp > *occupancy.get() {
                        occupancy.insert(timestamp);
                        true
                    } else {
                        false
                    }
                }
            };

            dbg!(&node_addr_timestamps);

            new
        };

        if updated_timestamp {
            self.endpoint.add_node_addr(node_addr)?;
        } else {
            dbg!("!!!!");
        }

        Ok(())
    }

    pub async fn connect(&self, node_addr: NodeAddr) -> anyhow::Result<()> {
        // Allow for a bit of latency.
        self.add_node_addr(get_timestamp().saturating_sub(1000), node_addr.clone())
            .await?;

        self.gossip
            .join(TOPIC_ID, vec![node_addr.node_id])
            .await?
            .await?;

        let my_addr = self.endpoint.my_addr().await?;
        self.gossip
            .broadcast(
                TOPIC_ID,
                TimestampedMessage::new(my_addr).sign(self.endpoint.secret_key())?,
            )
            .await?;

        Ok(())
    }
}

enum PacketType {
    Data = 0,
    NewNode = 1,
}

impl PacketType {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Data,
            1 => Self::NewNode,
            _ => return None,
        })
    }
}

pub const TOPIC_ID: TopicId = TopicId::from_bytes([69; 32]);

fn get_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("time drift")
        .as_micros() as u64
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct TimestampedMessage<T> {
    message: T,
    timestamp: u64,
}

impl<T: serde::Serialize + serde::de::DeserializeOwned> TimestampedMessage<T> {
    fn new(message: T) -> Self {
        Self {
            message,
            timestamp: get_timestamp(),
        }
    }

    fn sign(&self, key: &iroh_net::key::SecretKey) -> anyhow::Result<bytes::Bytes> {
        let message_bytes = postcard::to_stdvec(&self)?;
        let mut bytes = bytes::BytesMut::from(&key.sign(&message_bytes).to_bytes()[..]);
        bytes.extend_from_slice(&message_bytes);
        Ok(bytes.into())
    }
}

impl TimestampedMessage<NodeAddr> {
    pub fn verify(bytes: &[u8]) -> anyhow::Result<Self> {
        let message_bytes = bytes
            .get(64..)
            .ok_or_else(|| anyhow::anyhow!("Message less than 64 bytes long ({})", bytes.len()))?;
        let signature =
            iroh_net::key::Signature::from_slice(bytes.get(..64).ok_or_else(|| {
                anyhow::anyhow!("Message less than 64 bytes long ({})", bytes.len())
            })?)?;

        let timestamped_message: Self = postcard::from_bytes(message_bytes)?;

        timestamped_message
            .message
            .node_id
            .verify(&message_bytes, &signature)?;

        Ok(timestamped_message)
    }
}

pub async fn accept(
    connecting: quinn::Connecting,
    gossip_layer: GossipLayer,
    approved_nodes: ApprovedNodes,
    approval_queue: tokio::sync::mpsc::Sender<(PublicKey, oneshot::Sender<bool>)>,
) {
    let (node_id, alpn, connection) = match accept_conn(connecting).await {
        Ok(data) => data,
        Err(error) => {
            log::error!("Error accepting incoming connection: {}", error);
            return;
        }
    };

    if !approved_nodes.read().await.contains(&node_id) {
        let (tx, rx) = oneshot::channel();
        approval_queue.send((node_id, tx)).await.unwrap();

        let approved = rx.await.unwrap();

        if approved {
            approved_nodes.write().await.insert(node_id);
        } else {
            log::info!("Denying {}", node_id);
            return;
        }
    }

    if alpn.as_bytes() == iroh_gossip::net::GOSSIP_ALPN {
        if let Err(err) = gossip_layer.gossip.handle_connection(connection).await {
            log::error!("{}", err);
        }
    }

    /*

    let mut addr_info = AddrInfo::default();
    addr_info.direct_addresses.insert(connection.remote_address());

    check_if_already_connected(connection.clone(), &connected_nodes, node_id, addr_info).await;

    handle_connection(connection, node_id, connected_nodes).await*/
}
/*
pub async fn connect(
    endpoint: iroh_net::MagicEndpoint,
    addr: iroh_net::NodeAddr,
    connected_nodes: ConnectedNodes,
) {
    if connected_nodes.lock().await.contains_key(&addr.node_id) {
        log::warn!("Not connecting to {}: already connected", addr.node_id);
        return;
    }

    let node_id = addr.node_id;

    let connection = match endpoint.connect(addr.clone(), ALPN).await {
        Ok(connection) => connection,
        Err(error) => {
            log::error!("Connecting to {} failed: {}", node_id, error);
            return;
        }
    };

    check_if_already_connected(connection.clone(), &connected_nodes, node_id, addr.info).await;

    handle_connection(connection, node_id, connected_nodes).await
}

async fn check_if_already_connected(
    connection: quinn::Connection,
    connected_nodes: &ConnectedNodes,
    node_id: PublicKey,
    addr_info: AddrInfo
) {
    let other = {
        let mut connected_nodes = connected_nodes.lock().await;

        if connected_nodes.contains_key(&node_id) {
            log::warn!("Not connecting to {}: already connected", node_id);
            return;
        }

        let other_node_addrs = connected_nodes.iter().map(|(node_id, info)| {
            NodeAddr {
                node_id: *node_id,
                info: info.clone()
            }
        }).collect::<Vec<_>>();

        connected_nodes.insert(node_id, addr_info);

        other_node_addrs
    };


    dbg!(&other);

    // todo: send new node packet.
}

async fn handle_outgoing(connection: quinn::Connection) -> anyhow::Result<()> {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let mut stream = connection.open_uni().await?;
        stream.write_all(&[PacketType::Data as u8]).await?;
        stream.write_all(b"hello!").await?;
    }
}

async fn handle_connection(
    connection: quinn::Connection,
    node_id: PublicKey,
    connected_nodes: ConnectedNodes,
) {
    let incoming = tokio::spawn({
        let connection = connection.clone();
        async move {
            if let Err(error) = handle_incoming(connection).await {
                log::error!("Error when handling incoming from {}: {}", node_id, error);
            }
        }
    });

    let outgoing = tokio::spawn(async move {
        if let Err(error) = handle_outgoing(connection).await {
            log::error!("Error when sending outgoing to {}: {}", node_id, error);
        }
    });

    let _ = incoming.await;
    let _ = outgoing.await;

    connected_nodes.lock().await.remove(&node_id);
}

async fn handle_incoming(connection: quinn::Connection) -> anyhow::Result<()> {
    loop {
        let mut stream = connection.accept_uni().await?;
        let ty = {
            let mut ty_byte = [0_u8];
            stream.read_exact(&mut ty_byte).await?;
            PacketType::from_byte(ty_byte[0])
                .ok_or_else(|| anyhow::anyhow!("Got invalid packet byte: {}", ty_byte[0]))?
        };
        match ty {
            PacketType::Data => {
                let data = stream.read_to_end(1024 * 1024).await?;
                dbg!(data);
            }
            PacketType::NewNode => {
                dbg!(());
            }
        }
    }
}
*/
