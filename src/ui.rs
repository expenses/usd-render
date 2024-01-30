use crate::networking::{self, NodeApprovalDirection, NodeApprovalResponse, NodeSharingPolicy};
use bbl_usd::cpp;
use iroh_net::{key::PublicKey, ticket::NodeTicket, NodeAddr};
use std::collections::HashMap;
use std::str::FromStr;
use tokio::sync::oneshot;

pub fn draw_connection(ui: &mut egui::Ui, connection_info: &iroh_net::magicsock::EndpointInfo) {
    ui.label(connection_info.id.to_string());
    ui.label(connection_info.public_key.fmt_short());
    ui.label(format!("{}", connection_info.conn_type));
    ui.label(match connection_info.latency {
        Some(duration) => format!("{:.2} ms", duration.as_secs_f32() * 1000.0),
        None => "N/A".to_string(),
    });
    ui.label(match connection_info.last_used {
        Some(duration) => format!("{:.2} s", duration.as_secs_f32()),
        None => "Never".to_string(),
    });
}

pub fn draw_connection_grid(
    ui: &mut egui::Ui,
    connection_infos: &[iroh_net::magicsock::EndpointInfo],
) {
    if connection_infos.is_empty() {
        ui.label("No current connections");
    } else {
        egui::Grid::new("connection_grid")
            .striped(true)
            .show(ui, |ui| {
                for connection_info in connection_infos {
                    draw_connection(ui, connection_info);
                    ui.end_row();
                }
            });
    }
}

pub fn draw_node_info(
    ui: &mut egui::Ui,
    addr: &NodeAddr,
    ticket: &NodeTicket,
    window: &mut glfw::Window,
) {
    ui.label(format!("Node ID: {}", addr.node_id.fmt_short()));
    ui.label("Node Ticket (click to copy):");
    let node_ticket_str = ticket.to_string();
    if ui.button(&node_ticket_str).clicked() {
        window.set_clipboard_string(&node_ticket_str);
    }
}

pub fn draw_connect_to_node(
    ui: &mut egui::Ui,
    networking_state: &networking::State,
    addr: &NodeAddr,
    text: &mut String,
) {
    let response = ui
        .horizontal(|ui| {
            ui.label("Connect to node: ");
            ui.add(egui::widgets::text_edit::TextEdit::singleline(text))
        })
        .inner;

    if response.lost_focus() && response.ctx.input(|ctx| ctx.key_pressed(egui::Key::Enter)) {
        match NodeTicket::from_str(text) {
            Ok(ticket) => {
                let node_addr = ticket.node_addr().clone();
                if node_addr == *addr {
                    log::error!("Not connecting to self.");
                } else {
                    tokio::spawn(networking::connect(
                        networking_state.clone(),
                        node_addr,
                        None,
                    ));
                    text.clear();
                }
            }
            Err(error) => {
                log::error!("bad ticket {}: {}", text, error);
            }
        }
    }
}

pub fn draw_buttons(ui: &mut egui::Ui, networking_state: &networking::State) {
    if ui.button("export").clicked() {
        tokio::spawn({
            let networking_state = networking_state.clone();
            async move {
                let state = networking_state.usd.read().await;
                state.stage.export(&cpp::String::new("export.usdc"));
            }
        });
    }
    if ui.button("save keyfile").clicked() {
        tokio::spawn({
            let endpoint = networking_state.endpoint.clone();
            async move {
                let function = || async move {
                    let key = endpoint.secret_key();
                    let serialized = key.to_openssh()?;
                    // None if cancelled.
                    if let Some(filehandle) = rfd::AsyncFileDialog::new().save_file().await {
                        filehandle.write(serialized.as_bytes()).await?;
                    }
                    Ok::<_, anyhow::Error>(())
                };

                if let Err(error) = function().await {
                    log::error!("{}", error);
                }
            }
        });
    }
}

pub fn draw_approval_queue(
    ui: &mut egui::Ui,
    approval_queue: &mut Vec<(
        PublicKey,
        NodeApprovalDirection,
        Option<oneshot::Sender<NodeApprovalResponse>>,
    )>,
    approved_nodes: &HashMap<PublicKey, NodeSharingPolicy>,
) {
    ui.heading("Approval Queue");

    approval_queue.retain_mut(|(node_id, direction, sender)| {
        if sender.is_none() {
            return false;
        }

        if let Some(node_sharing) = approved_nodes.get(node_id) {
            let _ = sender
                .take()
                .unwrap()
                .send(NodeApprovalResponse::Approved(node_sharing.clone()));
            return false;
        }

        ui.horizontal(|ui| {
            ui.label(node_id.fmt_short());
            match direction {
                networking::NodeApprovalDirection::Incoming => {
                    ui.label("Incoming");
                }
                networking::NodeApprovalDirection::Outgoing { referrer } => {
                    ui.label(format!(
                        "Outgoing (referred to by {})",
                        referrer.fmt_short()
                    ));
                }
            }

            let mut retain = true;

            if ui.button("Allow").clicked() {
                let _ = sender.take().unwrap().send(NodeApprovalResponse::Approved(
                    NodeSharingPolicy::AllExcept(Default::default()),
                ));
                retain = false;
            }

            if ui.button("Allow (private)").clicked() {
                let _ = sender.take().unwrap().send(NodeApprovalResponse::Approved(
                    NodeSharingPolicy::NoneExcept(Default::default()),
                ));
                retain = false;
            }
            if ui.button("Deny").clicked() {
                let _ = sender
                    .take()
                    .unwrap()
                    .send(networking::NodeApprovalResponse::Denied);
                retain = false;
            }

            retain
        })
        .inner
    });
}
