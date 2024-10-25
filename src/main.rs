use actix_web::{rt, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_ws::{handle, AggregatedMessage, ProtocolError, Session};
use futures_util:: StreamExt ;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use std::collections::HashMap;

type Clients = Arc<Mutex<HashMap<Uuid, Arc<Mutex<Session>>>>>;

async fn chat_handler(
    req: HttpRequest,
    stream: web::Payload,
    clients: web::Data<Clients>,
) -> Result<HttpResponse, Error> {
    let (res, session, stream) = handle(&req, stream)?;

    let user_id = Uuid::new_v4();
    let session = Arc::new(Mutex::new(session));
    clients.lock().unwrap().insert(user_id, session.clone());

    // Send the UUID to the client as a welcome message
    session.lock().unwrap().text(format!("Your UUID: {}", user_id)).await.unwrap();

    let stream = Box::pin(stream.aggregate_continuations().max_continuation_size(2_usize.pow(20)));

    // Spawn a task to handle incoming messages
    rt::spawn(handle_messages(user_id, stream, clients.clone()));

    Ok(res)
}

async fn handle_messages(
    user_id: Uuid,
    mut stream: impl StreamExt<Item = Result<AggregatedMessage, ProtocolError>> + Unpin,
    clients: web::Data<Clients>,
) {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(AggregatedMessage::Text(text)) => {
                if let Some((recipient_id, message)) = parse_message(&text) {
                    let clients = clients.lock().unwrap();
                    if let Some(recipient_session) = clients.get(&recipient_id) {
                        if let Err(e) = recipient_session.lock().unwrap().text(message).await {
                            eprintln!("Failed to send message: {}", e);
                        }
                    } else {
                        eprintln!("Recipient not found: {}", recipient_id);
                    }
                } else {
                    eprintln!("Failed to parse message: {}", text);
                }
            }
            Ok(AggregatedMessage::Close(_)) => {
                clients.lock().unwrap().remove(&user_id);
                break;
            }
            _ => {}
        }
    }
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let clients = web::Data::new(Clients::default());

    HttpServer::new(move || {
        App::new()
            .app_data(clients.clone())
            .route("/ws", web::get().to(chat_handler))
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
