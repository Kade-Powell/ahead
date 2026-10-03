//! Direct, certificate-pinned connection to an active AHEAD session host.

use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    net::{
        IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket,
    },
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use ahead_rpc::ahead::{
    AheadRequest, CodeComment, ConversationMessage, DisplayRange, GitHubUser,
    Participant, SessionRole, SessionView, SharedBufferEditResult,
    SharedBufferSnapshot, SharedPresence, SharedSessionOffer,
};
use ahead_rpc::proxy::ProxyRpcHandler;
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use parking_lot::{Mutex, RwLock};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection,
    StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
};
use serde::{Deserialize, Serialize};

use super::{auth::GitHubAuthManager, host::AheadSessionHost};

const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum ClientFrame {
    Join {
        session_id: String,
        token: String,
    },
    Poll {
        after_sequence: i64,
        active_path: Option<String>,
        active_line: Option<u32>,
    },
    PostHumanMessage {
        content: String,
    },
    StartAgentTurn {
        content: String,
    },
    CreateCodeComment {
        path: String,
        range: DisplayRange,
        quote: String,
        source_sha256: String,
        body: String,
    },
    ResolveCodeComment {
        comment_id: String,
    },
    ListCodeComments,
    ReadBuffer {
        path: String,
    },
    ReplaceBuffer {
        path: String,
        expected_revision: u64,
        content: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum ServerFrame {
    Joined {
        view: SessionView,
        messages: Vec<ConversationMessage>,
        code_comments: Vec<CodeComment>,
        terminal_output: String,
        presence: Vec<SharedPresence>,
        actor_id: String,
    },
    Messages {
        view: SessionView,
        messages: Vec<ConversationMessage>,
        code_comments: Vec<CodeComment>,
        terminal_output: String,
        presence: Vec<SharedPresence>,
    },
    MessagePosted {
        message: ConversationMessage,
    },
    TurnStarted {
        turn_id: String,
    },
    CodeComments {
        comments: Vec<CodeComment>,
    },
    CodeCommentSaved {
        comment: CodeComment,
    },
    BufferRead {
        snapshot: SharedBufferSnapshot,
    },
    BufferEdited {
        result: SharedBufferEditResult,
    },
    Error {
        message: String,
    },
}

fn write_frame(stream: &mut impl Write, frame: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(frame)?;
    anyhow::ensure!(bytes.len() <= MAX_FRAME_BYTES, "Share frame is too large");
    stream.write_all(&u32::try_from(bytes.len())?.to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut impl Read) -> Result<T> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    anyhow::ensure!(length <= MAX_FRAME_BYTES, "Share frame is too large");
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn github_user(token: &str, api_base: &str, path: &str) -> Result<GitHubUser> {
    anyhow::ensure!(
        !token.is_empty() && token.len() <= 4096,
        "Invalid GitHub token"
    );
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()?
        .get(format!("{api_base}/{path}"))
        .header(reqwest::header::USER_AGENT, "AHEAD collaboration")
        .bearer_auth(token)
        .send()?
        .error_for_status()
        .context("GitHub did not accept this identity")?;
    let mut body = String::new();
    response.take(64 * 1024).read_to_string(&mut body)?;
    let user = GitHubAuthManager::parse_user_profile(&body)?;
    anyhow::ensure!(user.id != 0, "GitHub identity is invalid");
    Ok(user)
}

fn verified_github_user(token: &str, api_base: &str) -> Result<GitHubUser> {
    github_user(token, api_base, "user")
}

pub(crate) fn resolve_github_user(token: &str, handle: &str) -> Result<GitHubUser> {
    let handle = handle.trim().trim_start_matches('@');
    anyhow::ensure!(
        !handle.is_empty()
            && handle.len() <= 64
            && handle
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && handle
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && handle
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "Enter a GitHub username"
    );
    let user =
        github_user(token, "https://api.github.com", &format!("users/{handle}"))?;
    anyhow::ensure!(
        user.login.eq_ignore_ascii_case(handle),
        "GitHub returned a different user"
    );
    Ok(user)
}

fn member_view(
    host: &AheadSessionHost,
    session_id: &str,
    user: &GitHubUser,
) -> Result<SessionView> {
    let view = host.shareable_session(session_id)?;
    host.ensure_team_member(&user.login)?;
    anyhow::ensure!(
        view.participants.iter().any(|record| {
            matches!(&record.participant, Participant::Human { id, subject, .. }
                if id.eq_ignore_ascii_case(&user.login)
                    && subject == &format!("github:{}", user.id))
        }),
        "This GitHub user is not a participant in the session"
    );
    Ok(view)
}

fn presence_snapshot(
    owner: &Arc<Mutex<SharedPresence>>,
    peers: &Arc<Mutex<HashMap<SocketAddr, SharedPresence>>>,
) -> Vec<SharedPresence> {
    let mut by_actor = HashMap::new();
    let owner = owner.lock().clone();
    by_actor.insert(owner.actor_id.clone(), owner);
    for presence in peers.lock().values() {
        by_actor.insert(presence.actor_id.clone(), presence.clone());
    }
    let mut result: Vec<_> = by_actor.into_values().collect();
    result.sort_by(|left, right| left.actor_id.cmp(&right.actor_id));
    result
}

fn serve_peer(
    socket: TcpStream,
    config: Arc<ServerConfig>,
    host: Arc<RwLock<AheadSessionHost>>,
    proxy_rpc: ProxyRpcHandler,
    session_id: String,
    api_base: String,
    owner_presence: Arc<Mutex<SharedPresence>>,
    peer_presence: Arc<Mutex<HashMap<SocketAddr, SharedPresence>>>,
) -> Result<()> {
    let peer_address = socket.peer_addr()?;
    let connection = ServerConnection::new(config)?;
    let mut stream = StreamOwned::new(connection, socket);
    let result = (|| -> Result<()> {
        let ClientFrame::Join {
            session_id: requested_session,
            token,
        } = read_frame(&mut stream)?
        else {
            bail!("Join before sending session commands");
        };
        anyhow::ensure!(requested_session == session_id, "Wrong shared session");
        let user = verified_github_user(&token, &api_base)?;
        let view = member_view(&host.read(), &session_id, &user)?;
        peer_presence.lock().insert(
            peer_address,
            SharedPresence {
                actor_id: user.login.clone(),
                path: None,
                line: None,
            },
        );
        let messages = host.read().shared_messages_after(&session_id, 0)?;
        let code_comments = host.read().shared_code_comments(&session_id)?;
        let terminal_output = host.read().shared_terminal_output(&session_id)?;
        write_frame(
            &mut stream,
            &ServerFrame::Joined {
                view,
                messages,
                code_comments,
                terminal_output,
                presence: presence_snapshot(&owner_presence, &peer_presence),
                actor_id: user.login.clone(),
            },
        )?;
        stream.sock.set_read_timeout(None)?;
        loop {
            let command = match read_frame::<ClientFrame>(&mut stream) {
                Ok(command) => command,
                Err(error)
                    if error.downcast_ref::<std::io::Error>().is_some_and(
                        |error| {
                            matches!(
                                error.kind(),
                                ErrorKind::UnexpectedEof
                                    | ErrorKind::ConnectionReset
                            )
                        },
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };
            let view = member_view(&host.read(), &session_id, &user)?;
            match command {
                ClientFrame::Join { .. } => bail!("Already joined"),
                ClientFrame::Poll {
                    after_sequence,
                    active_path,
                    active_line,
                } => {
                    let current_path = peer_presence
                        .lock()
                        .get(&peer_address)
                        .and_then(|presence| presence.path.clone());
                    if current_path != active_path {
                        let valid_path = active_path.filter(|path| {
                            path.len() <= 1024
                                && proxy_rpc
                                    .ahead_request_blocking(
                                        AheadRequest::ReadSharedBuffer {
                                            session_id: session_id.clone(),
                                            path: path.clone(),
                                        },
                                    )
                                    .is_ok()
                        });
                        if let Some(presence) =
                            peer_presence.lock().get_mut(&peer_address)
                        {
                            presence.path = valid_path;
                            presence.line = presence.path.as_ref().and(active_line);
                        }
                    } else if let Some(presence) =
                        peer_presence.lock().get_mut(&peer_address)
                    {
                        presence.line = presence.path.as_ref().and(active_line);
                    }
                    let messages = host
                        .read()
                        .shared_messages_after(&session_id, after_sequence)?;
                    let code_comments =
                        host.read().shared_code_comments(&session_id)?;
                    let terminal_output =
                        host.read().shared_terminal_output(&session_id)?;
                    write_frame(
                        &mut stream,
                        &ServerFrame::Messages {
                            view,
                            messages,
                            code_comments,
                            terminal_output,
                            presence: presence_snapshot(
                                &owner_presence,
                                &peer_presence,
                            ),
                        },
                    )?;
                }
                ClientFrame::PostHumanMessage { content } => {
                    let message = host.read().post_human_message_as(
                        &session_id,
                        &content,
                        &user.login,
                        Some(user.id),
                    )?;
                    write_frame(
                        &mut stream,
                        &ServerFrame::MessagePosted { message },
                    )?;
                }
                ClientFrame::StartAgentTurn { content } => {
                    let turn_id = host.read().start_shared_agent_turn(
                        &session_id,
                        content,
                        &user.login,
                        user.id,
                    )?;
                    write_frame(&mut stream, &ServerFrame::TurnStarted { turn_id })?;
                }
                ClientFrame::ListCodeComments => {
                    let comments = host.read().shared_code_comments(&session_id)?;
                    write_frame(
                        &mut stream,
                        &ServerFrame::CodeComments { comments },
                    )?;
                }
                ClientFrame::CreateCodeComment {
                    path,
                    range,
                    quote,
                    source_sha256,
                    body,
                } => {
                    let comment = host.read().create_code_comment_as(
                        &session_id,
                        path,
                        range,
                        quote,
                        source_sha256,
                        body,
                        &user.login,
                        user.id,
                    )?;
                    write_frame(
                        &mut stream,
                        &ServerFrame::CodeCommentSaved { comment },
                    )?;
                }
                ClientFrame::ResolveCodeComment { comment_id } => {
                    let comment = host.read().resolve_code_comment_as(
                        &session_id,
                        &comment_id,
                        &user.login,
                        user.id,
                    )?;
                    write_frame(
                        &mut stream,
                        &ServerFrame::CodeCommentSaved { comment },
                    )?;
                }
                ClientFrame::ReadBuffer { path } => {
                    let response = proxy_rpc
                        .ahead_request_blocking(AheadRequest::ReadSharedBuffer {
                            session_id: session_id.clone(),
                            path,
                        })
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    let snapshot: SharedBufferSnapshot =
                        serde_json::from_value(response)?;
                    write_frame(&mut stream, &ServerFrame::BufferRead { snapshot })?;
                }
                ClientFrame::ReplaceBuffer {
                    path,
                    expected_revision,
                    content,
                } => {
                    anyhow::ensure!(view.participants.iter().any(|record| {
                        matches!(&record.participant, Participant::Human { id, subject, .. }
                            if id.eq_ignore_ascii_case(&user.login)
                                && subject == &format!("github:{}", user.id))
                            && matches!(record.role, SessionRole::Owner | SessionRole::Editor)
                    }), "This participant cannot edit shared buffers");
                    let response = proxy_rpc
                        .ahead_request_blocking(AheadRequest::ReplaceSharedBuffer {
                            session_id: session_id.clone(),
                            path,
                            expected_revision,
                            content,
                        })
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    let result: SharedBufferEditResult =
                        serde_json::from_value(response)?;
                    write_frame(&mut stream, &ServerFrame::BufferEdited { result })?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = &result {
        if let Err(write_error) = write_frame(
            &mut stream,
            &ServerFrame::Error {
                message: error.to_string(),
            },
        ) {
            tracing::debug!(?write_error, "could not send share error");
        }
    }
    result
}

pub(crate) struct SharedSessionServer {
    offer: SharedSessionOffer,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<HashMap<SocketAddr, TcpStream>>>,
    owner_presence: Arc<Mutex<SharedPresence>>,
    peer_presence: Arc<Mutex<HashMap<SocketAddr, SharedPresence>>>,
    thread: Option<JoinHandle<()>>,
}

impl SharedSessionServer {
    pub(crate) fn start(
        host: Arc<RwLock<AheadSessionHost>>,
        proxy_rpc: ProxyRpcHandler,
        session_id: String,
        bind_address: &str,
    ) -> Result<Self> {
        Self::start_with_api(
            host,
            proxy_rpc,
            session_id,
            bind_address,
            "https://api.github.com",
        )
    }

    pub(crate) fn start_with_api(
        host: Arc<RwLock<AheadSessionHost>>,
        proxy_rpc: ProxyRpcHandler,
        session_id: String,
        bind_address: &str,
        api_base: &str,
    ) -> Result<Self> {
        host.read().can_host_share(&session_id)?;
        host.read().bind_owner_identity(&session_id)?;
        let owner_actor_id = host
            .read()
            .shareable_session(&session_id)?
            .participants
            .iter()
            .find_map(|record| {
                (record.role == SessionRole::Owner)
                    .then(|| {
                        if let Participant::Human { id, .. } = &record.participant {
                            Some(id.clone())
                        } else {
                            None
                        }
                    })
                    .flatten()
            })
            .context("Shared session owner is missing")?;
        let listener = TcpListener::bind(bind_address)?;
        listener.set_nonblocking(true)?;
        let key = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
        let certificate = key.cert.der().clone();
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            key.signing_key.serialize_der(),
        ));
        let config = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], private_key)?,
        );
        let bound = listener.local_addr()?;
        let address = if bound.ip().is_unspecified() {
            // ponytail: select the default route; let the user edit the invite
            // address when a VPN or second interface is the reachable route.
            let selected_ip = UdpSocket::bind("0.0.0.0:0")
                .and_then(|route| {
                    route.connect("192.0.2.1:80")?;
                    route.local_addr()
                })
                .map(|address| address.ip())
                .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
            SocketAddr::new(selected_ip, bound.port())
        } else {
            bound
        };
        let offer = SharedSessionOffer {
            session_id: session_id.clone(),
            address: address.to_string(),
            certificate: STANDARD.encode(certificate.as_ref()),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(HashMap::new()));
        let owner_presence = Arc::new(Mutex::new(SharedPresence {
            actor_id: owner_actor_id,
            path: None,
            line: None,
        }));
        let peer_presence = Arc::new(Mutex::new(HashMap::new()));
        let thread = {
            let stop = stop.clone();
            let sockets = sockets.clone();
            let owner_presence = owner_presence.clone();
            let peer_presence = peer_presence.clone();
            let api_base = api_base.to_string();
            thread::spawn(move || {
                let mut peers: Vec<JoinHandle<()>> = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    let mut index = 0;
                    while index < peers.len() {
                        if peers[index].is_finished() {
                            let peer: JoinHandle<()> = peers.swap_remove(index);
                            if peer.join().is_err() {
                                tracing::warn!("share peer thread panicked");
                            }
                        } else {
                            index += 1;
                        }
                    }
                    match listener.accept() {
                        Ok((socket, peer_address)) => {
                            if sockets.lock().len() >= 32 {
                                tracing::warn!("share connection limit reached");
                                continue;
                            }
                            if let Err(error) = socket.set_nonblocking(false) {
                                tracing::warn!(?error, "share socket setup failed");
                                continue;
                            }
                            if let Err(error) = socket
                                .set_read_timeout(Some(Duration::from_secs(30)))
                            {
                                tracing::warn!(?error, "share socket setup failed");
                                continue;
                            }
                            if let Err(error) = socket
                                .set_write_timeout(Some(Duration::from_secs(10)))
                            {
                                tracing::warn!(?error, "share socket setup failed");
                                continue;
                            }
                            if let Err(error) = socket.set_nodelay(true) {
                                tracing::warn!(?error, "share socket setup failed");
                                continue;
                            }
                            match socket.try_clone() {
                                Ok(cancel_socket) => {
                                    sockets
                                        .lock()
                                        .insert(peer_address, cancel_socket);
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        ?error,
                                        "share socket clone failed"
                                    );
                                    continue;
                                }
                            }
                            let host = host.clone();
                            let proxy_rpc = proxy_rpc.clone();
                            let config = config.clone();
                            let session_id = session_id.clone();
                            let api_base = api_base.clone();
                            let sockets = sockets.clone();
                            let owner_presence = owner_presence.clone();
                            let peer_presence = peer_presence.clone();
                            peers.push(thread::spawn(move || {
                                if let Err(error) = serve_peer(
                                    socket,
                                    config,
                                    host,
                                    proxy_rpc,
                                    session_id,
                                    api_base,
                                    owner_presence,
                                    peer_presence.clone(),
                                ) {
                                    tracing::debug!(
                                        ?error,
                                        "share peer disconnected"
                                    );
                                }
                                sockets.lock().remove(&peer_address);
                                peer_presence.lock().remove(&peer_address);
                            }));
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(25));
                        }
                        Err(error) => {
                            tracing::warn!(?error, "share listener failed");
                            break;
                        }
                    }
                }
                for socket in sockets.lock().values() {
                    if let Err(error) = socket.shutdown(Shutdown::Both) {
                        tracing::debug!(?error, "share socket already closed");
                    }
                }
                for peer in peers {
                    if peer.join().is_err() {
                        tracing::warn!("share peer thread panicked");
                    }
                }
            })
        };
        Ok(Self {
            offer,
            stop,
            sockets,
            owner_presence,
            peer_presence,
            thread: Some(thread),
        })
    }

    pub(crate) fn offer(&self) -> &SharedSessionOffer {
        &self.offer
    }

    pub(crate) fn set_owner_location(
        &self,
        path: Option<String>,
        line: Option<u32>,
    ) {
        let mut presence = self.owner_presence.lock();
        presence.line = path.as_ref().and(line);
        presence.path = path;
    }

    pub(crate) fn presence(&self) -> Vec<SharedPresence> {
        presence_snapshot(&self.owner_presence, &self.peer_presence)
    }

    pub(crate) fn revoke_actor(&self, actor_id: &str) {
        let addresses: Vec<_> = self
            .peer_presence
            .lock()
            .iter()
            .filter_map(|(address, presence)| {
                presence
                    .actor_id
                    .eq_ignore_ascii_case(actor_id)
                    .then_some(*address)
            })
            .collect();
        for address in addresses {
            self.peer_presence.lock().remove(&address);
            if let Some(socket) = self.sockets.lock().get(&address)
                && let Err(error) = socket.shutdown(Shutdown::Both)
            {
                tracing::debug!(?error, "share socket already closed");
            }
        }
    }
}

impl Drop for SharedSessionServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for socket in self.sockets.lock().values() {
            if let Err(error) = socket.shutdown(Shutdown::Both) {
                tracing::debug!(?error, "share socket already closed");
            }
        }
        if let Some(thread) = self.thread.take() {
            // A peer may be waiting for a proxy RPC; the dispatcher must stay
            // free to answer it while the listener drains its peer threads.
            if let Err(error) = thread::Builder::new()
                .name("ahead-share-cleanup".into())
                .spawn(move || {
                    if thread.join().is_err() {
                        tracing::warn!("share listener thread panicked");
                    }
                })
            {
                tracing::warn!(?error, "could not wait for share listener cleanup");
            }
        }
    }
}

pub(crate) struct SharedSessionClient {
    offer: SharedSessionOffer,
    token: String,
    session_id: String,
    actor_id: String,
    stream: StreamOwned<ClientConnection, TcpStream>,
}

impl SharedSessionClient {
    pub(crate) fn connect(
        offer: &SharedSessionOffer,
        token: &str,
    ) -> Result<(
        Self,
        SessionView,
        Vec<ConversationMessage>,
        Vec<CodeComment>,
        String,
        Vec<SharedPresence>,
    )> {
        anyhow::ensure!(
            offer.certificate.len() <= 16 * 1024,
            "Invalid share certificate"
        );
        let certificate = CertificateDer::from(STANDARD.decode(&offer.certificate)?);
        let mut roots = RootCertStore::empty();
        roots.add(certificate)?;
        let config = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
        );
        let address: SocketAddr =
            offer.address.parse().context("Invalid share address")?;
        let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(10)))?;
        socket.set_write_timeout(Some(Duration::from_secs(10)))?;
        let connection =
            ClientConnection::new(config, ServerName::try_from("localhost")?)?;
        let mut client = Self {
            offer: offer.clone(),
            token: token.to_string(),
            session_id: offer.session_id.clone(),
            actor_id: String::new(),
            stream: StreamOwned::new(connection, socket),
        };
        write_frame(
            &mut client.stream,
            &ClientFrame::Join {
                session_id: offer.session_id.clone(),
                token: token.to_string(),
            },
        )?;
        match read_frame(&mut client.stream)? {
            ServerFrame::Joined {
                view,
                messages,
                code_comments,
                terminal_output,
                presence,
                actor_id,
            } => {
                client.actor_id = actor_id;
                Ok((
                    client,
                    view,
                    messages,
                    code_comments,
                    terminal_output,
                    presence,
                ))
            }
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected share join response"),
        }
    }

    pub(crate) fn poll(
        &mut self,
        after_sequence: i64,
        active_path: Option<String>,
        active_line: Option<u32>,
    ) -> Result<(
        SessionView,
        Vec<ConversationMessage>,
        Vec<CodeComment>,
        String,
        Vec<SharedPresence>,
    )> {
        match self.poll_once(after_sequence, active_path.clone(), active_line) {
            Ok(update) => Ok(update),
            Err(error)
                if error.downcast_ref::<std::io::Error>().is_some_and(
                    |io_error| {
                        matches!(
                            io_error.kind(),
                            ErrorKind::UnexpectedEof
                                | ErrorKind::ConnectionReset
                                | ErrorKind::ConnectionAborted
                                | ErrorKind::BrokenPipe
                                | ErrorKind::NotConnected
                                | ErrorKind::TimedOut
                        )
                    },
                ) =>
            {
                let (replacement, _, _, _, _, _) =
                    Self::connect(&self.offer, &self.token)?;
                anyhow::ensure!(
                    replacement.actor_id == self.actor_id,
                    "Shared session identity changed"
                );
                *self = replacement;
                self.poll_once(after_sequence, active_path, active_line)
            }
            Err(error) => Err(error),
        }
    }

    fn poll_once(
        &mut self,
        after_sequence: i64,
        active_path: Option<String>,
        active_line: Option<u32>,
    ) -> Result<(
        SessionView,
        Vec<ConversationMessage>,
        Vec<CodeComment>,
        String,
        Vec<SharedPresence>,
    )> {
        write_frame(
            &mut self.stream,
            &ClientFrame::Poll {
                after_sequence,
                active_path,
                active_line,
            },
        )?;
        match read_frame(&mut self.stream)? {
            ServerFrame::Messages {
                view,
                messages,
                code_comments,
                terminal_output,
                presence,
            } => Ok((view, messages, code_comments, terminal_output, presence)),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected share poll response"),
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn actor_id(&self) -> &str {
        &self.actor_id
    }

    pub(crate) fn post_human_message(
        &mut self,
        content: String,
    ) -> Result<ConversationMessage> {
        write_frame(&mut self.stream, &ClientFrame::PostHumanMessage { content })?;
        match read_frame(&mut self.stream)? {
            ServerFrame::MessagePosted { message } => Ok(message),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected share message response"),
        }
    }

    pub(crate) fn start_agent_turn(&mut self, content: String) -> Result<String> {
        write_frame(&mut self.stream, &ClientFrame::StartAgentTurn { content })?;
        match read_frame(&mut self.stream)? {
            ServerFrame::TurnStarted { turn_id } => Ok(turn_id),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected share turn response"),
        }
    }

    pub(crate) fn list_code_comments(&mut self) -> Result<Vec<CodeComment>> {
        write_frame(&mut self.stream, &ClientFrame::ListCodeComments)?;
        match read_frame(&mut self.stream)? {
            ServerFrame::CodeComments { comments } => Ok(comments),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected shared code comments response"),
        }
    }

    pub(crate) fn create_code_comment(
        &mut self,
        path: String,
        range: DisplayRange,
        quote: String,
        source_sha256: String,
        body: String,
    ) -> Result<CodeComment> {
        write_frame(
            &mut self.stream,
            &ClientFrame::CreateCodeComment {
                path,
                range,
                quote,
                source_sha256,
                body,
            },
        )?;
        self.read_saved_comment()
    }

    pub(crate) fn resolve_code_comment(
        &mut self,
        comment_id: String,
    ) -> Result<CodeComment> {
        write_frame(
            &mut self.stream,
            &ClientFrame::ResolveCodeComment { comment_id },
        )?;
        self.read_saved_comment()
    }

    fn read_saved_comment(&mut self) -> Result<CodeComment> {
        match read_frame(&mut self.stream)? {
            ServerFrame::CodeCommentSaved { comment } => Ok(comment),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected shared code comment response"),
        }
    }

    pub(crate) fn read_buffer(
        &mut self,
        path: String,
    ) -> Result<SharedBufferSnapshot> {
        write_frame(&mut self.stream, &ClientFrame::ReadBuffer { path })?;
        match read_frame(&mut self.stream)? {
            ServerFrame::BufferRead { snapshot } => Ok(snapshot),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected shared buffer response"),
        }
    }

    pub(crate) fn replace_buffer(
        &mut self,
        path: String,
        expected_revision: u64,
        content: String,
    ) -> Result<SharedBufferEditResult> {
        write_frame(
            &mut self.stream,
            &ClientFrame::ReplaceBuffer {
                path,
                expected_revision,
                content,
            },
        )?;
        match read_frame(&mut self.stream)? {
            ServerFrame::BufferEdited { result } => Ok(result),
            ServerFrame::Error { message } => bail!("{message}"),
            _ => bail!("Unexpected shared edit response"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahead_rpc::ahead::{
        DisplayPosition, SessionRole, SharedSessionUpdate, WorkKind,
    };
    use ahead_rpc::core::CoreRpcHandler;
    use ahead_rpc::file::{EditorRecoverySnapshot, EditorRecoverySummary};
    use ahead_rpc::proxy::{ProxyHandler, ProxyNotification};
    use sha2::Digest;

    #[test]
    fn share_protocol_has_no_terminal_control_command() {
        for command in ["terminal_input", "terminal_resize", "terminal_close"] {
            assert!(
                serde_json::from_value::<ClientFrame>(serde_json::json!({
                    "type": command,
                    "data": { "bytes": "exit\\n" }
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn direct_tls_clients_join_only_as_session_members_and_post_human_chat()
    -> Result<()> {
        let api = TcpListener::bind("127.0.0.1:0")?;
        let api_address = api.local_addr()?;
        let api_thread = thread::spawn(move || -> Result<()> {
            for _ in 0..9 {
                let (mut socket, _) = api.accept()?;
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") && headers.len() < 8192 {
                    let mut byte = [0];
                    socket.read_exact(&mut byte)?;
                    headers.push(byte[0]);
                }
                let headers = String::from_utf8_lossy(&headers).to_ascii_lowercase();
                let body = if headers.contains("authorization: bearer client-token")
                {
                    r#"{"login":"bob","id":42,"name":"Bob"}"#
                } else if headers.contains("authorization: bearer reviewer-token") {
                    r#"{"login":"carol","id":43,"name":"Carol"}"#
                } else {
                    r#"{"login":"mallory","id":99,"name":"Mallory"}"#
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )?;
            }
            Ok(())
        });

        let host = Arc::new(RwLock::new(AheadSessionHost::in_memory()?));
        let temp = tempfile::tempdir()?;
        std::fs::create_dir(temp.path().join(".ahead"))?;
        let team_path = temp.path().join(".ahead/team.toml");
        let team_manifest = "[[members]]\ngithub = 'bob'\ndisplay_name = 'Bob'\nrole = 'editor'\n\
             [[members]]\ngithub = 'carol'\ndisplay_name = 'Carol'\nrole = 'reviewer'\n";
        std::fs::write(&team_path, team_manifest)?;
        std::fs::write(temp.path().join("src.rs"), "fn main() {}\n")?;
        host.read().set_workspace(temp.path().to_path_buf());
        let mut auth = GitHubAuthManager::with_custom_dir(temp.path().join("auth"));
        auth.save_auth(
            "host-token".into(),
            GitHubUser {
                login: "host".into(),
                id: 1,
                name: None,
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
        )?;
        host.read().set_auth_for_test(auth);
        host.read()
            .add_workspace_participant("bob".into(), SessionRole::Editor)?;
        host.read()
            .add_workspace_participant("carol".into(), SessionRole::Reviewer)?;
        let view = host.read().start_work(
            Some(WorkKind::ProductChange),
            "Shared task".into(),
            "Discuss".into(),
            None,
        )?;
        host.read().add_session_participant_verified(
            &view.session.id,
            GitHubUser {
                login: "bob".into(),
                id: 42,
                name: Some("Bob".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
            SessionRole::Editor,
        )?;
        host.read().add_session_participant_verified(
            &view.session.id,
            GitHubUser {
                login: "carol".into(),
                id: 43,
                name: Some("Carol".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
            SessionRole::Reviewer,
        )?;
        let proxy_rpc = ProxyRpcHandler::new();
        let mut dispatcher = crate::dispatch::Dispatcher::new(
            CoreRpcHandler::new(),
            proxy_rpc.clone(),
        );
        dispatcher.handle_notification(ProxyNotification::Initialize {
            workspace: Some(temp.path().to_path_buf()),
            window_id: 0,
            tab_id: 0,
        });
        dispatcher.ahead_host = Some(host.clone());
        let dispatcher_rpc = proxy_rpc.clone();
        let dispatcher_thread = thread::spawn(move || {
            dispatcher_rpc.mainloop(&mut dispatcher);
        });
        let server = SharedSessionServer::start_with_api(
            host.clone(),
            proxy_rpc.clone(),
            view.session.id.clone(),
            "127.0.0.1:0",
            &format!("http://{api_address}"),
        )?;
        let offer = server.offer().clone();
        let (
            mut bob,
            joined,
            initial,
            initial_comments,
            terminal_output,
            initial_presence,
        ) = SharedSessionClient::connect(&offer, "client-token")?;
        assert_eq!(joined.session.id, view.session.id);
        assert!(initial.is_empty());
        assert!(initial_comments.is_empty());
        assert!(terminal_output.is_empty());
        assert!(
            initial_presence
                .iter()
                .any(|presence| presence.actor_id == "bob")
        );
        server.set_owner_location(Some("src.rs".into()), Some(0));
        host.read().publish_shared_terminal(
            &view.session.id,
            "shell\nhello from host".into(),
        )?;
        let (_, _, _, terminal_output, _) =
            bob.poll(0, Some("src.rs".into()), Some(2))?;
        assert_eq!(terminal_output, "shell\nhello from host");
        std::fs::write(
            &team_path,
            "[[members]]\ngithub = 'carol'\ndisplay_name = 'Carol'\nrole = 'reviewer'\n",
        )?;
        let error = bob
            .poll(0, None, None)
            .err()
            .context("removed team member should lose the live connection")?;
        assert!(error.to_string().contains(".ahead/team.toml"));
        let error = SharedSessionClient::connect(&offer, "client-token")
            .err()
            .context("removed team member should not reconnect")?;
        assert!(error.to_string().contains(".ahead/team.toml"));
        std::fs::write(&team_path, team_manifest)?;
        bob.poll(0, None, None)?;
        let message = bob.post_human_message("@carol please review".into())?;
        assert_eq!(message.actor_id, "bob");
        assert_eq!(message.human_recipient_ids(), Some(vec!["carol"]));
        let (_, messages, _, _, presence) =
            bob.poll(0, Some("src.rs".into()), Some(2))?;
        assert!(presence.iter().any(|presence| presence.actor_id == "bob"
            && presence.path.as_deref() == Some("src.rs")));
        assert!(
            presence
                .iter()
                .any(|presence| presence.actor_id == "bob"
                    && presence.line == Some(2))
        );
        assert!(presence.iter().any(|presence| presence.actor_id != "bob"
            && presence.path.as_deref() == Some("src.rs")));
        let (_, _, _, _, private_presence) =
            bob.poll(0, Some(".ahead/team.toml".into()), Some(4))?;
        assert!(
            private_presence
                .iter()
                .any(|presence| presence.actor_id == "bob"
                    && presence.path.is_none()
                    && presence.line.is_none())
        );
        let message_sequence = message.sequence;
        assert_eq!(messages, vec![message]);
        for socket in server.sockets.lock().values() {
            socket.shutdown(Shutdown::Both)?;
        }
        let (_, reconnected_messages, _, _, _) =
            bob.poll(message_sequence, Some("src.rs".into()), Some(2))?;
        assert!(reconnected_messages.is_empty());
        let source = bob.read_buffer("src.rs".into())?;
        let edit = bob.replace_buffer(
            "src.rs".into(),
            source.revision,
            "fn shared() {}\n".into(),
        )?;
        assert!(edit.applied);
        assert_eq!(edit.snapshot.content, "fn shared() {}\n");
        let stale = bob.replace_buffer(
            "src.rs".into(),
            source.revision,
            "fn stale() {}\n".into(),
        )?;
        assert!(!stale.applied);
        assert_eq!(stale.snapshot, edit.snapshot);
        let comment = bob.create_code_comment(
            "src.rs".into(),
            DisplayRange {
                start: DisplayPosition { line: 0, col: 0 },
                end: DisplayPosition { line: 0, col: 2 },
            },
            "fn".into(),
            "0".repeat(64),
            "Please review".into(),
        )?;
        assert_eq!(comment.actor_id, "bob");
        assert_eq!(bob.list_code_comments()?, vec![comment.clone()]);
        let resolved = bob.resolve_code_comment(comment.id.clone())?;
        assert_eq!(resolved.resolved_by.as_deref(), Some("bob"));
        let (mut reviewer, _, _, _, _, _) =
            SharedSessionClient::connect(&offer, "reviewer-token")?;
        let reviewer_source = reviewer.read_buffer("src.rs".into())?;
        assert!(
            reviewer
                .replace_buffer(
                    "src.rs".into(),
                    reviewer_source.revision,
                    "not permitted\n".into()
                )
                .is_err()
        );
        host.read().add_session_participant_verified(
            &view.session.id,
            GitHubUser {
                login: "carol".into(),
                id: 43,
                name: Some("Carol".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
            SessionRole::Editor,
        )?;
        let (mut reviewer, _, _, _, _, _) =
            SharedSessionClient::connect(&offer, "reviewer-token")?;
        let latest = reviewer.read_buffer("src.rs".into())?;
        let bob_edit = bob.replace_buffer(
            "src.rs".into(),
            latest.revision,
            "fn bob() {}\n".into(),
        )?;
        assert!(bob_edit.applied);
        let stale = reviewer.replace_buffer(
            "src.rs".into(),
            latest.revision,
            "fn carol() {}\n".into(),
        )?;
        assert!(!stale.applied);
        assert_eq!(stale.snapshot, bob_edit.snapshot);
        let carol_edit = reviewer.replace_buffer(
            "src.rs".into(),
            stale.snapshot.revision,
            "fn carol() {}\n".into(),
        )?;
        assert!(carol_edit.applied);
        assert_eq!(bob.read_buffer("src.rs".into())?, carol_edit.snapshot);
        host.read()
            .publish_shared_terminal(&view.session.id, String::new())?;
        let (_, _, _, terminal_output, _) = reviewer.poll(0, None, None)?;
        assert!(terminal_output.is_empty());
        let guest_workspace = tempfile::tempdir()?;
        std::fs::create_dir(guest_workspace.path().join(".ahead"))?;
        let guest_database = guest_workspace.path().join(".ahead/session.db");
        let guest_host = Arc::new(RwLock::new(AheadSessionHost::new(
            crate::ahead::store::SessionStore::open(&guest_database)?,
        )));
        guest_host
            .read()
            .set_workspace(guest_workspace.path().to_path_buf());
        let mut guest_auth =
            GitHubAuthManager::with_custom_dir(guest_workspace.path().join("auth"));
        guest_auth.save_auth(
            "client-token".into(),
            GitHubUser {
                login: "bob".into(),
                id: 42,
                name: Some("Bob".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
        )?;
        guest_host.read().set_auth_for_test(guest_auth);
        let guest_rpc = ProxyRpcHandler::new();
        let mut guest_dispatcher = crate::dispatch::Dispatcher::new(
            CoreRpcHandler::new(),
            guest_rpc.clone(),
        );
        guest_dispatcher.handle_notification(ProxyNotification::Initialize {
            workspace: Some(guest_workspace.path().to_path_buf()),
            window_id: 1,
            tab_id: 1,
        });
        guest_dispatcher.ahead_host = Some(guest_host);
        let guest_dispatcher_rpc = guest_rpc.clone();
        let guest_thread = thread::spawn(move || {
            guest_dispatcher_rpc.mainloop(&mut guest_dispatcher);
        });
        let joined: SharedSessionUpdate = serde_json::from_value(
            guest_rpc
                .ahead_request_blocking(AheadRequest::JoinSharedSession {
                    offer: offer.clone(),
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert_eq!(joined.actor_id, "bob");
        let guest_buffer: SharedBufferSnapshot = serde_json::from_value(
            guest_rpc
                .ahead_request_blocking(AheadRequest::ReadSharedBuffer {
                    session_id: view.session.id.clone(),
                    path: "src.rs".into(),
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert_eq!(guest_buffer, carol_edit.snapshot);
        let guest_edit: SharedBufferEditResult = serde_json::from_value(
            guest_rpc
                .ahead_request_blocking(AheadRequest::ReplaceSharedBuffer {
                    session_id: view.session.id.clone(),
                    path: "src.rs".into(),
                    expected_revision: guest_buffer.revision,
                    content: "fn guest_proxy() {}\n".into(),
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert!(guest_edit.applied);
        assert_eq!(bob.read_buffer("src.rs".into())?, guest_edit.snapshot);
        let guest_poll: SharedSessionUpdate = serde_json::from_value(
            guest_rpc
                .ahead_request_blocking(AheadRequest::PollSharedSession {
                    session_id: view.session.id.clone(),
                    after_sequence: message_sequence,
                    active_path: Some("src.rs".into()),
                    active_line: Some(3),
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert_eq!(guest_poll.actor_id, "bob");
        assert!(guest_poll.terminal_output.is_empty());
        let draft_id = uuid::Uuid::new_v4().to_string();
        let draft_path = format!(".ahead/shared-drafts/{}/src.rs", view.session.id);
        let draft_written: bool = serde_json::from_value(
            guest_rpc
                .ahead_request_blocking(AheadRequest::WriteEditorRecovery {
                    snapshot: EditorRecoverySnapshot {
                        buffer_id: draft_id.clone(),
                        revision: 1,
                        path: draft_path.clone().into(),
                        contents: Some("fn unsent_guest_draft() {}\n".into()),
                        saved_sha256: Some(format!(
                            "{:x}",
                            sha2::Sha256::digest(
                                guest_edit.snapshot.content.as_bytes()
                            )
                        )),
                    },
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert!(draft_written);
        guest_rpc
            .ahead_request_blocking(AheadRequest::LeaveSharedSession {
                session_id: view.session.id.clone(),
            })
            .map_err(|error| anyhow::anyhow!(error.message))?;
        assert!(
            guest_rpc
                .ahead_request_blocking(AheadRequest::PollSharedSession {
                    session_id: view.session.id.clone(),
                    after_sequence: message_sequence,
                    active_path: None,
                    active_line: None,
                })
                .is_err()
        );
        guest_rpc.notification(ProxyNotification::Shutdown {});
        guest_thread.join().expect("guest proxy dispatcher");
        let restarted_host = Arc::new(RwLock::new(AheadSessionHost::new(
            crate::ahead::store::SessionStore::open(&guest_database)?,
        )));
        restarted_host
            .read()
            .set_workspace(guest_workspace.path().to_path_buf());
        restarted_host
            .read()
            .set_auth_for_test(GitHubAuthManager::with_custom_dir(
                guest_workspace.path().join("auth"),
            ));
        let restarted_rpc = ProxyRpcHandler::new();
        let mut restarted_dispatcher = crate::dispatch::Dispatcher::new(
            CoreRpcHandler::new(),
            restarted_rpc.clone(),
        );
        restarted_dispatcher.handle_notification(ProxyNotification::Initialize {
            workspace: Some(guest_workspace.path().to_path_buf()),
            window_id: 2,
            tab_id: 2,
        });
        restarted_dispatcher.ahead_host = Some(restarted_host);
        let dispatcher_rpc = restarted_rpc.clone();
        let restarted_thread = thread::spawn(move || {
            dispatcher_rpc.mainloop(&mut restarted_dispatcher);
        });
        let rejoined: SharedSessionUpdate = serde_json::from_value(
            restarted_rpc
                .ahead_request_blocking(AheadRequest::JoinSharedSession {
                    offer: offer.clone(),
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert_eq!(rejoined.actor_id, "bob");
        let drafts: Vec<EditorRecoverySummary> = serde_json::from_value(
            restarted_rpc
                .ahead_request_blocking(AheadRequest::ListEditorRecoveries)
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].buffer_id, draft_id);
        let recovered: Option<EditorRecoverySnapshot> = serde_json::from_value(
            restarted_rpc
                .ahead_request_blocking(AheadRequest::ReadEditorRecovery {
                    buffer_id: draft_id,
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        let recovered = recovered.context("guest draft survived restart")?;
        assert_eq!(recovered.path.to_str(), Some(draft_path.as_str()));
        assert_eq!(
            recovered.contents.as_deref(),
            Some("fn unsent_guest_draft() {}\n")
        );
        let host_content: SharedBufferSnapshot = serde_json::from_value(
            restarted_rpc
                .ahead_request_blocking(AheadRequest::ReadSharedBuffer {
                    session_id: view.session.id.clone(),
                    path: "src.rs".into(),
                })
                .map_err(|error| anyhow::anyhow!(error.message))?,
        )?;
        assert_eq!(host_content, guest_edit.snapshot);
        restarted_rpc.notification(ProxyNotification::Shutdown {});
        restarted_thread.join().expect("restarted guest proxy");
        host.read().add_session_participant_verified(
            &view.session.id,
            GitHubUser {
                login: "carol".into(),
                id: 43,
                name: Some("Carol".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
            SessionRole::Viewer,
        )?;
        assert!(
            reviewer
                .replace_buffer(
                    "src.rs".into(),
                    carol_edit.snapshot.revision,
                    "not permitted\n".into(),
                )
                .is_err()
        );
        assert!(
            host.read()
                .start_shared_agent_turn(
                    &view.session.id,
                    "@carol please review".into(),
                    "bob",
                    42,
                )
                .is_err()
        );
        assert!(SharedSessionClient::connect(&offer, "outsider-token").is_err());
        host.read()
            .revoke_session_participant(&view.session.id, "bob")?;
        server.revoke_actor("bob");
        assert!(
            !server
                .presence()
                .iter()
                .any(|presence| presence.actor_id == "bob")
        );
        assert!(bob.poll(message_sequence, None, None).is_err());
        drop(server);
        proxy_rpc.notification(ProxyNotification::Shutdown {});
        dispatcher_thread.join().expect("proxy dispatcher");
        api_thread.join().expect("GitHub test API")?;
        Ok(())
    }
}
