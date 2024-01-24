use crate::ALPN;
use iroh_net::key::PublicKey;
use iroh_net::magic_endpoint::AddrInfo;
use iroh_net::magic_endpoint::accept_conn;
use std::collections::HashMap;
use iroh_net::NodeAddr;
use std::sync::Arc;

// todo: use the magic endpoit to get addrinfo.
pub type ConnectedNodes = Arc<tokio::sync::Mutex<HashMap<PublicKey, AddrInfo>>>;

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
    connecting: quinn::Connecting,
    connected_nodes: ConnectedNodes,
) {
    let (node_id, _alpn, connection) = match accept_conn(connecting).await {
        Ok(data) => data,
        Err(error) => {
            log::error!("Error accepting incoming connection: {}", error);
            return;
        }
    };

    let mut addr_info = AddrInfo::default();
    addr_info.direct_addresses.insert(connection.remote_address());

    check_if_already_connected(connection.clone(), &connected_nodes, node_id, addr_info).await;

    handle_connection(connection, node_id, connected_nodes).await
}

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
