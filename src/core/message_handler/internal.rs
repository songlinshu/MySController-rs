use std::time::{SystemTime, UNIX_EPOCH};

use diesel;
use diesel::prelude::*;
use diesel::r2d2::ConnectionManager;
use r2d2::*;

use crate::channel::{Receiver, Sender};
use crate::core::message::internal::*;
use crate::model::node::Node;
use crate::model::node::nodes::dsl;

const MIN_NODE_ID: u8 = 1;
const MAX_NODE_ID: u8 = 254;

pub fn handle(
    receiver: &Receiver<InternalMessage>,
    response_sender: &Sender<String>,
    controller_forward_sender: &Sender<String>,
    db_connection: PooledConnection<ConnectionManager<SqliteConnection>>,
) {
    loop {
        if let Ok(message) = receiver.recv() {
            match message.sub_type {
                InternalType::IdRequest => send_node_id(&db_connection, response_sender, message),
                InternalType::SketchName => update_node_name(&db_connection, message),
                InternalType::Time => send_current_time(response_sender, message),
                InternalType::DiscoverResponse => {
                    send_discover_response(&db_connection, &message);
                    forward_to_controller(controller_forward_sender, message)
                }
                _ => forward_to_controller(controller_forward_sender, message),
            }
        }
    }
}

fn send_node_id(
    db_connection: &PooledConnection<ConnectionManager<SqliteConnection>>,
    response_sender: &Sender<String>,
    mut message: InternalMessage,
) {
    match get_next_node_id(db_connection) {
        Some(new_node_id) => match create_node(db_connection, i32::from(new_node_id)) {
            Ok(_) => match response_sender.send(message.as_response(new_node_id.to_string())) {
                Ok(_) => (),
                Err(_) => error!("Error while sending to node_handler"),
            },
            Err(_) => error!("Error while creating node with new id"),
        },
        None => error!("There is no free node id! All 254 id's are already reserved!"),
    }
}

fn update_node_name(
    db_connection: &PooledConnection<ConnectionManager<SqliteConnection>>,
    message: InternalMessage,
) {
    use crate::model::node::nodes::dsl::*;
    match diesel::update(nodes)
        .filter(node_id.eq(i32::from(message.node_id)))
        .filter(node_name.eq("New Node".to_owned()))
        .set(node_name.eq(message.payload.clone()))
        .execute(db_connection)
        {
            Ok(_) => (),
            Err(e) => error!(
                "Error while trying to update node name for {:?} : {:?}",
                message, e
            ),
        }
}

fn send_current_time(response_sender: &Sender<String>, mut message: InternalMessage) {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    match response_sender.send(message.as_response(since_the_epoch.as_secs().to_string())) {
        Ok(_) => (),
        Err(_) => error!("Error while sending to node_handler"),
    }
}

fn send_discover_response(
    db_connection: &PooledConnection<ConnectionManager<SqliteConnection>>,
    message: &InternalMessage,
) {
    match message.payload.parse::<u8>() {
        Ok(parent_node_id) => match update_network_topology(
            &db_connection,
            i32::from(message.node_id),
            i32::from(parent_node_id),
        ) {
            Ok(_) => info!("Updated network topology"),
            Err(e) => error!("Update network topology failed {:?}", e),
        },
        Err(e) => error!(
            "Error {:?} while parsing discover message payload {:?}",
            e, &message.payload
        ),
    }
}

fn forward_to_controller(controller_sender: &Sender<String>, message: InternalMessage) {
    match controller_sender.send(message.to_string()) {
        Ok(_) => (),
        Err(error) => error!(
            "Error while forwarding internal message to controller {:?}",
            error
        ),
    }
}

pub fn create_node(conn: &SqliteConnection, id: i32) -> Result<Node, diesel::result::Error> {
    let new_node = Node {
        node_id: id,
        node_name: "New Node".to_owned(),
        firmware_type: 0,
        firmware_version: 0,
        desired_firmware_type: 0,
        desired_firmware_version: 0,
        auto_update: false,
        scheduled: false,
        parent_node_id: 0,
    };

    diesel::insert_into(dsl::nodes)
        .values(&new_node)
        .execute(conn)
        .map(|_| new_node)
}

pub fn update_network_topology(
    conn: &SqliteConnection,
    _node_id: i32,
    _parent_node_id: i32,
) -> Result<usize, diesel::result::Error> {
    use crate::model::node::nodes::dsl::*;
    diesel::update(nodes)
        .filter(node_id.eq(_node_id))
        .set(parent_node_id.eq(_parent_node_id))
        .execute(conn)
}

pub fn get_next_node_id(conn: &SqliteConnection) -> Option<u8> {
    let existing_nodes = dsl::nodes
        .load::<Node>(conn)
        .expect("error while loading existing nodes");
    let used_node_ids: Vec<u8> = existing_nodes.iter().map(|node| node.node_id()).collect();
    for node_id in MIN_NODE_ID..=MAX_NODE_ID {
        if used_node_ids.contains(&node_id) {
            continue;
        }
        return Some(node_id);
    }
    None
}
