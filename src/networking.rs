use crate::ALPN;
use futures_util::StreamExt;
use iroh_net::key::PublicKey;
use iroh_net::magic_endpoint::AddrInfo;
use iroh_net::magic_endpoint::{accept_conn, MagicEndpoint};
use iroh_net::NodeAddr;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::sync::watch;

pub type ApprovedNodes = Arc<tokio::sync::RwLock<HashMap<PublicKey, NodeSharingPolicy>>>;
pub type ConnectedNodes = Arc<tokio::sync::Mutex<HashSet<PublicKey>>>;
pub type ApprovalQueue = tokio::sync::mpsc::Sender<NodeApprovalRequest>;

pub struct NodeApprovalRequest {
    pub node_id: PublicKey,
    pub direction: NodeApprovalDirection,
    pub response_sender: oneshot::Sender<NodeApprovalResponse>,
}

pub enum NodeApprovalDirection {
    Incoming,
    Outgoing { referrer: PublicKey },
}

pub enum NodeApprovalResponse {
    Approved(NodeSharingPolicy),
    Denied,
}

// Who should a node's addrinfo be shared with?
#[derive(Clone)]
pub enum NodeSharingPolicy {
    AllExcept(HashSet<PublicKey>),
    NoneExcept(HashSet<PublicKey>),
}

impl NodeSharingPolicy {
    fn allows(&self, node_id: PublicKey) -> bool {
        match self {
            Self::AllExcept(all_except) => !all_except.contains(&node_id),
            Self::NoneExcept(none_except) => none_except.contains(&node_id),
        }
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

#[derive(Clone)]
pub struct State {
    pub endpoint: MagicEndpoint,
    pub approved_nodes: ApprovedNodes,
    pub approval_queue: ApprovalQueue,
    pub connected_nodes: ConnectedNodes,
    pub state: watch::Receiver<String>,
}

pub async fn accept(connecting: quinn::Connecting, state: State) {
    let (node_id, _alpn, connection) = match accept_conn(connecting).await {
        Ok(data) => data,
        Err(error) => {
            log::error!("Error accepting incoming connection: {}", error);
            return;
        }
    };

    match wait_for_approval(state.clone(), node_id, NodeApprovalDirection::Incoming).await {
        Ok(true) => {}
        Err(error) => {
            log::error!("{}", error);
            connection.close(0_u32.into(), b"error");
            return;
        }
        Ok(false) => {
            log::info!("Denied connection to {}", node_id.fmt_short());
            connection.close(0_u32.into(), b"denied");
            return;
        }
    }

    handle_connection(state, connection, node_id).await;
}

async fn wait_for_approval(
    state: State,
    node_id: PublicKey,
    direction: NodeApprovalDirection,
) -> anyhow::Result<bool> {
    if !state.approved_nodes.read().await.contains_key(&node_id) {
        let (tx, rx) = oneshot::channel();
        state
            .approval_queue
            .send(NodeApprovalRequest {
                node_id,
                direction,
                response_sender: tx,
            })
            .await?;

        let node_sharing = match rx.await? {
            NodeApprovalResponse::Approved(node_sharing) => node_sharing,
            NodeApprovalResponse::Denied => return Ok(false),
        };

        state
            .approved_nodes
            .write()
            .await
            .insert(node_id, node_sharing);
    }

    Ok(true)
}

pub async fn connect(state: State, addr: NodeAddr, referrer: Option<PublicKey>) {
    if let Some(referrer) = referrer {
        match wait_for_approval(
            state.clone(),
            addr.node_id,
            NodeApprovalDirection::Outgoing { referrer },
        )
        .await
        {
            Ok(true) => {}
            Err(error) => {
                log::error!("{}", error);
                return;
            }
            Ok(false) => {
                log::info!("Denied connection to {}", addr.node_id.fmt_short());
                return;
            }
        }
    }

    if state.connected_nodes.lock().await.contains(&addr.node_id) {
        log::warn!("Not connecting to {}: already connected", addr.node_id);
        return;
    }

    let node_id = addr.node_id;

    let connection = match state.endpoint.connect(addr, ALPN).await {
        Ok(connection) => connection,
        Err(error) => {
            log::error!("Connecting to {} failed: {}", node_id, error);
            return;
        }
    };

    handle_connection(state, connection, node_id).await;
}

async fn handle_connection(
    state: State,
    connection: quinn::Connection,
    connection_node_id: PublicKey,
) {
    let mut third_parties = Vec::new();

    {
        let mut connected_nodes = state.connected_nodes.lock().await;

        if connected_nodes.contains(&connection_node_id) {
            log::info!(
                "Ending new connection to {}: already connected",
                connection_node_id
            );
            connection.close(0_u32.into(), b"already connected");
            return;
        }

        for &existing_node_id in connected_nodes.iter() {
            match state.approved_nodes.read().await.get(&existing_node_id) {
                Some(sharing_policy) => {
                    if !sharing_policy.allows(connection_node_id) {
                        log::info!("Not sharing {} to {}", existing_node_id, connection_node_id);
                        continue;
                    }
                }
                None => {
                    log::error!("Node {} connected but not allowed.", existing_node_id);
                }
            }

            let connection_info = match state.endpoint.connection_info(existing_node_id).await {
                Err(error) => {
                    log::error!(
                        "Error getting connection info for {}: {}",
                        existing_node_id,
                        error
                    );
                    continue;
                }
                Ok(None) => {
                    log::error!("No connection info for {} found.", existing_node_id);
                    continue;
                }
                Ok(Some(info)) => info,
            };

            third_parties.push(NodeAddr {
                node_id: existing_node_id,
                info: AddrInfo {
                    derp_url: connection_info.derp_url,
                    direct_addresses: connection_info.addrs.iter().map(|addr| addr.addr).collect(),
                },
            });
        }

        connected_nodes.insert(connection_node_id);
    }

    log::info!("Sending {:?} to {}", third_parties, connection_node_id);

    let send_initial_third_parties = tokio::spawn({
        let connection = connection.clone();
        async move {
            if let Err(error) = send_third_parties(connection, third_parties).await {
                log::error!("{}", error);
            }
        }
    });

    let incoming = tokio::spawn({
        let connection = connection.clone();
        let state = state.clone();
        async move {
            if let Err(error) = handle_incoming(state, connection_node_id, connection).await {
                log::error!("{}", error);
            }
        }
    });

    let outgoing = tokio::spawn({
        async move {
            if let Err(error) = handle_outgoing(connection, state).await {
                log::error!("{}", error);
            }
        }
    });

    let _ = send_initial_third_parties.await;
    let _ = incoming.await;
    let _ = outgoing.await;
}

async fn send_third_parties(
    connection: quinn::Connection,
    third_parties: Vec<NodeAddr>,
) -> anyhow::Result<()> {
    if third_parties.is_empty() {
        return Ok(());
    }

    let mut stream = connection.open_uni().await?;

    stream.write_all(&[PacketType::NewNode as u8]).await?;

    let third_parties = postcard::to_stdvec(&third_parties)?;

    stream.write_all(&third_parties).await?;

    Ok(())
}

async fn handle_outgoing(connection: quinn::Connection, state: State) -> anyhow::Result<()> {
    let mut state_stream = tokio_stream::wrappers::WatchStream::from_changes(state.state);
    while let Some(state) = state_stream.next().await {
        let mut stream = connection.open_uni().await?;
        stream.write_all(&[PacketType::Data as u8]).await?;
        stream.write_all(state.as_bytes()).await?;
    }
    Ok(())
}

async fn handle_incoming(
    state: State,
    node_id: PublicKey,
    connection: quinn::Connection,
) -> anyhow::Result<()> {
    loop {
        let mut stream = connection.accept_uni().await?;
        let ty = {
            let mut ty_byte = 0_u8;
            stream
                .read_exact(std::slice::from_mut(&mut ty_byte))
                .await?;
            PacketType::from_byte(ty_byte)
                .ok_or_else(|| anyhow::anyhow!("Got invalid packet byte: {}", ty_byte))?
        };
        match ty {
            PacketType::Data => {
                let data = stream.read_to_end(1024 * 1024).await?;
                log::info!(
                    "Got {:?} bytes from {}",
                    std::str::from_utf8(&data),
                    node_id
                );
            }
            PacketType::NewNode => {
                let data = stream.read_to_end(1024 * 1024).await?;
                let third_parties: Vec<NodeAddr> = postcard::from_bytes(&data)?;
                for node_addr in third_parties.into_iter() {
                    fn spawn_connect(state: State, node_addr: NodeAddr, referrer: PublicKey) {
                        tokio::spawn(async move {
                            connect(state, node_addr, Some(referrer)).await;
                        });
                    }

                    spawn_connect(state.clone(), node_addr, node_id);
                }
            }
        }
    }
}
