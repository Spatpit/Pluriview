use super::*;

const REQUEST_LIFETIME: Duration = Duration::from_secs(60);
const PROMPT_INTERVAL: Duration = Duration::from_secs(10);
/// Repeated denied or ignored prompts double the wait up to this, so an
/// unknown device cannot keep interrupting the desktop. Approval resets it.
const MAX_PROMPT_INTERVAL: Duration = Duration::from_secs(10 * 60);

pub(super) fn random_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| io::Error::other("Random source unavailable"))?;
    Ok(bytes.iter().map(|v| format!("{v:02x}")).collect())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PairRequest {
    request: String,
    name: String,
}

impl PairRequest {
    fn valid(&self) -> bool {
        self.request.len() == 32
            && self.request.bytes().all(|c| c.is_ascii_hexdigit())
            && !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || " -_().".contains(c))
    }
}

struct Pending {
    request: String,
    client: String,
    name: String,
    code: String,
    created: Instant,
    decision: Option<bool>,
}

struct Grant {
    client: String,
    token: String,
    replay: super::auth::ReplayGuard,
}

#[derive(Default)]
pub(super) struct Pairing {
    pending: Option<Pending>,
    grant: Option<Grant>,
    last_prompt: Option<Instant>,
    refusals: u32,
}

impl Pairing {
    fn expire(&mut self, now: Instant) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| now.duration_since(p.created) >= REQUEST_LIFETIME)
        {
            if self.pending.as_ref().is_some_and(|p| p.decision.is_none()) {
                self.refusals += 1;
            }
            self.pending = None;
        }
    }

    fn prompt_interval(&self) -> Duration {
        PROMPT_INTERVAL
            .saturating_mul(1 << self.refusals.min(8))
            .min(MAX_PROMPT_INTERVAL)
    }

    pub(super) fn prompt(&mut self, now: Instant) -> Option<(String, String, String)> {
        self.expire(now);
        self.pending
            .as_ref()
            .filter(|p| p.decision.is_none())
            .map(|p| (p.request.clone(), p.name.clone(), p.code.clone()))
    }

    pub(super) fn pending(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.decision.is_none() && p.created.elapsed() < REQUEST_LIFETIME)
    }

    pub(super) fn authorized(
        &mut self,
        request: &Request,
        credentials: &super::auth::Credentials,
    ) -> bool {
        self.grant.as_mut().is_some_and(|grant| {
            grant.client == request.client
                && credentials.signed_by(
                    &grant.token,
                    &request.method,
                    &request.path,
                    &request.client,
                )
                && grant.replay.accept(credentials)
        })
    }

    pub(super) fn decide(&mut self, request: &str, allow: bool, busy: bool, now: Instant) {
        self.expire(now);
        if let Some(p) = self
            .pending
            .as_mut()
            .filter(|p| p.request == request && p.decision.is_none())
        {
            p.decision = Some(false);
            if !allow {
                self.refusals += 1;
            }
            if allow && !busy {
                if let Ok(token) = random_id() {
                    self.grant = Some(Grant {
                        client: p.client.clone(),
                        token,
                        replay: Default::default(),
                    });
                    p.decision = Some(true);
                    self.refusals = 0;
                }
            }
        }
    }

    pub(super) fn poll(
        &mut self,
        input: PairRequest,
        client: &str,
        busy: bool,
        cancel: bool,
        now: Instant,
    ) -> (u16, Vec<u8>) {
        if !input.valid() {
            return (400, vec![]);
        }
        self.expire(now);
        if let Some(p) = &mut self.pending {
            if p.request == input.request && p.client == client {
                if cancel {
                    if p.decision == Some(true) {
                        self.grant = None;
                    }
                    p.decision = Some(false);
                    return (200, vec![]);
                }
                return match p.decision {
                    None => (
                        202,
                        serde_json::to_vec(&serde_json::json!({"code":p.code})).unwrap(),
                    ),
                    Some(false) => (403, vec![]),
                    Some(true) => match &self.grant {
                        Some(g) if g.client == client => (
                            200,
                            serde_json::to_vec(&serde_json::json!({"token":g.token})).unwrap(),
                        ),
                        _ => (403, vec![]),
                    },
                };
            }
            if p.decision.is_none() {
                return (409, vec![]);
            }
        }
        if cancel {
            return (404, vec![]);
        }
        if busy {
            return (409, vec![]);
        }
        if self
            .last_prompt
            .is_some_and(|last| now.duration_since(last) < self.prompt_interval())
        {
            return (429, vec![]);
        }
        let Ok(random) = random_id() else {
            return (503, vec![]);
        };
        let code = format!(
            "{:06}",
            u32::from_str_radix(&random[..8], 16).unwrap() % 1_000_000
        );
        let reply = serde_json::to_vec(&serde_json::json!({"code":code})).unwrap();
        self.pending = Some(Pending {
            request: input.request,
            client: client.into(),
            name: input.name,
            code,
            created: now,
            decision: None,
        });
        self.last_prompt = Some(now);
        (202, reply)
    }
}

pub(super) fn serve_pairing(
    stream: &mut TcpStream,
    shared: &Shared,
    request: &Request,
    ctx: &egui::Context,
) -> io::Result<()> {
    if request.method == "GET" && request.path == "/v3/info" {
        let body = serde_json::to_vec(&serde_json::json!({"protocol":3,"name":shared.name,"busy":shared.watching() || shared.pairing.lock().pending()})).unwrap();
        return respond(stream, 200, "Content-Type: application/json\r\n", &body);
    }
    if request.method != "POST" || !matches!(request.path.as_str(), "/v3/pair" | "/v3/pair/cancel")
    {
        return respond(stream, 404, "", &[]);
    }
    let input = match serde_json::from_slice::<PairRequest>(&request.body) {
        Ok(input) => input,
        Err(_) => return respond(stream, 400, "", &[]),
    };
    let busy = shared.watching();
    let result = shared.pairing.lock().poll(
        input,
        &request.client,
        busy,
        request.path.ends_with("/cancel"),
        Instant::now(),
    );
    // Wake the approval dialog even when the canvas would otherwise be idle.
    if result.0 == 202 || request.path.ends_with("/cancel") {
        ctx.request_repaint();
    }
    respond(
        stream,
        result.0,
        "Content-Type: application/json\r\n",
        &result.1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn http(
        host: &Host,
        route: &str,
        client: &str,
        token: &str,
        body: Option<&str>,
        origin: bool,
    ) -> (TcpStream, String) {
        let uri = url::Url::parse(&host.invite).unwrap();
        let port = uri
            .query_pairs()
            .find(|(k, _)| k == "port")
            .unwrap()
            .1
            .parse::<u16>()
            .unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let method = if body.is_some() { "POST" } else { "GET" };
        let body = body.unwrap_or("");
        write!(stream, "{method} {route} HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: {client}\r\nContent-Length: {}\r\n{}\r\n{body}", super::super::auth::test_authorization(token, method, route, client), body.len(), if origin { "Origin: http://untrusted.invalid\r\n" } else { "" }).unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        let mut response = String::from_utf8(header).unwrap();
        if !matches!(route, "/v2/video" | "/v2/control") || !response.starts_with("HTTP/1.1 200") {
            let length = response
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .trim()
                .parse::<usize>()
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            response.push_str(std::str::from_utf8(&body).unwrap());
        }
        (stream, response)
    }

    #[test]
    fn approval_http_never_exposes_access_before_accept_and_remains_available_while_streaming() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let client = random_id().unwrap();
        let other = random_id().unwrap();
        let id = random_id().unwrap();
        let body = serde_json::json!({"request":id,"name":"Test phone"}).to_string();
        let info = http(&host, "/v3/info", &client, "", None, false).1;
        assert!(info.starts_with("HTTP/1.1 200"));
        assert!(!info.contains("token"));
        // A rejected header may close a socket before reading its POST body.
        // Use a bodyless request to check the response without a Winsock reset.
        assert!(http(&host, "/v3/info", &client, "", None, true)
            .1
            .starts_with("HTTP/1.1 400"));
        assert!(!host.shared.pairing.lock().pending());
        let response = http(&host, "/v3/pair", &client, "", Some(&body), false).1;
        assert!(response.starts_with("HTTP/1.1 202"));
        assert!(!response.contains("token"));
        assert!(http(&host, "/v2/video", &client, "", None, false)
            .1
            .starts_with("HTTP/1.1 401"));
        assert!(http(&host, "/v3/pair", &other, "", Some(&body), false)
            .1
            .starts_with("HTTP/1.1 409"));
        host.shared
            .pairing
            .lock()
            .decide(&id, true, false, Instant::now());
        let response = http(&host, "/v3/pair", &client, "", Some(&body), false).1;
        assert!(response.starts_with("HTTP/1.1 200"));
        let value: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        let token = value["token"].as_str().unwrap();
        assert!(http(&host, "/v2/video", &other, token, None, false)
            .1
            .starts_with("HTTP/1.1 401"));
        let (_video, header) = http(&host, "/v2/video", &client, token, None, false);
        assert!(header.starts_with("HTTP/1.1 200"));
        let (_control, header) = http(&host, "/v2/control", &client, token, None, false);
        assert!(header.starts_with("HTTP/1.1 200"));
        let info = http(&host, "/v3/info", &other, "", None, false).1;
        assert!(info.starts_with("HTTP/1.1 200"));
        assert!(info.contains("\"busy\":true"));
        let body =
            serde_json::json!({"request":random_id().unwrap(),"name":"Other phone"}).to_string();
        assert!(http(&host, "/v3/pair", &other, "", Some(&body), false)
            .1
            .starts_with("HTTP/1.1 409"));
        assert!(!host.shared.pairing.lock().pending());
    }

    #[test]
    fn approval_window_blocks_capture_even_on_its_acceptance_frame() {
        let ctx = egui::Context::default();
        let host = Host::start(Ipv4Addr::LOCALHOST, ctx.clone()).unwrap();
        let id = random_id().unwrap();
        host.shared
            .pairing
            .lock()
            .poll(input(&id), "phone", false, false, Instant::now());
        let mut viewer = RemoteView {
            host: Some(host),
            ..Default::default()
        };
        assert!(viewer.is_blocking());
        let _ = ctx.run(Default::default(), |ctx| viewer.ui(ctx));
        assert!(viewer.approval_drawn);
        viewer.host.as_ref().unwrap().shared.pairing.lock().decide(
            &id,
            true,
            false,
            Instant::now(),
        );
        assert!(
            viewer.is_blocking(),
            "Painted approval UI must never enter the video stream"
        );
        let _ = ctx.run(Default::default(), |ctx| viewer.ui(ctx));
        assert!(!viewer.is_blocking());
    }
    fn input(id: &str) -> PairRequest {
        PairRequest {
            request: id.into(),
            name: "Test phone".into(),
        }
    }
    #[test]
    fn approval_is_required_bound_to_request_and_client_and_revocable() {
        let mut state = Pairing::default();
        let now = Instant::now();
        let id = random_id().unwrap();
        let other = random_id().unwrap();
        let (status, body) = state.poll(input(&id), "phone", false, false, now);
        assert_eq!(status, 202);
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(response.get("token").is_none());
        assert_eq!(response["code"].as_str().unwrap().len(), 6);
        state.decide(&other, true, false, now);
        assert_eq!(state.poll(input(&id), "phone", false, false, now).0, 202);
        assert_eq!(state.poll(input(&id), "other", false, false, now).0, 409);
        state.decide(&id, true, false, now);
        let (status, body) = state.poll(input(&id), "phone", false, false, now);
        assert_eq!(status, 200);
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let token = response["token"].as_str().unwrap().to_owned();
        let authorized = |state: &mut Pairing, client: &str| {
            let request = Request {
                method: "GET".into(),
                path: "/v2/video".into(),
                authorization: super::super::auth::test_authorization(
                    &token,
                    "GET",
                    "/v2/video",
                    client,
                ),
                client: client.into(),
                last_frame: 0,
                body: Vec::new(),
            };
            let credentials =
                super::super::auth::Credentials::parse(&request.authorization).unwrap();
            state.authorized(&request, &credentials)
        };
        assert!(authorized(&mut state, "phone"));
        assert!(!authorized(&mut state, "other"));
        assert_ne!(state.poll(input(&id), "other", false, false, now).0, 200);
        assert_eq!(state.poll(input(&id), "phone", false, true, now).0, 200);
        assert!(!authorized(&mut state, "phone"));
        assert_eq!(state.poll(input(&id), "phone", false, false, now).0, 403);
    }
    #[test]
    fn denied_and_ignored_prompts_back_off_until_an_approval() {
        let mut state = Pairing::default();
        let mut now = Instant::now();
        let prompt = |state: &mut Pairing, now: Instant| {
            let id = random_id().unwrap();
            (state.poll(input(&id), "phone", false, false, now).0, id)
        };
        let (status, id) = prompt(&mut state, now);
        assert_eq!(status, 202);
        state.decide(&id, false, false, now);
        // One denial doubles the ten-second cooldown.
        now += PROMPT_INTERVAL;
        assert_eq!(prompt(&mut state, now).0, 429);
        now += PROMPT_INTERVAL;
        let (status, _) = prompt(&mut state, now);
        assert_eq!(status, 202);
        // Ignoring the prompt until it expires counts too.
        now += REQUEST_LIFETIME;
        assert!(state.prompt(now).is_none());
        assert_eq!(state.prompt_interval(), PROMPT_INTERVAL * 4);
        let (status, id) = prompt(&mut state, now);
        assert_eq!(status, 202);
        // Approval restores the normal interval.
        state.decide(&id, true, false, now);
        state.pending = None;
        assert_eq!(prompt(&mut state, now + PROMPT_INTERVAL).0, 202);
        assert!(state.prompt_interval() <= MAX_PROMPT_INTERVAL);
    }

    #[test]
    fn pending_requests_are_bounded_expire_and_cannot_interrupt_a_viewer() {
        let mut state = Pairing::default();
        let now = Instant::now();
        let id = random_id().unwrap();
        let other = random_id().unwrap();
        assert_eq!(state.poll(input(&id), "phone", true, false, now).0, 409);
        assert_eq!(state.poll(input(&id), "phone", false, false, now).0, 202);
        assert_eq!(state.poll(input(&other), "other", false, false, now).0, 409);
        state.decide(&id, true, true, now);
        assert_eq!(state.poll(input(&id), "phone", false, false, now).0, 403);
        assert_eq!(state.poll(input(&other), "other", false, false, now).0, 429);
        assert_eq!(
            state
                .poll(input(&other), "other", false, false, now + PROMPT_INTERVAL)
                .0,
            202
        );
        state.decide(
            &other,
            true,
            false,
            now + PROMPT_INTERVAL + REQUEST_LIFETIME,
        );
        assert!(state.grant.is_none());
        assert!(state
            .prompt(now + PROMPT_INTERVAL + REQUEST_LIFETIME)
            .is_none());
        assert_eq!(state.poll(input("bad"), "phone", false, false, now).0, 400);
    }
}
