use std::fs;
use std::io::Write;
use std::net;
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::sync::Mutex;
use toml::Value;

use std::time::Duration;

use std::convert::TryInto;

use std::io::{Error, ErrorKind};

use std::io::Read;
use std::string::ToString;

use regex::Regex;

use std::thread;

// TODO: cleanup project. multiple files/modules?
// TODO: write a client

static MAX_CLIENT_THREADS: usize = 64;
static CLIENT_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
enum Message {
    RegisterUsername(String),
    Text(String),
}

impl ToString for Message {
    fn to_string(&self) -> String {
        match self {
            Message::RegisterUsername(v) => {
                return v.to_string();
            }
            Message::Text(v) => {
                return v.to_string();
            }
        }
    }
}

struct Client {
    stream: net::TcpStream,
    id: usize,
}

impl Client {
    fn new(stream: net::TcpStream) -> Client {
        Client {
            stream: stream,
            id: CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }
}

struct ClientThread {
    join_handle: thread::JoinHandle<()>,
    is_active: Arc<AtomicBool>,
}

fn convert_be_u8_to_usize(buffer: &[u8; 4]) -> usize {
    let mut result: usize = buffer[3] as usize;

    result += (buffer[2] as usize) << 8;
    result += (buffer[1] as usize) << 16;
    result += (buffer[0] as usize) << 24;

    result
}

fn convert_usize_to_be_u8(a: usize) -> [u8; 4] {
    let mut result = [0; 4];

    result[0] = (a & 0x000000ff) as u8;
    result[1] = (a & 0x0000ff00 >> 8) as u8;
    result[2] = (a & 0x00ff0000 >> 16) as u8;
    result[3] = (a & 0xff000000 >> 24) as u8;

    result
}

fn parse_message(message: String) -> Result<Message, Error> {
    let text_re = Regex::new(r"^Text\|(.)*$").unwrap();
    let username_re = Regex::new(r"^RegisterUsername\|(.)*$").unwrap();

    if text_re.is_match(&message) {
        return Ok(Message::Text(String::from(&message[5..])));
    } else if username_re.is_match(&message) {
        return Ok(Message::RegisterUsername(String::from(&message[17..])));
    }

    return Err(Error::new(
        ErrorKind::InvalidData,
        "Message not of type Text or RegisterUsername",
    ));
}

fn send_message(
    stream: &mut net::TcpStream,
    message: &Message,
    username: &String,
) -> Result<(), Error> {
    // 1. get length of message
    let message = match message {
        Message::Text(v) => format!("{}: {}", username, v),

        Message::RegisterUsername(v) => {
            format!("\"{}\" changed username to \"{}\"", username, v)
        }
    };

    let message = message.as_bytes();

    let message_length = convert_usize_to_be_u8(message.len());

    // 2. send length of message
    stream.write(&message_length)?;

    // 3. send utf8-encoded bytes of message
    stream.write(&message)?;

    Ok(())
}

fn get_message(stream: &mut net::TcpStream) -> Result<Message, Error> {
    let mut tmp_buffer = [0; 512];

    let mut raw_message: Vec<u8> = Vec::new();

    // first get length of the message:
    let read_in_bytes = stream.read(&mut tmp_buffer[..4])?;

    if read_in_bytes != 4 {
        if read_in_bytes == 0 {
            return Err(Error::new(
                ErrorKind::ConnectionAborted,
                "Client aborted the connection",
            ));
        }

        return Err(Error::new(
            ErrorKind::InvalidData,
            "Unable to read in first 4 bytes making up the u32 message-length",
        ));
    }

    let mut message_size = match &tmp_buffer[..4].try_into() {
        Ok(v) => convert_be_u8_to_usize(&v),
        Err(_) => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Unable to convert first 4 bytes into message-length",
            ));
        }
    };

    while message_size > 0 {
        let read_in = stream.read(&mut tmp_buffer)?;

        raw_message.extend_from_slice(&tmp_buffer[..read_in]);

        message_size -= read_in;
    }

    let message = match String::from_utf8(raw_message) {
        Ok(v) => v,
        Err(_) => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Unable to parse Vec<u8> to UTF8",
            ));
        }
    };

    parse_message(message)
}

fn handle_client(
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
        let message = match get_message(&mut stream) {
            Ok(v) => v,
            Err(e) => match e.kind() {
                ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::new(0, 20_000));
                    continue;
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
                if let Err(e) = send_message(&mut (*client).stream, &message, &username) {
                    println!("WARNING: Error when sending message to client: {}", e);
                }
            }
        }

        // change username if message was RegisterUsername
        if let Message::RegisterUsername(v) = message {
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

    dbg!((*clients_unlocked).len());

    // delete client-struct from vector
    let index = (*clients_unlocked)
        .iter()
        .position(|x| (*x).id == own_id)
        .unwrap();
    (*clients_unlocked).remove(index);

    dbg!((*clients_unlocked).len());

    // tell the main-thread that this thread has exited...
    active_bool.store(false, Ordering::Relaxed);
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

    let clients: Arc<Mutex<Vec<Client>>> = Arc::new(Mutex::new(vec![]));

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
                    handle_client(v, active_bool_cloned, clients_cloned);
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
