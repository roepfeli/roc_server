use std::net;

use std::convert::TryInto;
use std::io::{Error, ErrorKind, Read, Write};

use regex::Regex;

#[derive(Debug)]
pub enum Message {
    RegisterUsername(String),
    UserText(String),
    ServerInfo(ServerInfoType, String),
}

#[derive(Debug)]
pub enum ServerInfoType {
    Disconnected,
}

impl Message {
    fn to_string(&self, username: &String) -> String {
        match self {
            Message::UserText(v) => format!("UserText|{}: {}", username, v),

            Message::RegisterUsername(v) => {
                format!("ServerInfo|\"{}\" changed username to \"{}\"", username, v)
            }

            Message::ServerInfo(t, v) => match t {
                ServerInfoType::Disconnected => format!("ServerInfo|\"{}\" {}", username, v),
            },
        }
    }
}

fn convert_be_u8_array_to_u32(buffer: &[u8; 4]) -> u32 {
    let mut result = buffer[3] as u32;

    result += (buffer[2] as u32) << 8;
    result += (buffer[1] as u32) << 16;
    result += (buffer[0] as u32) << 24;

    result
}

fn convert_u32_to_be_u8_array(a: u32) -> [u8; 4] {
    let mut result = [0; 4];

    result[3] = (a & 0x000000ff) as u8;
    result[2] = ((a & 0x0000ff00) >> 8) as u8;
    result[1] = ((a & 0x00ff0000) >> 16) as u8;
    result[0] = ((a & 0xff000000) >> 24) as u8;

    result
}

pub fn parse_message(message: String) -> Result<Message, Error> {
    let text_re = Regex::new(r"^UserText\|(.)*$").unwrap();
    let username_re = Regex::new(r"^RegisterUsername\|(.)*$").unwrap();

    // TODO: hardcoded slices suck.. try using captures()!

    if text_re.is_match(&message) {
        return Ok(Message::UserText(String::from(&message[9..])));
    } else if username_re.is_match(&message) {
        return Ok(Message::RegisterUsername(String::from(&message[17..])));
    }

    return Err(Error::new(
        ErrorKind::InvalidData,
        "Message not of type Text or RegisterUsername",
    ));
}

pub fn send_message(
    stream: &mut net::TcpStream,
    message: &Message,
    username: &String,
) -> Result<(), Error> {
    // 1. get length of message

    // TODO: this is madness: avoiding message_* and putting it in one line leads to the borrowchecker complaining!
    // TODO: see why this is the case and how to fix it?
    // let message_bytes = message.to_string(&username).as_bytes();
    // let message_length = message_bytes.len();

    let message_string = message.to_string(&username);
    let message_bytes = message_string.as_bytes();

    let message_length = convert_u32_to_be_u8_array(message_bytes.len() as u32);

    // 2. send length of message
    stream.write(&message_length)?;

    // 3. send utf8-encoded bytes of message
    stream.write(&message_bytes)?;

    Ok(())
}

pub fn get_message(stream: &mut net::TcpStream) -> Result<Message, Error> {
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
        Ok(v) => convert_be_u8_array_to_u32(&v),
        Err(_) => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Unable to convert first 4 bytes into message-length",
            ));
        }
    };

    while message_size > 0 {
        let read_in = stream.read(&mut tmp_buffer)?;

        if read_in == 0 {
            break;
        }

        raw_message.extend_from_slice(&tmp_buffer[..read_in]);

        message_size -= read_in as u32;
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
