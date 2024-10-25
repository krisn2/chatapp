use actix_web::{rt, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_ws::{handle, AggregatedMessage, ProtocolError, Session};
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use std::collections::HashMap;

type Clients = Arc<Mutex<HashMap<Uuid, Arc<Mutex<Session>>>>>; // Store active WebSocket sessions
type Groups = Arc<Mutex<HashMap<String, Vec<Uuid>>>>; // Store groups and their members

struct AppState {
    clients: Clients,
    groups: Groups,
}

async fn chat_handler(
    req: HttpRequest,
    stream: web::Payload,
    data: web::Data<AppState>,
) -> Result<HttpResponse, Error> {
    let (res, session, stream) = handle(&req, stream)?;

    let user_id = Uuid::new_v4();
    let session = Arc::new(Mutex::new(session));
    data.clients.lock().unwrap().insert(user_id, session.clone());

    // Send the UUID to the client as a welcome message
    session.lock().unwrap().text(format!("Your UUID: {}", user_id)).await.unwrap();

    let stream = Box::pin(stream.aggregate_continuations().max_continuation_size(2_usize.pow(20)));

    // Spawn a task to handle incoming messages
    let data_clone = data.clone();
    rt::spawn(handle_messages(user_id, stream, data_clone));

    Ok(res)
}

async fn handle_messages(
    user_id: Uuid,
    mut stream: impl StreamExt<Item = Result<AggregatedMessage, ProtocolError>> + Unpin,
    data: web::Data<AppState>,
) {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(AggregatedMessage::Text(text)) => {
                if text.starts_with("create_group:") {
                    let group_name = text.trim_start_matches("create_group:").trim().to_string();
                    create_group(group_name, user_id, data.groups.clone()).await;
                } else if let Some((recipient_id, message)) = parse_message(&text) {
                    let clients = data.clients.lock().unwrap();
                    if let Some(recipient_session) = clients.get(&recipient_id) {
                        if let Err(e) = recipient_session.lock().unwrap().text(message).await {
                            eprintln!("Failed to send message: {}", e);
                        }
                    } else {
                        eprintln!("Recipient not found: {}", recipient_id);
                    }
                } else if let Some((group_name, message)) = parse_group_message(&text) {
                    send_group_message(group_name, message, user_id, data.clone()).await;
                } else {
                    eprintln!("Failed to parse message: {}", text);
                }
            }
            Ok(AggregatedMessage::Close(_)) => {
                data.clients.lock().unwrap().remove(&user_id);
                break;
            }
            _ => {}
        }
    }
}

async fn create_group(group_name: String, user_id: Uuid, groups: Groups) {
    let mut groups = groups.lock().unwrap();
    groups.entry(group_name.clone()).or_insert_with(Vec::new).push(user_id);
    println!("User {} created/added to group {}", user_id, group_name);
}

fn parse_message(input: &str) -> Option<(Uuid, String)> {
    let parts: Vec<&str> = input.split(", message:").collect();
    if parts.len() == 2 {
        let recipient_id = parts[0].trim_start_matches("to:").trim();
        let message = parts[1].trim();
        if let Ok(uuid) = Uuid::parse_str(recipient_id) {
            return Some((uuid, message.to_string()));
        }
    }
    None
}

fn parse_group_message(input: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = input.split(", message:").collect();
    if parts.len() == 2 {
        let group_name = parts[0].trim_start_matches("to_group:").trim().to_string();
        let message = parts[1].trim().to_string();
        return Some((group_name, message));
    }
    None
}

async fn send_group_message(group_name: String, message: String, sender_id: Uuid, data: web::Data<AppState>) {
    let groups = data.groups.lock().unwrap();
    if let Some(group) = groups.get(&group_name) {
        let clients = data.clients.lock().unwrap();
        for &user_id in group.iter() {
            if user_id != sender_id {
                if let Some(session) = clients.get(&user_id) {
                    if let Err(e) = session.lock().unwrap().text(message.clone()).await {
                        eprintln!("Failed to send message to group {}: {}", group_name, e);
                    }
                }
            }
        }
    } else {
        eprintln!("Group not found: {}", group_name);
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let groups: Groups = Arc::new(Mutex::new(HashMap::new()));
    let data = web::Data::new(AppState { clients, groups });

    HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .route("/ws", web::get().to(chat_handler))
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
