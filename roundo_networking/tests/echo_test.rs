use rcgen::generate_simple_self_signed;
use roundo_networking::echo_test::{EchoTestClientConnection, EchoTestServerConnection, dispatch};
use roundo_networking::schema::{ClientMessage, EchoTestRequest, EchoTestResponse, ServerMessage};
use roundo_networking::tls::{self, TlsServer};
use rustls::pki_types::{PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::sync::Arc;
use tokio::time::{Duration, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);
const ECHO_SEQUENCE: u32 = 358;
const ECHO_MESSAGE: &str = "roundo tls postcard echo";

#[tokio::test]
async fn echo_message_round_trips_after_tls_handshake() {
    timeout(TEST_TIMEOUT, async {
        let (server_config, client_config) = test_tls_configs();
        let server = TlsServer::bind("127.0.0.1:0", server_config)
            .await
            .expect("echo TLS server should bind");
        let server_address = server
            .local_addr()
            .expect("echo TLS server should expose its address");
        let server_task = tokio::spawn(async move {
            let stream = server
                .accept()
                .await
                .expect("echo TLS server should accept the client")
                .handshake()
                .await
                .expect("server TLS handshake should succeed");
            let mut connection = EchoTestServerConnection::new(stream);
            let request = connection
                .receive()
                .await
                .expect("server should receive the postcard request");
            let response = dispatch(request);
            connection
                .send(&response)
                .await
                .expect("server should send the postcard response");
        });

        let stream = tls::connect(
            server_address,
            ServerName::try_from("localhost").expect("test server name should be valid"),
            client_config,
        )
        .await
        .expect("client TLS handshake should succeed");
        let mut connection = EchoTestClientConnection::new(stream);
        let request = ClientMessage::EchoTest(EchoTestRequest {
            sequence: ECHO_SEQUENCE,
            message: ECHO_MESSAGE.to_string(),
        });
        connection
            .send(&request)
            .await
            .expect("client should send the postcard request");

        let response = connection
            .receive()
            .await
            .expect("client should receive the postcard response");
        assert_eq!(
            response,
            ServerMessage::EchoTest(EchoTestResponse {
                sequence: ECHO_SEQUENCE,
                message: ECHO_MESSAGE.to_string(),
            })
        );
        server_task
            .await
            .expect("echo TLS server task should not panic");
    })
    .await
    .expect("echo exchange should finish before the test timeout");
}

fn test_tls_configs() -> (Arc<ServerConfig>, Arc<ClientConfig>) {
    let certificate = generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("test certificate should generate");
    let certificate_der = certificate.cert.der().clone();
    let private_key_der = PrivateKeyDer::Pkcs8(certificate.signing_key.serialize_der().into());
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate_der.clone()], private_key_der)
        .expect("test TLS server config should build");
    let mut roots = RootCertStore::empty();
    roots
        .add(certificate_der)
        .expect("test certificate should enter the root store");
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (Arc::new(server_config), Arc::new(client_config))
}
