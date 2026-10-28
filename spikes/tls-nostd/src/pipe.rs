//! Drives rustls's unbuffered connections (the only kind without std) over
//! plain byte buffers: what `kainet` will do over a NET_RECV/NET_SEND stream,
//! here between a client and a server in one process.

use alloc::vec::Vec;
use rustls::client::{ClientConnectionData, UnbufferedClientConnection};
use rustls::server::{ServerConnectionData, UnbufferedServerConnection};
use rustls::unbuffered::{
    ConnectionState, EncodeError, EncodeTlsData, EncryptError, UnbufferedStatus, WriteTraffic,
};

#[derive(Debug)]
pub enum PipeError {
    Tls(rustls::Error),
    /// The connection could not encrypt any more (its record limit).
    Exhausted,
    /// A state this driver does not expect (early data).
    Unexpected,
}

impl From<rustls::Error> for PipeError {
    fn from(error: rustls::Error) -> Self {
        Self::Tls(error)
    }
}

/// An unbuffered connection of either side.
pub trait Endpoint {
    type Data;
    fn process<'c, 'i>(
        &'c mut self,
        incoming: &'i mut [u8],
    ) -> UnbufferedStatus<'c, 'i, Self::Data>;
}

impl Endpoint for UnbufferedClientConnection {
    type Data = ClientConnectionData;
    fn process<'c, 'i>(
        &'c mut self,
        incoming: &'i mut [u8],
    ) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process_tls_records(incoming)
    }
}

impl Endpoint for UnbufferedServerConnection {
    type Data = ServerConnectionData;
    fn process<'c, 'i>(
        &'c mut self,
        incoming: &'i mut [u8],
    ) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process_tls_records(incoming)
    }
}

/// What one side did in a turn.
#[derive(Debug, Default, Clone, Copy)]
pub struct Turn {
    /// Application bytes decrypted.
    pub received: usize,
    /// The handshake is over: application data may flow.
    pub ready: bool,
    pub closed: bool,
}

/// Application data per WriteTraffic call: one full 16 KiB record.
const WRITE_CHUNK: usize = 16 * 1024;
/// Room for one record's header, content type and tag.
const RECORD_OVERHEAD: usize = 5 + 1 + 16;

/// Runs `conn` until it needs bytes from its peer: it reads `incoming` (and
/// removes what it consumed), appends what it sends to `outgoing`, encrypts
/// up to `budget` bytes of `send` once the handshake allows, and passes each
/// decrypted record to `sink`.
pub fn turn<E: Endpoint>(
    conn: &mut E,
    incoming: &mut Vec<u8>,
    outgoing: &mut Vec<u8>,
    send: &mut &[u8],
    budget: usize,
    sink: &mut dyn FnMut(&[u8]),
) -> Result<Turn, PipeError> {
    let mut turn = Turn::default();
    let mut consumed = 0;
    let mut sent = 0;
    loop {
        let UnbufferedStatus { discard, state } = conn.process(&mut incoming[consumed..]);
        let state = state?;
        let mut stop = false;
        match state {
            ConnectionState::ReadTraffic(mut read) => {
                while let Some(record) = read.next_record() {
                    let record = record?;
                    turn.received += record.payload.len();
                    sink(record.payload);
                }
            }
            ConnectionState::EncodeTlsData(mut encode) => encode_into(&mut encode, outgoing),
            // The bytes are already in `outgoing`: the caller hands them over.
            ConnectionState::TransmitTlsData(transmit) => transmit.done(),
            ConnectionState::BlockedHandshake => stop = true,
            ConnectionState::WriteTraffic(mut write) => {
                turn.ready = true;
                if send.is_empty() || sent >= budget {
                    stop = true;
                } else {
                    let n = send.len().min(WRITE_CHUNK).min(budget - sent);
                    encrypt_into(&mut write, &send[..n], outgoing)?;
                    *send = &send[n..];
                    sent += n;
                }
            }
            ConnectionState::PeerClosed | ConnectionState::Closed => {
                turn.closed = true;
                stop = true;
            }
            _ => return Err(PipeError::Unexpected),
        }
        consumed += discard;
        if stop {
            break;
        }
    }
    incoming.drain(..consumed);
    Ok(turn)
}

fn encode_into<D>(encode: &mut EncodeTlsData<'_, D>, out: &mut Vec<u8>) {
    let start = out.len();
    let mut room = 4096;
    loop {
        out.resize(start + room, 0);
        match encode.encode(&mut out[start..]) {
            Ok(n) => {
                out.truncate(start + n);
                return;
            }
            Err(EncodeError::InsufficientSize(need)) => room = need.required_size,
            Err(EncodeError::AlreadyEncoded) => {
                out.truncate(start);
                return;
            }
        }
    }
}

fn encrypt_into<D>(
    write: &mut WriteTraffic<'_, D>,
    data: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), PipeError> {
    let start = out.len();
    let mut room = data.len() + RECORD_OVERHEAD * (data.len() / WRITE_CHUNK + 2);
    loop {
        out.resize(start + room, 0);
        match write.encrypt(data, &mut out[start..]) {
            Ok(n) => {
                out.truncate(start + n);
                return Ok(());
            }
            Err(EncryptError::InsufficientSize(need)) => room = need.required_size,
            Err(_) => {
                out.truncate(start);
                return Err(PipeError::Exhausted);
            }
        }
    }
}
