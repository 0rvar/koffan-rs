use koffan::{Client, CreateItem, CreateStatus, Error, ItemUpdate};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn item(id: i64, name: &str, completed: bool) -> Value {
    json!({
        "id": id, "section_id": 1, "name": name, "description": "", "completed": completed,
        "uncertain": false, "quantity": 0, "sort_order": 0,
        "created_at": "2026-10-10T12:00:00Z", "updated_at": 1791633600
    })
}

async fn setup() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::new(format!("{}/", server.uri()), "secret").unwrap();
    (server, client)
}

#[tokio::test]
async fn null_arrays_deserialize_as_empty() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists"))
        .and(header("authorization", "Bearer secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "lists": null })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists/1/sections"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sections": [{ "id": 3, "list_id": 1, "name": "Bröd", "sort_mode": "manual", "items": null }]
        })))
        .mount(&server)
        .await;

    assert!(client.lists().await.unwrap().is_empty());
    let sections = client.sections(1).await.unwrap();
    assert_eq!(sections.len(), 1);
    assert!(sections[0].items.is_empty());
}

#[tokio::test]
async fn create_item_parses_all_three_response_shapes() {
    let cases = [
        (201, item(7, "Mjölk", false), CreateStatus::Created),
        (
            200,
            json!({ "item": item(7, "Mjölk", false), "reactivated": true }),
            CreateStatus::Reactivated,
        ),
        (
            200,
            json!({ "item": item(7, "Mjölk", false), "already_active": true }),
            CreateStatus::AlreadyActive,
        ),
    ];
    for (status, body, expected) in cases {
        let (server, client) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/items"))
            .and(header("content-type", "application/json"))
            .and(body_json(
                json!({ "section_id": 1, "name": "mjölk", "quantity": 2 }),
            ))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let outcome = client
            .create_item(&CreateItem {
                section_id: 1,
                name: "mjölk".into(),
                description: None,
                quantity: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(outcome.status, expected);
        assert_eq!(outcome.item.id, 7);
        assert_eq!(outcome.item.name, "Mjölk");
    }
}

#[tokio::test]
async fn error_body_becomes_api_error() {
    let (server, client) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/lists"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "list_name_exists", "message": "A list with this name already exists"
        })))
        .mount(&server)
        .await;
    match client.create_list("Mat", None).await.unwrap_err() {
        Error::Api {
            status,
            code,
            message,
        } => {
            assert_eq!(status, 400);
            assert_eq!(code, "list_name_exists");
            assert_eq!(message, "A list with this name already exists");
        }
        e => panic!("unexpected {e:?}"),
    }
}

#[tokio::test]
async fn non_json_error_falls_back_to_status_text() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items/1"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    match client.item(1).await.unwrap_err() {
        Error::Api { status, code, .. } => {
            assert_eq!(status, 502);
            assert_eq!(code, "Bad Gateway");
        }
        e => panic!("unexpected {e:?}"),
    }
}

#[tokio::test]
async fn update_item_always_sends_name() {
    let (server, client) = setup().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/items/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(7, "Mjölk", false)))
        .expect(1)
        .mount(&server)
        .await;
    client
        .update_item(
            7,
            &ItemUpdate {
                name: "Mjölk".into(),
                description: None,
                quantity: Some(3),
            },
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body, json!({ "name": "Mjölk", "quantity": 3 }));
}

#[tokio::test]
async fn set_item_completed_skips_toggle_when_state_matches() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(7, "Mjölk", true)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items/7/toggle"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(7, "Mjölk", false)))
        .expect(0)
        .mount(&server)
        .await;
    assert!(client.set_item_completed(7, true).await.unwrap().completed);
}

#[tokio::test]
async fn set_item_completed_toggles_when_state_differs() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(7, "Mjölk", false)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items/7/toggle"))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(7, "Mjölk", true)))
        .expect(1)
        .mount(&server)
        .await;
    assert!(client.set_item_completed(7, true).await.unwrap().completed);
}

#[test]
fn rejects_non_http_base_url() {
    assert!(matches!(
        Client::new("ftp://example.com", "t").unwrap_err(),
        Error::InvalidUrl(_)
    ));
    assert!(matches!(
        Client::new("not a url", "t").unwrap_err(),
        Error::InvalidUrl(_)
    ));
}

#[tokio::test]
async fn update_item_refuses_blank_name_without_a_request() {
    let (server, client) = setup().await;
    let err = client
        .update_item(
            7,
            &ItemUpdate {
                name: "  ".into(),
                description: None,
                quantity: Some(3),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn debug_output_redacts_the_token() {
    let client = Client::new("https://koffan.example.com", "hunter2-secret").unwrap();
    let debug = format!("{client:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("koffan.example.com/api/v1"), "{debug}");
}

#[test]
fn rejects_base_url_with_query_or_fragment() {
    for url in ["https://example.com/?x=1", "https://example.com/#top"] {
        assert!(
            matches!(Client::new(url, "t").unwrap_err(), Error::InvalidUrl(_)),
            "{url}"
        );
    }
}

#[tokio::test]
async fn base_url_path_prefix_is_kept() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/koffan/api/v1/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "lists": [] })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(format!("{}/koffan/", server.uri()), "secret").unwrap();
    assert!(client.lists().await.unwrap().is_empty());
}

#[tokio::test]
async fn non_json_success_body_suggests_a_wrong_url() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>login</html>"))
        .mount(&server)
        .await;
    let err = client.lists().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "unexpected response from {}/api/v1/lists; is this a Koffan API URL?",
            server.uri()
        )
    );
}

#[tokio::test]
async fn connection_failure_names_the_host() {
    // Port 9 (discard) is closed on test machines, so the connection is refused.
    let client = Client::new("http://127.0.0.1:9", "t").unwrap();
    let err = client.lists().await.unwrap_err();
    assert_eq!(err.to_string(), "could not connect to 127.0.0.1");
}
