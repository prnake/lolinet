//! QUIC 连接：自签证书按 sha256 指纹钉扎（第一次 TOFU），控制流 Hello 过 token。
//! 新主机先只握手拿指纹（[`fingerprint`]），用户核对过再带着钉扎连、发 token：token 不会发给没核对过的证书。

use std::{
    net::{SocketAddr, ToSocketAddrs},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use lolidesk_proto::{self as proto, ControlReq, ControlResp, StreamKind};
use quinn::{Connection, RecvStream, SendStream};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    DigitallySignedStruct, SignatureScheme,
};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

#[derive(Debug)]
struct Pin {
    expected: Option<[u8; 32]>,
    seen: Mutex<Option<[u8; 32]>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if let Some(exp) = self.expected {
            if exp != fp {
                return Err(rustls::Error::General(format!(
                    "证书指纹变了：记住的是 {}，现在是 {}。主机重装了就 `lolidesk --forget <地址>`",
                    hex::encode(exp),
                    hex::encode(fp)
                )));
            }
        }
        *self.seen.lock().unwrap() = Some(fp);
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Welcome {
    pub host: String,
    pub os: String,
    pub version: String,
    pub desktop: bool,
    pub persistent: bool,
}

pub struct Client {
    pub conn: Connection,
    pub welcome: Welcome,
    pub addr: SocketAddr,
    control: tokio::sync::Mutex<(SendStream, RecvStream)>,
    _endpoint: quinn::Endpoint,
}

/// 主机名或地址，可带端口（默认 10190）。
pub fn resolve(host: &str) -> Result<SocketAddr> {
    let host = host.trim();
    let has_port = host.parse::<SocketAddr>().is_ok()
        || (host.starts_with('[') && host.contains("]:"))
        || (!host.starts_with('[') && host.matches(':').count() == 1);
    let with_port = if has_port {
        host.to_string()
    } else {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if bare.contains(':') {
            format!("[{bare}]:{}", proto::DEFAULT_PORT)
        } else {
            format!("{bare}:{}", proto::DEFAULT_PORT)
        }
    };
    with_port
        .to_socket_addrs()
        .with_context(|| format!("解析 {host}"))?
        .next()
        .with_context(|| format!("{host} 没有地址"))
}

fn endpoint_for(addr: SocketAddr) -> Result<quinn::Endpoint> {
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    Ok(quinn::Endpoint::client(bind)?)
}

/// 只握手、不发 token：拿主机证书的指纹给用户核对。
pub async fn fingerprint(addr: SocketAddr) -> Result<[u8; 32]> {
    let (cfg, pin) = client_config(None)?;
    let endpoint = endpoint_for(addr)?;
    let conn = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.connect_with(cfg, addr, proto::SERVER_NAME)?,
    )
    .await
    .with_context(|| format!("连 {addr} 超时"))?
    .with_context(|| format!("连 {addr}"))?;
    let fp = pin.seen.lock().unwrap().context("握手没拿到证书指纹")?;
    conn.close(0u32.into(), b"fingerprint");
    endpoint.wait_idle().await;
    Ok(fp)
}

fn client_config(pinned: Option<[u8; 32]>) -> Result<(quinn::ClientConfig, Arc<Pin>)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let pin = Arc::new(Pin {
        expected: pinned,
        seen: Mutex::new(None),
        provider: provider.clone(),
    });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(pin.clone())
        .with_no_client_auth();
    tls.alpn_protocols = vec![proto::ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .context("QUIC 不接受这份 TLS 配置")?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(5)));
    t.max_idle_timeout(Some(Duration::from_secs(30).try_into()?));
    t.datagram_receive_buffer_size(Some(16 << 20));
    t.datagram_send_buffer_size(1 << 20);
    t.stream_receive_window(quinn::VarInt::from_u32(8 << 20));
    t.send_window(8 << 20);
    cfg.transport_config(Arc::new(t));
    Ok((cfg, pin))
}

/// 连上并 Hello。`pinned` 为 None 时接受任何证书（调用方已经让用户核对过，或者 CLI 的 `-y`）。
pub async fn connect(
    addr: SocketAddr,
    token: &str,
    pinned: Option<[u8; 32]>,
    client_name: &str,
) -> Result<(Arc<Client>, [u8; 32])> {
    let (cfg, pin) = client_config(pinned)?;
    let endpoint = endpoint_for(addr)?;
    let conn = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.connect_with(cfg, addr, proto::SERVER_NAME)?,
    )
    .await
    .with_context(|| format!("连 {addr} 超时"))?
    .with_context(|| format!("连 {addr}"))?;
    let fp = pin.seen.lock().unwrap().context("握手没拿到证书指纹")?;

    let (mut send, mut recv) = open(&conn, StreamKind::Control).await?;
    proto::send(
        &mut send,
        &ControlReq::Hello {
            version: proto::VERSION,
            token: token.to_string(),
            client: client_name.to_string(),
        },
    )
    .await?;
    let welcome = match proto::recv::<ControlResp>(&mut recv).await? {
        Some(ControlResp::Welcome {
            host,
            os,
            version,
            desktop,
            persistent,
        }) => Welcome {
            host,
            os,
            version,
            desktop,
            persistent,
        },
        Some(ControlResp::Denied(why)) => bail!("主机拒绝：{why}"),
        other => bail!("Hello 回应不对：{other:?}"),
    };
    Ok((
        Arc::new(Client {
            conn,
            welcome,
            addr,
            control: tokio::sync::Mutex::new((send, recv)),
            _endpoint: endpoint,
        }),
        fp,
    ))
}

pub async fn open(conn: &Connection, kind: StreamKind) -> Result<(SendStream, RecvStream)> {
    let (mut send, recv) = conn.open_bi().await?;
    send.write_all(&[kind.to_u8()]).await?;
    Ok((send, recv))
}

impl Client {
    pub async fn open(&self, kind: StreamKind) -> Result<(SendStream, RecvStream)> {
        open(&self.conn, kind).await
    }

    /// 控制流是一问一答：同一时间只有一个请求在上面。
    pub async fn control(&self, req: ControlReq) -> Result<ControlResp> {
        let mut guard = self.control.lock().await;
        let (send, recv) = &mut *guard;
        proto::send(send, &req).await?;
        proto::recv::<ControlResp>(recv)
            .await?
            .context("控制流被主机关了")
    }

    pub fn rtt(&self) -> Duration {
        self.conn.rtt()
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn resolve_adds_port() {
        assert_eq!(resolve("127.0.0.1").unwrap().to_string(), "127.0.0.1:10190");
        assert_eq!(resolve("127.0.0.1:9").unwrap().to_string(), "127.0.0.1:9");
        assert_eq!(resolve("::1").unwrap().to_string(), "[::1]:10190");
        assert_eq!(resolve("[::1]").unwrap().to_string(), "[::1]:10190");
        assert_eq!(resolve("[::1]:7").unwrap().to_string(), "[::1]:7");
    }
}
