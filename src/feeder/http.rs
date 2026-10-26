//! A minimal HTTP/1.0 GET client: one request per connection, the whole body into a buffer.
//! Enough for tar1090's static JSON files behind lighttpd (no chunking, no compression: HTTP/1.0
//! requests ask for neither).

use core::fmt::Write as _;

use embassy_net::Stack;
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Ipv4Address};
use embassy_time::Duration;
use embedded_io_async::Write as _;
use heapless::String;

/// How long a connection may stay silent.
const TIMEOUT: Duration = Duration::from_secs(4);

/// Why a request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The host name did not resolve.
    Dns,
    /// The connection was refused or timed out.
    Connect,
    /// The connection broke or timed out while the response arrived.
    Io,
    /// The response was not `HTTP/1.x 200`; holds the status (0 if unreadable).
    Status(u16),
    /// The response is larger than the buffer.
    TooLarge,
}

/// Resolves `host` (an IPv4 address or a name).
///
/// # Errors
/// [`Error::Dns`] when the name does not resolve.
pub async fn resolve(stack: Stack<'_>, host: &str) -> Result<Ipv4Address, Error> {
    if let Ok(ip) = host.parse::<Ipv4Address>() {
        return Ok(ip);
    }
    let addrs = stack
        .dns_query(host, DnsQueryType::A)
        .await
        .map_err(|_| Error::Dns)?;
    addrs
        .first()
        .map(|IpAddress::Ipv4(v4)| *v4)
        .ok_or(Error::Dns)
}

/// GETs `path` from `host:port` into `buf`; returns the body (a slice of `buf`).
///
/// # Errors
/// See [`Error`].
pub async fn get<'b>(
    stack: Stack<'_>,
    ip: Ipv4Address,
    host: &str,
    port: u16,
    path: &str,
    buf: &'b mut [u8],
) -> Result<&'b [u8], Error> {
    let mut rx = [0u8; 4096];
    let mut tx = [0u8; 256];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(TIMEOUT));
    socket
        .connect((ip, port))
        .await
        .map_err(|_| Error::Connect)?;
    let mut request: String<200> = String::new();
    // The request always fits: the path is ours and the host is at most 63 bytes.
    let _ = write!(
        request,
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: nano\r\n\r\n"
    );
    socket
        .write_all(request.as_bytes())
        .await
        .map_err(|_| Error::Io)?;

    let mut len = 0;
    loop {
        if len == buf.len() {
            socket.abort();
            return Err(Error::TooLarge);
        }
        match socket.read(&mut buf[len..]).await {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(_) => return Err(Error::Io),
        }
    }
    socket.close();
    body(&buf[..len])
}

/// The body of a complete response, after checking its status.
fn body(response: &[u8]) -> Result<&[u8], Error> {
    let head_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(Error::Io)?;
    let status = response
        .get(9..12)
        .and_then(|s| core::str::from_utf8(s).ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if !response.starts_with(b"HTTP/1.") || status != 200 {
        return Err(Error::Status(status));
    }
    Ok(&response[head_end + 4..])
}
