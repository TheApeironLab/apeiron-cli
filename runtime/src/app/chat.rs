use super::*;
use reqwest::{Method, Url};

const HELP: &str = include_str!("../../assets/chat-help.txt");
fn encode(s: &str) -> String {
    s.as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn request(
    c: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let mut req = c
        .request(method, format!("{base}/_matrix/client/v3{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        req = req.json(body);
    }
    let response = req
        .send()
        .map_err(|_| fail("Matrix request failed or timed out; mutation outcome may be unknown", 9))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        if let Some(retry) = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.chars().all(|c| c.is_ascii_digit()))
        {
            eprintln!("retry_after_seconds: {retry}");
        }
        let code = match status {
            401 | 403 => 7,
            404 => 4,
            409 => 5,
            429 => 8,
            300..=399 | 500..=599 => 9,
            _ => 2,
        };
        return Err(fail(format!("Matrix HTTP {status}"), code));
    }
    serde_json::from_reader(response.take(8 * 1024 * 1024)).map_err(|_| {
        fail(
            "Invalid or oversized Matrix response; mutation outcome may be unknown",
            9,
        )
    })
}
fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value[key].as_array().ok_or_else(|| fail("Invalid Matrix response", 9))
}
pub fn run(args: &[String]) -> Result<()> {
    let (pos, o) = parse(
        args,
        &[
            "server",
            "token-file",
            "query",
            "limit",
            "user",
            "name",
            "room",
            "event",
            "text",
            "txn-id",
            "cursor",
            "timeout",
            "out",
        ],
        &["help", "jsonl", "dry-run"],
    )?;
    if pos.is_empty() || o.contains_key("help") {
        println!("{HELP}");
        return Ok(());
    }
    let action = pos.join(" ");
    let allowed: &[&str] = match action.as_str() {
        "status" | "whoami" | "room list" => &[],
        "user search" => &["query", "limit"],
        "room create" => &["user", "name", "dry-run"],
        "room join" | "room leave" => &["room", "dry-run"],
        "room invite" => &["room", "user", "dry-run"],
        "room read" => &["room", "event", "dry-run"],
        "message send" => &["room", "text", "txn-id", "dry-run"],
        "message edit" => &["room", "text", "event", "txn-id", "dry-run"],
        "message redact" => &["room", "event", "txn-id", "dry-run"],
        "message list" => &["room", "cursor", "limit"],
        "sync" => &["cursor", "timeout"],
        _ => return Err(fail("Unknown chat command; use apeiron chat --help", 2)),
    };
    for key in o.keys() {
        if !["server", "token-file", "jsonl", "out"].contains(&key.as_str()) && !allowed.contains(&key.as_str()) {
            return Err(fail(format!("Unexpected --{key} for {action}"), 2));
        }
    }
    let get = |key: &str| required(&o, key);
    let bounded = |key: &str, default: u64, max: u64| -> Result<u64> {
        let value = o
            .get(key)
            .map(|v| v.parse::<u64>().map_err(|_| fail(format!("Invalid --{key}"), 2)))
            .transpose()?
            .unwrap_or(default);
        if value > max || (key != "timeout" && value == 0) {
            return Err(fail(format!("Invalid --{key}"), 2));
        }
        Ok(value)
    };
    let mut method = Method::GET;
    let mut body = None;
    let mut room = String::new();
    let mut encryption = false;
    let path = match action.as_str() {
        "status" | "whoami" => "/account/whoami".into(),
        "user search" => {
            method = Method::POST;
            body = Some(json!({"search_term":get("query")?,"limit":bounded("limit",10,100)?}));
            "/user_directory/search".into()
        }
        "room list" => "/joined_rooms".into(),
        "room create" => {
            method = Method::POST;
            let mut b = json!({"preset":"private_chat","is_direct":true,"invite":[get("user")?]});
            if let Some(name) = o.get("name") {
                b["name"] = json!(name);
            }
            body = Some(b);
            "/createRoom".into()
        }
        "room join" => {
            method = Method::POST;
            body = Some(json!({}));
            format!("/join/{}", encode(get("room")?))
        }
        "room leave" | "room invite" | "room read" => {
            method = Method::POST;
            body = Some(match action.as_str() {
                "room invite" => json!({"user_id":get("user")?}),
                _ => json!({}),
            });
            let suffix = match action.as_str() {
                "room read" => format!("receipt/m.read/{}", encode(get("event")?)),
                "room leave" => "leave".into(),
                _ => "invite".into(),
            };
            format!("/rooms/{}/{suffix}", encode(get("room")?))
        }
        "message send" | "message edit" => {
            method = Method::PUT;
            room = get("room")?.into();
            encryption = true;
            let text = get("text")?;
            let mut b = json!({"msgtype":"m.text","body":text});
            if action == "message edit" {
                b = json!({"msgtype":"m.text","body":format!("* {text}"),"m.new_content":b,"m.relates_to":{"rel_type":"m.replace","event_id":get("event")?}});
            }
            body = Some(b);
            format!(
                "/rooms/{}/send/m.room.message/{}",
                encode(&room),
                encode(get("txn-id")?)
            )
        }
        "message redact" => {
            method = Method::PUT;
            body = Some(json!({}));
            format!(
                "/rooms/{}/redact/{}/{}",
                encode(get("room")?),
                encode(get("event")?),
                encode(get("txn-id")?)
            )
        }
        "message list" => format!(
            "/rooms/{}/messages?dir=b&limit={}{}",
            encode(get("room")?),
            bounded("limit", 10, 100)?,
            o.get("cursor")
                .map(|c| format!("&from={}", encode(c)))
                .unwrap_or_default()
        ),
        "sync" => {
            get("out")?;
            format!(
                "/sync?timeout={}&filter={}{}",
                bounded("timeout", 0, 30)? * 1000,
                encode(r#"{"room":{"timeline":{"limit":10}}}"#),
                o.get("cursor")
                    .map(|c| format!("&since={}", encode(c)))
                    .unwrap_or_default()
            )
        }
        _ => unreachable!(),
    };
    if o.contains_key("dry-run") {
        return output(
            json!({"action":action,"method":method.as_str(),"path":path,"body":body}),
            &o,
            "apeiron.chat.v1",
        );
    }
    let configured = o
        .get("server")
        .cloned()
        .or_else(|| std::env::var("APEIRON_CHAT_SERVER").ok())
        .ok_or_else(|| fail("Set APEIRON_CHAT_SERVER or --server", 2))?;
    let url = Url::parse(&configured).map_err(|_| fail("Invalid Matrix origin", 2))?;
    let local = ["127.0.0.1", "localhost", "[::1]"].contains(&url.host_str().unwrap_or(""));
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !(url.scheme() == "https" || (url.scheme() == "http" && local))
    {
        return Err(fail("Matrix server must be an HTTPS origin (loopback HTTP allowed)", 2));
    }
    let token_path = o
        .get("token-file")
        .cloned()
        .or_else(|| std::env::var("APEIRON_CHAT_TOKEN_FILE").ok())
        .ok_or_else(|| fail("Set APEIRON_CHAT_TOKEN_FILE to a Matrix access-token file", 7))?;
    let token = token_file(Path::new(&token_path))?;
    let c = client(if action == "sync" { 45 } else { 15 })?;
    let base = url.as_str().trim_end_matches('/');
    if encryption {
        let state = request(
            &c,
            base,
            &token,
            Method::GET,
            &format!("/rooms/{}/state", encode(&room)),
            None,
        )?;
        let events = state.as_array().ok_or_else(|| fail("Invalid room state", 9))?;
        if events.iter().any(|e| e["type"] == "m.room.encryption") {
            return Err(fail("Encrypted room: use an E2EE-capable client", 2));
        }
    }
    let result = request(&c, base, &token, method, &path, body.as_ref())?;
    let required_field = match action.as_str() {
        "status" | "whoami" => Some("user_id"),
        "room create" => Some("room_id"),
        "message send" | "message edit" | "message redact" => Some("event_id"),
        "sync" => Some("next_batch"),
        _ => None,
    };
    if let Some(key) = required_field {
        if result[key].as_str().filter(|s| !s.is_empty()).is_none() {
            return Err(fail("Incomplete Matrix response; mutation outcome may be unknown", 9));
        }
    }
    let data = match action.as_str() {
        "status" | "whoami" => json!({"user_id":result["user_id"],"device_id":result["device_id"],"connected":true}),
        "user search" => {
            json!({"items":list(&result,"results")?.iter().map(|u|json!({"user_id":u["user_id"],"display_name":u["display_name"]})).collect::<Vec<_>>(),"limited":result["limited"]})
        }
        "room list" => {
            let rooms = list(&result, "joined_rooms")?;
            if rooms.len() > 100 && !o.contains_key("out") {
                return Err(fail("Large room list: repeat with --out NEW_FILE", 2));
            }
            json!({"items":rooms.iter().map(|r|json!({"room_id":r})).collect::<Vec<_>>()})
        }
        "message list" => {
            json!({"items":list(&result,"chunk")?.iter().take(bounded("limit",10,100)? as usize).map(|e|json!({"event_id":e["event_id"],"sender":e["sender"],"type":e["type"],"body":e["content"]["body"]})).collect::<Vec<_>>(),"next_cursor":result["end"]})
        }
        "sync" => {
            let mut r = result.clone();
            r["next_cursor"] = result["next_batch"].clone();
            r
        }
        _ => {
            let mut r = result;
            r["ok"] = json!(true);
            r
        }
    };
    output(data, &o, "apeiron.chat.v1")
}
