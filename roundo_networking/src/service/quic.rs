//! QUIC endpoint setup and deterministic pairing of prioritized streams.

use super::NetworkError;
use crate::protocol::StreamId;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, VarInt};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

// ALPN prevents negotiation with unrelated QUIC applications.
const ALPN_PROTOCOL: &[u8] = b"roundo-quic-v1";

/// Paired bidirectional adapters for the two protocol priorities.
pub(super) struct StreamPairs {
    pub(super) stream0: PairedStream,
    pub(super) stream1: PairedStream,
}

/// Combines independently opened QUIC send and receive streams.
pub(super) struct PairedStream {
    receiver: RecvStream,
    sender: SendStream,
}

impl AsyncRead for PairedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.receiver), context, buffer)
    }
}

impl AsyncWrite for PairedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.sender), context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.sender), context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.sender), context)
    }
}

/// Builds a server endpoint from certificate and transport settings.
pub(super) fn server_endpoint(
    address: SocketAddr,
    tls_config: Arc<rustls::ServerConfig>,
) -> Result<Endpoint, NetworkError> {
    let mut tls_config = (*tls_config).clone();
    tls_config.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
    let crypto = QuicServerConfig::try_from(tls_config).map_err(NetworkError::from_display)?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport_config());
    Endpoint::server(config, address).map_err(NetworkError::from_display)
}

/// Builds a client endpoint with the configured trust policy.
pub(super) fn client_endpoint(
    remote_address: SocketAddr,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<Endpoint, NetworkError> {
    let bind_ip = match remote_address.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let mut endpoint =
        Endpoint::client(SocketAddr::new(bind_ip, 0)).map_err(NetworkError::from_display)?;
    let mut tls_config = (*tls_config).clone();
    tls_config.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
    let crypto = QuicClientConfig::try_from(tls_config).map_err(NetworkError::from_display)?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport_config());
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

/// Completes a remote QUIC handshake.
pub(super) async fn dial(
    endpoint: &Endpoint,
    address: SocketAddr,
    server_name: &str,
) -> Result<Connection, NetworkError> {
    let connection_attempt = endpoint.connect(address, server_name);
    let connecting = connection_attempt.map_err(|error| {
        NetworkError::new(format!(
            "cannot begin QUIC connection to {server_name} at {address}: {error}"
        ))
    })?;
    connecting.await.map_err(|error| {
        NetworkError::new(format!(
            "QUIC connection to {server_name} at {address} failed: {error}"
        ))
    })
}

/// Opens and accepts both priorities, then pairs them by stream ID.
pub(super) async fn establish_streams(
    connection: &Connection,
) -> Result<StreamPairs, NetworkError> {
    let (senders, receivers) =
        tokio::try_join!(open_senders(connection), accept_receivers(connection),)?;
    Ok(StreamPairs {
        stream0: PairedStream {
            receiver: receivers.stream0,
            sender: senders.stream0,
        },
        stream1: PairedStream {
            receiver: receivers.stream1,
            sender: senders.stream1,
        },
    })
}

/// Locally opened send halves keyed by protocol stream.
struct Senders {
    stream0: SendStream,
    stream1: SendStream,
}

/// Remotely opened receive halves keyed by protocol stream.
struct Receivers {
    stream0: RecvStream,
    stream1: RecvStream,
}

// Streams are opened sequentially to assign explicit priorities.
async fn open_senders(connection: &Connection) -> Result<Senders, NetworkError> {
    let stream0 = open_sender(connection, StreamId::Stream0).await?;
    let stream1 = open_sender(connection, StreamId::Stream1).await?;
    Ok(Senders { stream0, stream1 })
}

// The stream ID prefix precedes framed application data.
async fn open_sender(
    connection: &Connection,
    stream_id: StreamId,
) -> Result<SendStream, NetworkError> {
    let mut sender = connection
        .open_uni()
        .await
        .map_err(NetworkError::from_display)?;
    sender
        .set_priority(quinn_priority(stream_id))
        .map_err(NetworkError::from_display)?;
    sender
        .write_u8(stream_id as u8)
        .await
        .map_err(NetworkError::from_display)?;
    Ok(sender)
}

// Quinn assigns larger values greater scheduling priority.
const fn quinn_priority(stream_id: StreamId) -> i32 {
    match stream_id {
        StreamId::Stream0 => 1,
        StreamId::Stream1 => 0,
    }
}

// Pairing rejects duplicate and unknown stream identifiers.
async fn accept_receivers(connection: &Connection) -> Result<Receivers, NetworkError> {
    let mut stream0 = None;
    let mut stream1 = None;
    while stream0.is_none() || stream1.is_none() {
        let mut receiver = connection
            .accept_uni()
            .await
            .map_err(NetworkError::from_display)?;
        let wire_id = receiver
            .read_u8()
            .await
            .map_err(NetworkError::from_display)?;
        match StreamId::from_wire(wire_id) {
            Some(StreamId::Stream0) if stream0.is_none() => stream0 = Some(receiver),
            Some(StreamId::Stream1) if stream1.is_none() => stream1 = Some(receiver),
            Some(stream_id) => {
                return Err(NetworkError::new(format!(
                    "peer opened duplicate {stream_id:?}"
                )));
            }
            None => {
                return Err(NetworkError::new(format!(
                    "peer opened unknown logical stream {wire_id}"
                )));
            }
        }
    }
    Ok(Receivers {
        stream0: stream0.expect("stream0 presence checked by loop condition"),
        stream1: stream1.expect("stream1 presence checked by loop condition"),
    })
}

// Shared limits bound idle lifetime and concurrent streams.
fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config
        .max_concurrent_bidi_streams(VarInt::from_u32(0))
        .max_concurrent_uni_streams(VarInt::from_u32(2));
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::quinn_priority;
    use roundo_contracts::StreamId;

    #[test]
    fn stream0_has_higher_quinn_priority_than_stream1() {
        assert!(quinn_priority(StreamId::Stream0) > quinn_priority(StreamId::Stream1));
    }
}
