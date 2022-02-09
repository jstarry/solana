use {
    crossbeam_channel::Sender,
    futures_util::stream::StreamExt,
    pem::Pem,
    pkcs8::{der::Document, AlgorithmIdentifier, ObjectIdentifier},
    quinn::{Endpoint, EndpointConfig, ServerConfig},
    rcgen::{CertificateParams, DistinguishedName, DnType, SanType},
    solana_measure::measure::Measure,
    solana_perf::packet::PacketBatch,
    solana_sdk::{
        packet::{Packet, PACKET_DATA_SIZE},
        signature::Keypair,
    },
    std::{
        error::Error,
        net::{IpAddr, SocketAddr, UdpSocket},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    },
    tokio::{
        runtime::{Builder, Runtime},
        time::timeout,
    },
};

/// Returns default server configuration along with its PEM certificate chain.
#[allow(clippy::field_reassign_with_default)] // https://github.com/rust-lang/rust-clippy/issues/6527
fn configure_server(
    identity_keypair: &Keypair,
    gossip_host: IpAddr,
) -> Result<(ServerConfig, String), QuicServerError> {
    let (cert_chain, priv_key) =
        new_cert(identity_keypair, gossip_host).map_err(|_e| QuicServerError::ConfigureFailed)?;
    let cert_chain_pem_parts: Vec<Pem> = cert_chain
        .iter()
        .map(|cert| Pem {
            tag: "CERTIFICATE".to_string(),
            contents: cert.0.clone(),
        })
        .collect();
    let cert_chain_pem = pem::encode_many(&cert_chain_pem_parts);

    let mut server_config = ServerConfig::with_single_cert(cert_chain, priv_key)
        .map_err(|_e| QuicServerError::ConfigureFailed)?;
    let config = Arc::get_mut(&mut server_config.transport).unwrap();

    const MAX_CONCURRENT_UNI_STREAMS: u32 = 1;
    config.max_concurrent_uni_streams(MAX_CONCURRENT_UNI_STREAMS.into());
    config.stream_receive_window((PACKET_DATA_SIZE as u32).into());
    config.receive_window((PACKET_DATA_SIZE as u32 * MAX_CONCURRENT_UNI_STREAMS).into());

    // disable bidi & datagrams
    const MAX_CONCURRENT_BIDI_STREAMS: u32 = 0;
    config.max_concurrent_bidi_streams(MAX_CONCURRENT_BIDI_STREAMS.into());
    //config.datagram_receive_buffer_size(None);
    config.datagram_receive_buffer_size(Some(PACKET_DATA_SIZE));

    Ok((server_config, cert_chain_pem))
}

fn new_cert(
    identity_keypair: &Keypair,
    san: IpAddr,
) -> Result<(Vec<rustls::Certificate>, rustls::PrivateKey), Box<dyn Error>> {
    // Generate a self-signed cert from validator identity key
    let cert_params = new_cert_params(identity_keypair, san);
    let cert = rcgen::Certificate::from_params(cert_params)?;
    let cert_der = cert.serialize_der().unwrap();
    let priv_key = cert.serialize_private_key_der();
    let priv_key = rustls::PrivateKey(priv_key);
    let cert_chain = vec![rustls::Certificate(cert_der)];
    Ok((cert_chain, priv_key))
}

fn convert_to_rcgen_keypair(identity_keypair: &Keypair) -> rcgen::KeyPair {
    // from https://datatracker.ietf.org/doc/html/rfc8410#section-3
    const ED25519_IDENTIFIER: [u32; 4] = [1, 3, 101, 112];
    let mut private_key = Vec::<u8>::with_capacity(34);
    private_key.extend_from_slice(&[0x04, 0x20]); // ASN.1 OCTET STRING
    private_key.extend_from_slice(identity_keypair.secret().as_bytes());
    let key_pkcs8 = pkcs8::PrivateKeyInfo {
        algorithm: AlgorithmIdentifier {
            oid: ObjectIdentifier::from_arcs(&ED25519_IDENTIFIER).unwrap(),
            parameters: None,
        },
        private_key: &private_key,
        public_key: None,
    };
    let key_pkcs8_der = key_pkcs8
        .to_der()
        .expect("Failed to convert keypair to DER")
        .to_der();

    // Parse private key into rcgen::KeyPair struct.
    rcgen::KeyPair::from_der(&key_pkcs8_der).expect("Failed to parse keypair from DER")
}

fn new_cert_params(identity_keypair: &Keypair, san: IpAddr) -> CertificateParams {
    // TODO(terorie): Is it safe to sign the TLS cert with the identity private key?

    // Unfortunately, rcgen does not accept a "raw" Ed25519 key.
    // We have to convert it to DER and pass it to the library.

    // Convert private key into PKCS#8 v1 object.
    // RFC 8410, Section 7: Private Key Format
    // https://datatracker.ietf.org/doc/html/rfc8410#section-

    let keypair = convert_to_rcgen_keypair(identity_keypair);

    let mut cert_params = CertificateParams::default();
    cert_params.subject_alt_names = vec![SanType::IpAddress(san)];
    cert_params.alg = &rcgen::PKCS_ED25519;
    cert_params.key_pair = Some(keypair);
    cert_params.distinguished_name = DistinguishedName::new();
    cert_params
        .distinguished_name
        .push(DnType::CommonName, "Solana node");
    cert_params
}

pub fn rt() -> Runtime {
    //Builder::new_current_thread().enable_all().build().unwrap()
    Builder::new_multi_thread().enable_all().build().unwrap()
}

#[derive(thiserror::Error, Debug)]
pub enum QuicServerError {
    #[error("Server configure failed")]
    ConfigureFailed,

    #[error("Endpoint creation failed")]
    EndpointFailed,
}

// Return true if the server should drop the stream
fn handle_chunk(
    chunk: &Result<Option<quinn::Chunk>, quinn::ReadError>,
    maybe_batch: &mut Option<PacketBatch>,
    remote_addr: &SocketAddr,
    packet_sender: &Sender<PacketBatch>,
    skip_send: bool,
) -> bool {
    match chunk {
        Ok(maybe_chunk) => {
            if let Some(chunk) = maybe_chunk {
                if skip_send {
                    packet_sender.send(PacketBatch::default()).unwrap();
                    return false;
                }
                trace!("got chunk: {:?}", chunk);
                let chunk_len = chunk.bytes.len() as u64;

                // shouldn't happen, but sanity check the size and offsets
                if chunk.offset > PACKET_DATA_SIZE as u64 || chunk_len > PACKET_DATA_SIZE as u64 {
                    return true;
                }
                if chunk.offset + chunk_len > PACKET_DATA_SIZE as u64 {
                    return true;
                }

                // chunk looks valid
                if maybe_batch.is_none() {
                    let mut batch = PacketBatch::with_capacity(1);
                    let mut packet = Packet::default();
                    packet.meta.set_addr(remote_addr);
                    batch.packets.push(packet);
                    *maybe_batch = Some(batch);
                }

                if let Some(batch) = maybe_batch.as_mut() {
                    let end = chunk.offset as usize + chunk.bytes.len();
                    batch.packets[0].data[chunk.offset as usize..end].copy_from_slice(&chunk.bytes);
                    batch.packets[0].meta.size = std::cmp::max(batch.packets[0].meta.size, end);
                }
            } else {
                debug!("chunk is none");
                // done receiving chunks
                if let Some(batch) = maybe_batch.take() {
                    let len = batch.packets[0].meta.size;
                    if let Err(e) = packet_sender.send(batch) {
                        info!("send error: {}", e);
                    } else {
                        trace!("sent {} byte packet", len);
                    }
                }
                return true;
            }
        }
        Err(e) => {
            debug!("Received stream error: {:?}", e);
            return true;
        }
    }
    false
}

pub fn spawn_server(
    sock: UdpSocket,
    keypair: &Keypair,
    gossip_host: IpAddr,
    packet_sender: Sender<PacketBatch>,
    exit: Arc<AtomicBool>,
    skip_send: bool,
) -> Result<thread::JoinHandle<()>, QuicServerError> {
    let (config, _cert) = configure_server(keypair, gossip_host)?;

    let runtime = rt();
    let (_, mut incoming) = {
        let _guard = runtime.enter();
        Endpoint::new(EndpointConfig::default(), Some(config), sock)
            .map_err(|_e| QuicServerError::EndpointFailed)?
    };

    let handle = thread::spawn(move || {
        let handle = runtime.spawn(async move {
            while !exit.load(Ordering::Relaxed) {
                const WAIT_FOR_CONNECTION_TIMEOUT_MS: u64 = 1000;
                let timeout_connection = timeout(
                    Duration::from_millis(WAIT_FOR_CONNECTION_TIMEOUT_MS),
                    incoming.next(),
                )
                .await;

                if let Ok(Some(connection)) = timeout_connection {
                    if let Ok(new_connection) = connection.await {
                        let exit = exit.clone();
                        let quinn::NewConnection {
                            connection,
                            mut uni_streams,
                            //mut datagrams,
                            ..
                        } = new_connection;

                        let remote_addr = connection.remote_address();
                        let packet_sender = packet_sender.clone();
                        let packet_sender1 = packet_sender.clone();

                        /*info!("got connection: {}", remote_addr);
                        tokio::spawn(async move {
                            while let Some(Ok(packet)) = datagrams.next().await {
                                info!("sending packet");
                                let mut batch = PacketBatch::with_capacity(1);
                                let mut packet = Packet::default();
                                batch.packets.push(packet);
                                packet_sender.send(batch).unwrap();
                            }
                        });*/

                        let packet_sender = packet_sender1.clone();
                        tokio::spawn(async move {
                            debug!("new connection {}", remote_addr);
                            loop {
                                let mut start = Measure::start("stream start");
                                if let Some(Ok(mut stream)) = uni_streams.next().await {
                                    debug!("new stream");
                                    let packet_sender = packet_sender.clone();
                                    let exit = exit.clone();
                                    //tokio::spawn(async move {
                                    let mut maybe_batch = None;
                                    while !exit.load(Ordering::Relaxed) {
                                        if handle_chunk(
                                            &stream.read_chunk(PACKET_DATA_SIZE, false).await,
                                            &mut maybe_batch,
                                            &remote_addr,
                                            &packet_sender,
                                            skip_send,
                                        ) {
                                            break;
                                        }
                                    }
                                    //});
                                } else {
                                    start.stop();
                                    debug!("{}", start);
                                    break;
                                }
                            }
                        });
                    }
                }
            }
        });
        if let Err(e) = runtime.block_on(handle) {
            warn!("error from runtime.block_on: {:?}", e);
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod test {
    use super::*;
    use crossbeam_channel::unbounded;
    use quinn::{ClientConfig, NewConnection};
    use std::{net::SocketAddr, time::Instant};

    struct SkipServerVerification;

    impl SkipServerVerification {
        fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }

    impl rustls::client::ServerCertVerifier for SkipServerVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::Certificate,
            _intermediates: &[rustls::Certificate],
            _server_name: &rustls::ServerName,
            _scts: &mut dyn Iterator<Item = &[u8]>,
            _ocsp_response: &[u8],
            _now: std::time::SystemTime,
        ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::ServerCertVerified::assertion())
        }
    }

    pub fn get_client_config() -> quinn::ClientConfig {
        let crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();
        ClientConfig::new(Arc::new(crypto))
    }

    #[test]
    fn test_quic_server_exit() {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let (sender, _receiver) = unbounded();
        let keypair = Keypair::new();
        let ip = "127.0.0.1".parse().unwrap();
        let t = spawn_server(s, &keypair, ip, sender, exit.clone(), false).unwrap();
        exit.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }

    fn make_client_endpoint(runtime: &Runtime, addr: &SocketAddr) -> NewConnection {
        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut endpoint = quinn::Endpoint::new(EndpointConfig::default(), None, client_socket)
            .unwrap()
            .0;
        endpoint.set_default_client_config(get_client_config());
        runtime
            .block_on(endpoint.connect(*addr, "localhost").unwrap())
            .unwrap()
    }

    #[test]
    #[ignore]
    fn test_bench_single_connect() {
        solana_logger::setup();
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = unbounded();
        let keypair = Keypair::new();
        let ip = "127.0.0.1".parse().unwrap();
        let server_address = s.local_addr().unwrap();
        let t = spawn_server(s, &keypair, ip, sender, exit.clone(), false).unwrap();

        let mut num_threads = 1;
        let mut num_packets_per_thread = std::env::var("PACKETS_PER_THREAD")
            .map(|x| x.parse().unwrap())
            .unwrap_or(200_000);
        let packet_size = std::env::var("PACKET_SIZE")
            .map(|x| x.parse().unwrap())
            .unwrap_or(200);
        let use_datagram = false;
        for _ in 0..5 {
            let mut send_streams = Measure::start("send_streams");
            let mut received_packets = Measure::start("received_packets");
            let num_packets = num_packets_per_thread * num_threads;
            let client_threads: Vec<_> = (0..num_threads)
                .into_iter()
                .map(|tid| {
                    thread::spawn(move || {
                        let runtime = rt();
                        let _rt_guard = runtime.enter();

                        let conn = Arc::new(make_client_endpoint(&runtime, &server_address));
                        if tid == 0 {
                            info!("max_datagram: {:?}", conn.connection.max_datagram_size());
                        }

                        let batch_size = 10;
                        let num_batches = num_packets_per_thread / batch_size;
                        for _ in 0..num_batches {
                            let packet = vec![0u8; packet_size];
                            let packet_datagram = bytes::Bytes::copy_from_slice(&packet);
                            let conn = conn.clone();
                            let handle = runtime.spawn(async move {
                                for _ in 0..batch_size {
                                    if use_datagram {
                                        info!("sending datagram");
                                        conn.connection.send_datagram(packet_datagram.clone()).unwrap();
                                    } else {
                                        let mut stream = conn.connection.open_uni().await.unwrap();
                                        stream.write_all(&packet).await.unwrap();
                                        stream.finish().await.unwrap();
                                    }
                                }
                            });
                            runtime.block_on(handle).unwrap();
                        }
                        /*for _ in 0..num_packets_per_thread {
                            let mut s1 = runtime.block_on(conn.connection.open_uni()).unwrap();
                            runtime.block_on(s1.write_all(&[0u8; PACKET_SIZE])).unwrap();
                            runtime.block_on(s1.finish()).unwrap();
                        }*/
                    })
                })
                .collect();
            for t in client_threads {
                t.join().unwrap();
            }
            send_streams.stop();

            let mut num_received_packets = 0;
            let mut start = Instant::now();
            loop {
                while let Ok(new) = receiver.recv_timeout(Duration::from_millis(1000)) {
                    num_received_packets += new.packets.len();
                    if num_received_packets >= num_packets {
                        break;
                    }
                }
                if num_received_packets >= num_packets {
                    break;
                }
                if start.elapsed().as_secs() >= 10 {
                    info!(
                        "waiting... received: {} of {}",
                        num_received_packets, num_packets
                    );
                    start = Instant::now();
                }
            }
            received_packets.stop();
            info!(
                "sent {}x {}b packets with {} threads: kpps: {:.2} mbps: {:.2}\n {} {}",
                num_packets,
                packet_size,
                num_threads,
                num_packets as f32 / (1000.0f32 * received_packets.as_s()),
                (num_packets * packet_size) as f32
                    / (1000.0f32 * 1000.0f32 * received_packets.as_s()),
                send_streams,
                received_packets,
            );
            num_threads *= 2;
            num_packets_per_thread /= 2;
        }

        exit.store(true, Ordering::Relaxed);
        let mut after_store = Measure::start("after store");
        t.join().unwrap();
        after_store.stop();
        info!("{}", after_store);
    }

    #[test]
    fn test_quic_server_multiple_streams() {
        solana_logger::setup();
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = unbounded();
        let keypair = Keypair::new();
        let ip = "127.0.0.1".parse().unwrap();
        let server_address = s.local_addr().unwrap();
        let t = spawn_server(s, &keypair, ip, sender, exit.clone(), false).unwrap();

        let runtime = rt();
        let _rt_guard = runtime.enter();
        let conn1 = Arc::new(make_client_endpoint(&runtime, &server_address));
        let conn2 = Arc::new(make_client_endpoint(&runtime, &server_address));
        let mut num_expected_packets = 0;
        for i in 0..10 {
            info!("sending: {}", i);
            let c1 = conn1.clone();
            let c2 = conn2.clone();
            let handle = runtime.spawn(async move {
                let mut s1 = c1.connection.open_uni().await.unwrap();
                let mut s2 = c2.connection.open_uni().await.unwrap();
                s1.write_all(&[0u8]).await.unwrap();
                s1.finish().await.unwrap();
                s2.write_all(&[0u8]).await.unwrap();
                s2.finish().await.unwrap();
            });
            runtime.block_on(handle).unwrap();
            num_expected_packets += 2;
            thread::sleep(Duration::from_millis(200));
        }
        let mut all_packets = vec![];
        let now = Instant::now();
        let mut total_packets = 0;
        while now.elapsed().as_secs() < 10 {
            if let Ok(packets) = receiver.recv_timeout(Duration::from_secs(1)) {
                total_packets += packets.packets.len();
                all_packets.push(packets)
            }
            if total_packets == num_expected_packets {
                break;
            }
        }
        for batch in all_packets {
            for p in &batch.packets {
                assert_eq!(p.meta.size, 1);
            }
        }
        assert_eq!(total_packets, num_expected_packets);

        exit.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }

    #[test]
    fn test_quic_server_multiple_writes() {
        solana_logger::setup();
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = unbounded();
        let keypair = Keypair::new();
        let ip = "127.0.0.1".parse().unwrap();
        let server_address = s.local_addr().unwrap();
        let t = spawn_server(s, &keypair, ip, sender, exit.clone(), false).unwrap();

        let runtime = rt();
        let _rt_guard = runtime.enter();
        let conn1 = Arc::new(make_client_endpoint(&runtime, &server_address));

        // Send a full size packet with single byte writes.
        let num_bytes = PACKET_DATA_SIZE;
        let num_expected_packets = 1;
        let handle = runtime.spawn(async move {
            let mut s1 = conn1.connection.open_uni().await.unwrap();
            for _ in 0..num_bytes {
                s1.write_all(&[0u8]).await.unwrap();
            }
            s1.finish().await.unwrap();
        });
        runtime.block_on(handle).unwrap();

        let mut all_packets = vec![];
        let now = Instant::now();
        let mut total_packets = 0;
        while now.elapsed().as_secs() < 5 {
            if let Ok(packets) = receiver.recv_timeout(Duration::from_secs(1)) {
                total_packets += packets.packets.len();
                all_packets.push(packets)
            }
            if total_packets > num_expected_packets {
                break;
            }
        }
        for batch in all_packets {
            for p in &batch.packets {
                assert_eq!(p.meta.size, num_bytes);
            }
        }
        assert_eq!(total_packets, num_expected_packets);

        exit.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }
}
