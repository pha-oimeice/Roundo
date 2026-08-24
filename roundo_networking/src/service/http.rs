use super::ticket::TicketAuthority;
use super::*;

#[derive(Clone)]
pub(super) struct HttpState {
    pub(super) hooks: Arc<dyn ServerHooks>,
    pub(super) tickets: TicketAuthority,
    pub(super) certificate_pem: String,
}

pub(super) fn http_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/", get(get_server_certificate))
        .route("/server-cert", get(get_server_certificate))
        .route(
            "/public/connection-token",
            post(issue_public_connection_token),
        )
        .with_state(state)
}

async fn get_server_certificate(State(state): State<Arc<HttpState>>) -> String {
    log::debug!("serving TLS certificate through public HTTPS endpoint");
    state.certificate_pem.clone()
}

async fn issue_public_connection_token(
    State(state): State<Arc<HttpState>>,
) -> Result<Json<PublicConnectionTokenResponse>, (StatusCode, String)> {
    let public_session = state
        .hooks
        .public_session()
        .await
        .map_err(internal_server_error)?;
    let ticket = state.tickets.issue(public_session.user_session);
    log::debug!(
        "issued connection ticket: user_id={}, session_id={}",
        public_session.user_session.user_id.0,
        public_session.user_session.session_id.0
    );
    Ok(Json(PublicConnectionTokenResponse {
        user_id: public_session.user_session.user_id.0,
        session_id: public_session.user_session.session_id.0,
        session_name: public_session.session_name,
        connection_token: ticket.connection_token,
        expires_at: ticket.expires_at,
    }))
}

fn internal_server_error(error: String) -> (StatusCode, String) {
    log::error!("public network request failed: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, error)
}

#[derive(Serialize)]
struct PublicConnectionTokenResponse {
    user_id: i32,
    session_id: i32,
    session_name: Option<String>,
    connection_token: String,
    expires_at: u64,
}
