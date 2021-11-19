use std::net;

use std::io::ErrorKind;
use std::sync::{atomic::AtomicBool, atomic::AtomicUsize, atomic::Ordering, Arc, Mutex};
use std::time::Duration;

use crate::message;

static CLIENT_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Client {
    pub stream: net::TcpStream,
    pub id: usize,
}

impl Client {
    pub fn new(stream: net::TcpStream) -> Client {
        Client {
            stream: stream,
            id: CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }
}

pub fn handle_client(
    mut stream: net::TcpStream,
    active_bool: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<Client>>>,
) {
    // set stream-timeout to 20ms
    stream
        .set_read_timeout(Some(Duration::new(0, 20_000)))
        .expect("ERROR: Could not set timeout of TcpStream.");

    // register new client
    let cloned_stream = match stream.try_clone() {
        Ok(v) => v,
        Err(e) => {
            println!("ERROR: Unable to clone TcpStream. Exiting thread: {}", e);
            active_bool.store(false, Ordering::Relaxed);
            return;
        }
    };

    let mut username = match stream.peer_addr() {
        Ok(v) => v.to_string(),
        Err(_) => String::from("UNKNOWN-IP"),
    };

    let own_client = Client::new(cloned_stream);
    let own_id = own_client.id;
    let mut clients_unlocked = match clients.lock() {
        Ok(v) => v,
        Err(e) => {
            println!(
                "ERROR: Unable to lock the clients-arc. Exiting thread: {}",
                e
            );
            active_bool.store(false, Ordering::Relaxed);
            return;
        }
    };

    (*clients_unlocked).push(own_client);

    // "unlock" clients again
    drop(clients_unlocked);

    // in loop:
    while active_bool.load(Ordering::Relaxed) {
        // 1. get message
        let message = match message::get_message(&mut stream) {
            Ok(v) => v,
            Err(e) => match e.kind() {
                ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::new(0, 20_000));
                    continue;
                }
                ErrorKind::ConnectionAborted => {
                    println!("User aborted connection. Sending out disconnect message to remaining clients...");
                    active_bool.store(false, Ordering::Relaxed);
                    message::Message::ServerInfo(
                        message::ServerInfoType::Disconnected,
                        format!("disconnected."),
                    )
                }
                ErrorKind::TimedOut => {
                    continue;
                }
                _ => {
                    println!(
                        "WARNING: Error while getting message: {} | {:?}",
                        e,
                        e.kind()
                    );
                    break;
                }
            },
        };

        // 2. get access to client-vector
        let mut clients_unlocked = match clients.lock() {
            Ok(v) => v,
            Err(e) => {
                println!(
                    "ERROR: Unable to lock the clients-arc. Exiting thread: {}",
                    e
                );
                active_bool.store(false, Ordering::Relaxed);
                break;
            }
        };

        // 3. send message to all clients but own client
        for client in &mut (*clients_unlocked) {
            if (*client).id != own_id {
                if let Err(e) = message::send_message(&mut (*client).stream, &message, &username) {
                    println!("WARNING: Error when sending message to client: {}", e);
                }
            }
        }

        // change username if message was RegisterUsername
        if let message::Message::RegisterUsername(v) = message {
            username = v.to_string();
        }
    }

    // delete own Client-struct
    let mut clients_unlocked = match clients.lock() {
        Ok(v) => v,
        Err(e) => {
            println!(
                "ERROR: Unable to lock the clients-arc. Exiting thread: {}",
                e
            );
            active_bool.store(false, Ordering::Relaxed);
            return;
        }
    };

    // delete client-struct from vector
    let index = (*clients_unlocked)
        .iter()
        .position(|x| (*x).id == own_id)
        .unwrap();
    (*clients_unlocked).remove(index);

    // tell the main-thread that this thread has exited...
    active_bool.store(false, Ordering::Relaxed);
}
