use std::fs;
use std::net;
use std::thread;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use std::io::ErrorKind;

use std::time::Duration;

use toml::Value;

mod client;
mod message;

// TODO: write a client
// TODO: send serverinfo if to clients if a client connected
// TODO: send time with all messages

static MAX_CLIENT_THREADS: usize = 64;

struct ClientThread {
    join_handle: thread::JoinHandle<()>,
    is_active: Arc<AtomicBool>,
}

fn main() {
    let value = fs::read_to_string("roc_server.toml").expect("Could not open roc_server.toml");

    let parsed = value
        .parse::<Value>()
        .expect("Could not parse roc_server.toml");

    let port = parsed["port"].as_integer().unwrap_or(8000);

    println!("Using port: {:?}.", port);

    let listener = net::TcpListener::bind(format!("127.0.0.1:{:?}", port))
        .expect(&format!("Could not bind to adress 127.0.0.1:{:?}", port));

    listener
        .set_nonblocking(true)
        .expect("ERROR: Could not set tcp-listener to non-blocking. Terminating server...");

    let mut client_threads: Vec<ClientThread> = vec![];

    let clients: Arc<Mutex<Vec<client::Client>>> = Arc::new(Mutex::new(vec![]));

    // register the SIGINT-call to terminate the main server-loop
    let should_stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&should_stop))
        .expect("ERROR: Unable to register SIGINT-call. terminating...");
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&should_stop))
        .expect("ERROR: Unable to register SIGTERM-call. terminating...");

    println!("processing clients...");
    for stream in listener.incoming() {
        match stream {
            Ok(v) => {
                client_threads = client_threads
                    .into_iter()
                    .filter(|client_thread| client_thread.is_active.load(Ordering::Relaxed))
                    .collect();

                if client_threads.len() >= MAX_CLIENT_THREADS {
                    println!("WARNING: Maximum client threads already created. Waiting for threads to close, ignoring incoming connection...");
                    continue;
                }

                let active_bool = Arc::new(AtomicBool::new(true));
                let active_bool_cloned = active_bool.clone();
                let clients_cloned = clients.clone();

                let new_thread = thread::spawn(|| {
                    client::handle_client(v, active_bool_cloned, clients_cloned);
                });

                let new_client_thread = ClientThread {
                    join_handle: new_thread,
                    is_active: active_bool,
                };

                client_threads.push(new_client_thread);
            }
            Err(e) => match e.kind() {
                ErrorKind::WouldBlock => (),
                _ => {
                    println!("WARNING: Could not handle client: {}", e);
                }
            },
        };
        if should_stop.load(Ordering::Relaxed) {
            println!("Received signal will terminate...");
            break;
        }

        std::thread::sleep(Duration::new(0, 20_000));
    }

    println!("Shutting down. Waiting for all client threads to shut down...");

    for thread in client_threads {
        thread.is_active.store(false, Ordering::Relaxed);
        match thread.join_handle.join() {
            Ok(_) => (),
            Err(_) => {
                println!("ERROR: Problems joining a client-thread.");
            }
        }
    }
}
