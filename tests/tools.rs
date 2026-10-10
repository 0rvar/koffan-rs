use koffan::tools::Tools;
use koffan::Client;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn list(id: i64, name: &str, total: i64, completed: i64) -> Value {
    json!({
        "id": id, "name": name, "icon": "🛒", "sort_order": id, "is_active": false,
        "show_completed": true, "created_at": "2026-10-10T12:00:00Z", "updated_at": 0,
        "stats": { "total_items": total, "completed_items": completed, "percentage": 0 }
    })
}

fn item(
    id: i64,
    section_id: i64,
    name: &str,
    completed: bool,
    quantity: i64,
    description: &str,
) -> Value {
    json!({
        "id": id, "section_id": section_id, "name": name, "description": description,
        "completed": completed, "uncertain": false, "quantity": quantity, "sort_order": id,
        "created_at": "2026-10-10T12:00:00Z", "updated_at": 0
    })
}

fn section(id: i64, name: &str, items: Value) -> Value {
    json!({ "id": id, "list_id": 1, "name": name, "sort_order": id, "sort_mode": "manual", "items": items })
}

/// One list "Mat" with sections "Frukt & grönt", "Mejeri" and an empty "Bröd".
async fn server_with_mat() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "lists": [list(1, "Mat", 5, 2)] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists/1/sections"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sections": [
                section(10, "Frukt & grönt", json!([
                    item(1, 10, "Äpplen", false, 6, "röda"),
                    item(2, 10, "Banan", true, 0, ""),
                ])),
                section(20, "Mejeri", json!([
                    item(3, 20, "Mjölk", false, 0, ""),
                    item(4, 20, "Mjöl", false, 2, ""),
                    item(5, 20, "Smör", true, 1, "normalsaltat"),
                ])),
                section(30, "Bröd", Value::Null),
            ]})),
        )
        .mount(&server)
        .await;
    server
}

fn tools(server: &MockServer, default_list: Option<&str>) -> Tools {
    Tools::new(
        Client::new(server.uri(), "secret").unwrap(),
        default_list.map(String::from),
    )
}

#[tokio::test]
async fn show_renders_the_list_as_text() {
    let server = server_with_mat().await;
    let reply = tools(&server, None)
        .call("shopping_list_show", &json!({}))
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Mat: 3 open of 5\n\
         \n\
         Frukt & grönt\n\
         - [ ] Äpplen ×6 (röda)\n\
         - [x] Banan\n\
         \n\
         Mejeri\n\
         - [ ] Mjölk\n\
         - [ ] Mjöl ×2\n\
         - [x] Smör\n\
         \n\
         Empty sections: Bröd"
    );
}

#[tokio::test]
async fn add_resolves_section_by_prefix_and_creates_missing_ones() {
    let server = server_with_mat().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(
            json!({ "section_id": 10, "name": "Päron", "quantity": 3 }),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(item(6, 10, "Päron", false, 3, "")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sections"))
        .and(body_json(json!({ "list_id": 1, "name": "Kryddor" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(section(40, "Kryddor", Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(json!({ "section_id": 40, "name": "Kanel" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(item(7, 40, "Kanel", false, 0, "")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(json!({ "section_id": 40, "name": "Kardemumma" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(item(
            8,
            40,
            "Kardemumma",
            false,
            0,
            "",
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(json!({ "section_id": 20, "name": "mjölk" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "item": item(3, 20, "Mjölk", false, 0, ""), "already_active": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let reply = tools(&server, Some("mat"))
        .call(
            "shopping_list_add",
            &json!({ "items": [
                { "name": "Päron", "quantity": 3, "section": "frukt" },
                { "name": "Kanel", "section": "Kryddor" },
                { "name": "Kardemumma", "section": "kryddor" },
                { "name": "mjölk", "section": "mejeri" },
            ]}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Mat:\n\
         - Päron: added to Frukt & grönt\n\
         - Kanel: added to Kryddor (new section)\n\
         - Kardemumma: added to Kryddor\n\
         - Mjölk: already on the list"
    );
}

#[tokio::test]
async fn add_without_section_uses_first_section() {
    let server = server_with_mat().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(json!({ "section_id": 10, "name": "Banan" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "item": item(2, 10, "Banan", false, 0, ""), "reactivated": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let reply = tools(&server, None)
        .call(
            "shopping_list_add",
            &json!({ "items": [{ "name": "Banan" }] }),
        )
        .await;
    assert_eq!(reply.content, "Mat:\n- Banan: re-added (was checked off)");
}

#[tokio::test]
async fn check_reports_ambiguity_and_continues_with_other_items() {
    let server = server_with_mat().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items/1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(item(1, 10, "Äpplen", false, 6, "röda")),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items/1/toggle"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(item(1, 10, "Äpplen", true, 6, "röda")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let reply = tools(&server, None)
        .call(
            "shopping_list_check",
            &json!({ "items": ["mj", "äpp", "banan", "ost"] }),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Mat:\n\
         - mj: \"mj\" matches several items: Mjölk, Mjöl; use a more specific name\n\
         - Äpplen: checked off\n\
         - Banan: was already checked off\n\
         - ost: no item matches \"ost\""
    );
}

#[tokio::test]
async fn clear_checked_deletes_only_completed_items() {
    let server = server_with_mat().await;
    for (id, expected) in [(1, 0), (2, 1), (3, 0), (4, 0), (5, 1)] {
        Mock::given(method("DELETE"))
            .and(path(format!("/api/v1/items/{id}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(expected)
            .mount(&server)
            .await;
    }
    let reply = tools(&server, None)
        .call("shopping_list_clear_checked", &json!({ "list": "MAT" }))
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Removed 2 checked-off items from Mat: Banan, Smör."
    );
}

#[tokio::test]
async fn missing_default_list_names_the_lists() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "lists": [list(1, "Mat", 0, 0), list(2, "Apotek", 0, 0)]
        })))
        .mount(&server)
        .await;
    let reply = tools(&server, None)
        .call("shopping_list_show", &Value::Null)
        .await;
    assert!(reply.is_error);
    assert_eq!(
        reply.content,
        "no list was named and there is no default list; name one of: Mat, Apotek"
    );

    let reply = tools(&server, Some("Hemma"))
        .call("shopping_list_show", &json!({}))
        .await;
    assert!(reply.is_error);
    assert_eq!(
        reply.content,
        "no list matches \"Hemma\"; existing lists: Mat, Apotek"
    );
}

#[tokio::test]
async fn lists_marks_the_default() {
    let server = server_with_mat().await;
    let reply = tools(&server, Some("Mat"))
        .call("shopping_lists", &json!({}))
        .await;
    assert_eq!(reply.content, "- Mat (default): 3 open of 5");
}

#[tokio::test]
async fn bad_input_and_unknown_tools_are_error_replies() {
    let server = server_with_mat().await;
    let tools = tools(&server, None);
    let reply = tools
        .call("shopping_list_add", &json!({ "items": "milk" }))
        .await;
    assert!(reply.is_error);
    assert!(
        reply.content.starts_with("invalid input:"),
        "{}",
        reply.content
    );

    let reply = tools.call("shopping_list_explode", &json!({})).await;
    assert!(reply.is_error);
    assert!(reply.content.contains("shopping_list_show"));
}

#[test]
fn specs_cover_every_handled_tool() {
    let specs = Tools::specs();
    assert_eq!(specs.len(), 6);
    for spec in &specs {
        assert!(Tools::handles(spec.name));
        assert_eq!(spec.input_schema["type"], "object");
        assert_eq!(spec.input_schema["additionalProperties"], false);
        assert!(
            spec.description.split_whitespace().count() <= 60,
            "{}",
            spec.name
        );
    }
    assert!(!Tools::handles("send_email"));
}

#[tokio::test]
async fn add_treats_ambiguous_section_as_error_and_creates_fallback_section_for_empty_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "lists": [list(1, "Mat", 0, 0)] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists/1/sections"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sections": null })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sections"))
        .and(body_json(json!({ "list_id": 1, "name": "Övrigt" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(section(50, "Övrigt", Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(json!({ "section_id": 50, "name": "Tvål" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(item(9, 50, "Tvål", false, 0, "")))
        .expect(1)
        .mount(&server)
        .await;
    let reply = tools(&server, None)
        .call(
            "shopping_list_add",
            &json!({ "items": [{ "name": "Tvål" }] }),
        )
        .await;
    assert_eq!(reply.content, "Mat:\n- Tvål: added to Övrigt (new section)");

    let server = server_with_mat().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sections"))
        .respond_with(ResponseTemplate::new(201).set_body_json(section(60, "x", Value::Null)))
        .expect(0)
        .mount(&server)
        .await;
    // "ö" is inside both "Frukt & grönt" and "Bröd" without prefixing either.
    let reply = tools(&server, None)
        .call(
            "shopping_list_add",
            &json!({ "items": [{ "name": "Kaffe", "section": "ö" }] }),
        )
        .await;
    assert!(reply.is_error);
    assert_eq!(
        reply.content,
        "Mat:\n- Kaffe: not added: \"ö\" matches several sections: Frukt & grönt, Bröd; use a more specific name"
    );
}
