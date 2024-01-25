use crate::ALPN;
use futures_util::StreamExt;
use iroh_net::key::PublicKey;
use iroh_net::magic_endpoint::AddrInfo;
use iroh_net::magic_endpoint::{accept_conn, MagicEndpoint};
use iroh_net::NodeAddr;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::sync::watch;

pub type ApprovedNodes = Arc<tokio::sync::RwLock<HashSet<PublicKey>>>;
pub type ConnectedNodes = Arc<tokio::sync::Mutex<HashSet<PublicKey>>>;

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

pub async fn accept(
    endpoint: MagicEndpoint,
    connecting: quinn::Connecting,
    approved_nodes: ApprovedNodes,
    approval_queue: tokio::sync::mpsc::Sender<(PublicKey, oneshot::Sender<bool>)>,
    connected_nodes: ConnectedNodes,
    state: watch::Receiver<String>,
) {
    let (node_id, _alpn, connection) = match accept_conn(connecting).await {
        Ok(data) => data,
        Err(error) => {
            log::error!("Error accepting incoming connection: {}", error);
            return;
        }
    };

    match wait_for_approval(node_id, approved_nodes, approval_queue).await {
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

    handle_connection(endpoint, connection, connected_nodes, node_id, state).await;
}

async fn wait_for_approval(
    node_id: PublicKey,
    approved_nodes: ApprovedNodes,
    approval_queue: tokio::sync::mpsc::Sender<(PublicKey, oneshot::Sender<bool>)>,
) -> anyhow::Result<bool> {
    if !approved_nodes.read().await.contains(&node_id) {
        let (tx, rx) = oneshot::channel();
        approval_queue.send((node_id, tx)).await?;

        let approved = rx.await?;

        if !approved {
            return Ok(false);
        }

        approved_nodes.write().await.insert(node_id);
    }

    Ok(true)
}

pub async fn connect(
    endpoint: MagicEndpoint,
    addr: NodeAddr,
    connected_nodes: ConnectedNodes,
    state: watch::Receiver<String>,
) {
    if connected_nodes.lock().await.contains(&addr.node_id) {
        log::warn!("Not connecting to {}: already connected", addr.node_id);
        return;
    }

    let node_id = addr.node_id;

    let connection = match endpoint.connect(addr, ALPN).await {
        Ok(connection) => connection,
        Err(error) => {
            log::error!("Connecting to {} failed: {}", node_id, error);
            return;
        }
    };

    handle_connection(endpoint, connection, connected_nodes, node_id, state).await;
}

async fn handle_connection(
    endpoint: MagicEndpoint,
    connection: quinn::Connection,
    connected_nodes: ConnectedNodes,
    node_id: PublicKey,
    state: watch::Receiver<String>,
) {
    let mut third_parties = Vec::new();

    {
        let mut connected_nodes = connected_nodes.lock().await;

        if connected_nodes.contains(&node_id) {
            log::info!("Ending new connection to {}: already connected", node_id);
            return;
        }

        for &node_id in connected_nodes.iter() {
            let connection_info = match endpoint.connection_info(node_id).await {
                Err(error) => {
                    log::error!("Error getting connection info for {}: {}", node_id, error);
                    continue;
                }
                Ok(None) => {
                    log::error!("No connection info for {} found.", node_id);
                    continue;
                }
                Ok(Some(info)) => info,
            };

            third_parties.push(NodeAddr {
                node_id,
                info: AddrInfo {
                    derp_url: connection_info.derp_url,
                    direct_addresses: connection_info.addrs.iter().map(|addr| addr.addr).collect(),
                },
            });
        }

        connected_nodes.insert(node_id);
    }

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
            if let Err(error) = handle_incoming(
                node_id,
                connection,
                endpoint,
                connected_nodes,
                state.clone(),
            )
            .await
            {
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

async fn handle_outgoing(
    connection: quinn::Connection,
    state: watch::Receiver<String>,
) -> anyhow::Result<()> {
    let mut state_stream = tokio_stream::wrappers::WatchStream::from_changes(state);
    while let Some(state) = state_stream.next().await {
        let mut stream = connection.open_uni().await?;
        stream.write_all(&[PacketType::Data as u8]).await?;
        stream.write_all(state.as_bytes()).await?;
    }
    Ok(())
}

async fn handle_incoming(
    node_id: PublicKey,
    connection: quinn::Connection,
    endpoint: MagicEndpoint,
    connected_nodes: ConnectedNodes,
    state: watch::Receiver<String>,
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
                    fn spawn_connect(
                        endpoint: MagicEndpoint,
                        node_addr: NodeAddr,
                        connected_nodes: ConnectedNodes,
                        state: watch::Receiver<String>,
                    ) {
                        tokio::spawn(async move {
                            connect(endpoint, node_addr, connected_nodes, state).await;
                        });
                    }

                    spawn_connect(
                        endpoint.clone(),
                        node_addr,
                        connected_nodes.clone(),
                        state.clone(),
                    );
                }
            }
        }
    }
}
