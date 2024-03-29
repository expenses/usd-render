use crate::{ipc, layers, util::spawn_fallible, UsdState, ALPN};
use bbl_usd::cpp;
use iroh_net::{key::PublicKey, magic_endpoint::accept_conn, AddrInfo, MagicEndpoint, NodeAddr};
use std::collections::HashSet;
use std::sync::{atomic, Arc};
use tokio::sync::{mpsc, oneshot, watch};

pub type ApprovedNodes = Arc<scc::HashMap<PublicKey, NodeSharingPolicy>>;
pub type ConnectedNodes = Arc<scc::HashSet<PublicKey>>;
pub type ApprovalQueue = tokio::sync::mpsc::Sender<NodeApprovalRequest>;

// Adds a nodeid to the connected nodes set on creation, removes it on drop.
pub struct NodeConnection {
    connected_nodes: ConnectedNodes,
    node_id: PublicKey,
}

impl NodeConnection {
    pub async fn new(state: &State, node_id: PublicKey) -> Option<Self> {
        if state.connected_nodes.insert_async(node_id).await.is_ok() {
            Some(Self {
                connected_nodes: state.connected_nodes.clone(),
                node_id,
            })
        } else {
            log::info!("Not connecting to {}: already connected", node_id);
            None
        }
    }
}

impl Drop for NodeConnection {
    fn drop(&mut self) {
        log::info!("Disconnecting from {}", self.node_id);
        tokio::spawn({
            let connected_nodes = self.connected_nodes.clone();
            let node_id = self.node_id;
            async move {
                connected_nodes.remove_async(&node_id).await;
            }
        });
    }
}

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
    pub state: watch::Receiver<ipc::PublicLayerState>,
    pub usd: Arc<tokio::sync::RwLock<UsdState>>,
}

pub async fn accept(connecting: quinn::Connecting, state: State) {
    let (node_id, _alpn, connection) = match accept_conn(connecting).await {
        Ok(data) => data,
        Err(error) => {
            log::error!("Error accepting incoming connection: {}", error);
            return;
        }
    };

    log::info!("Accepted connection from {}", node_id);

    let _node_connection = match NodeConnection::new(&state, node_id).await {
        Some(node_connection) => node_connection,
        None => {
            connection.close(0_u32.into(), b"already connected");
            return;
        }
    };

    if !wait_for_approval(
        state.clone(),
        node_id,
        NodeApprovalDirection::Incoming,
        Some(connection.clone()),
    )
    .await
    {
        return;
    }

    handle_connection(state.clone(), connection, node_id).await;
}

async fn wait_for_approval(
    state: State,
    node_id: PublicKey,
    direction: NodeApprovalDirection,
    connection: Option<quinn::Connection>,
) -> bool {
    if state.approved_nodes.contains_async(&node_id).await {
        return true;
    }

    log::info!("Waiting for approval to connect to {}", node_id);

    let send_and_get_response = || async move {
        let (tx, rx) = oneshot::channel();

        state
            .approval_queue
            .send(NodeApprovalRequest {
                node_id,
                direction,
                response_sender: tx,
            })
            .await?;

        log::debug!("Sent message on approval queue");

        let response = rx.await?;

        log::info!("Got response");

        Ok::<_, anyhow::Error>(response)
    };

    let response = match send_and_get_response().await {
        Ok(response) => response,
        Err(error) => {
            if let Some(connection) = connection {
                connection.close(0_u32.into(), b"error");
            }
            log::error!("{}", error);
            return false;
        }
    };

    let node_sharing = match response {
        NodeApprovalResponse::Approved(node_sharing) => node_sharing,
        NodeApprovalResponse::Denied => {
            if let Some(connection) = connection {
                connection.close(0_u32.into(), b"denied");
            }
            log::info!("Denied connection to {}", node_id.fmt_short());
            return false;
        }
    };

    let _ = state
        .approved_nodes
        .insert_async(node_id, node_sharing)
        .await;

    true
}

pub async fn connect(state: State, addr: NodeAddr, referrer: Option<PublicKey>) {
    let node_id = addr.node_id;

    let _node_connection = match NodeConnection::new(&state, node_id).await {
        Some(node_connection) => node_connection,
        None => {
            return;
        }
    };

    if let Some(referrer) = referrer {
        if !wait_for_approval(
            state.clone(),
            addr.node_id,
            NodeApprovalDirection::Outgoing { referrer },
            None,
        )
        .await
        {
            return;
        }
    } else {
        let _ = state.approved_nodes.insert(
            addr.node_id,
            NodeSharingPolicy::AllExcept(Default::default()),
        );
    }

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
    let mut existing_node_ids = Vec::new();

    state
        .connected_nodes
        .scan_async(|existing_node_id| {
            if existing_node_id == &connection_node_id {
                return;
            }
            existing_node_ids.push(*existing_node_id);
        })
        .await;

    let mut third_parties = Vec::new();

    for existing_node_id in existing_node_ids {
        match state.approved_nodes.get_async(&existing_node_id).await {
            Some(sharing_policy) => {
                if !sharing_policy.get().allows(connection_node_id) {
                    log::info!("Not sharing {} to {}", existing_node_id, connection_node_id);
                    continue;
                }
            }
            None => {
                log::error!("Node {} connected but not allowed.", existing_node_id);
                continue;
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

    log::info!("Finished handling the connection to {}", connection_node_id);
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

    log::info!("Sent third parties");

    Ok(())
}

async fn write_data_packet(
    stream: &mut quinn::SendStream,
    index: u32,
    update_index: u32,
    state: &cpp::String,
) -> anyhow::Result<()> {
    stream.write_all(&[PacketType::Data as u8]).await?;
    stream.write_all(&index.to_le_bytes()).await?;
    stream.write_all(&update_index.to_le_bytes()).await?;
    stream.write_all(state.as_bytes()).await?;
    Ok(())
}

async fn handle_outgoing(connection: quinn::Connection, mut state: State) -> anyhow::Result<()> {
    spawn_fallible(
        {
            let connection = connection.clone();
            let layers = state.state.borrow().layers.clone();
            async move {
                for (index, layer) in layers.iter().enumerate() {
                    println!("{}: {}", index, layer.as_str());
                    let mut stream = connection.open_uni().await?;
                    stream.set_priority(i32::max_value().saturating_sub(index as i32))?;
                    write_data_packet(&mut stream, index as u32, u32::max_value(), layer).await?;
                }

                log::info!("Sent initial layers");

                Ok(())
            }
        },
        |error| async move {
            log::error!("{}", error);
        },
    );

    let (error_tx, mut error_rx) = mpsc::channel(1);

    loop {
        match error_rx.try_recv() {
            Ok(error) => return Err(error),
            Err(error @ mpsc::error::TryRecvError::Disconnected) => {
                return Err(anyhow::anyhow!("{}", error))
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        state.state.changed().await?;
        let (layer, index, update_index) = {
            let state = state.state.borrow();
            (
                state.layers[state.updated_layer].clone(),
                state.updated_layer,
                state.update_index,
            )
        };
        let error_tx = error_tx.clone();
        let connection = connection.clone();
        spawn_fallible(
            async move {
                let mut stream = connection.open_uni().await?;
                stream.set_priority(update_index as i32)?;
                write_data_packet(&mut stream, index as u32, update_index as u32, &layer).await?;
                Ok(())
            },
            |error| async move {
                let _ = error_tx.send(error).await;
            },
        );
    }
}

async fn handle_incoming(
    state: State,
    node_id: PublicKey,
    connection: quinn::Connection,
) -> anyhow::Result<()> {
    let mut remote_sublayers = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let latest_update = Arc::new(atomic::AtomicU32::new(0));

    let remote_root_layer = Arc::new(bbl_usd::sdf::Layer::create_anonymous(".usda"));
    state
        .usd
        .write()
        .await
        .root_layer
        .insert_sub_layer_path(remote_root_layer.get_identifier(), 0);

    log::info!("Created initial root layer");

    loop {
        let mut stream = connection.accept_uni().await?;
        let remote_sublayers = remote_sublayers.clone();
        let remote_root_layer = remote_root_layer.clone();
        let state = state.clone();
        let latest_update = latest_update.clone();
        spawn_fallible(
            async move {
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
                        let mut index = [0_u8; 4];
                        stream.read_exact(&mut index).await?;
                        let index = u32::from_le_bytes(index);

                        let mut update_index = [0_u8; 4];
                        stream.read_exact(&mut update_index).await?;
                        let update_index = u32::from_le_bytes(update_index);

                        if update_index != u32::max_value() {
                            let prev_latest =
                                latest_update.fetch_max(update_index, atomic::Ordering::Relaxed);
                            if prev_latest >= update_index {
                                log::info!(
                                    "prev_latest: {prev_latest}, update_index: {update_index}, skipping"
                                );
                                return Ok(());
                            }
                        }

                        let data = stream.read_to_end(1024 * 1024).await?;
                        let string = std::str::from_utf8(&data)?;
                        let cpp_string = bbl_usd::cpp::String::new(string);

                        {
                            let _lock = state.usd.write().await;
                            let mut remote_sublayers = remote_sublayers.lock().await;
                            layers::update_remote_sublayers(
                                &remote_root_layer,
                                &mut remote_sublayers,
                                index as _,
                                &cpp_string,
                            )?;
                        }

                        //log::info!("Got {:?} bytes from {}", string, node_id);
                    }
                    PacketType::NewNode => {
                        let data = stream.read_to_end(1024 * 1024).await?;
                        let third_parties: Vec<NodeAddr> = postcard::from_bytes(&data)?;
                        for node_addr in third_parties.into_iter() {
                            fn spawn_connect(
                                state: State,
                                node_addr: NodeAddr,
                                referrer: PublicKey,
                            ) {
                                tokio::spawn(async move {
                                    connect(state, node_addr, Some(referrer)).await;
                                });
                            }

                            spawn_connect(state.clone(), node_addr, node_id);
                        }
                    }
                }

                Ok(())
            },
            |error| async move {
                log::error!("{}", error);
            },
        );
    }
}
