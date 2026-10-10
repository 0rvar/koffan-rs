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

/// A server with the single list "Mat" (id 1) whose sections are `sections`.
async fn server_with(sections: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "lists": [list(1, "Mat", 7, 2)] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/lists/1/sections"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sections": sections })))
        .mount(&server)
        .await;
    server
}

/// "Mat" with sections Frukt & grönt, Mejeri, an empty Bröd and Skafferi.
async fn server_with_mat() -> MockServer {
    let mut mjol = item(4, 20, "Mjöl", false, 2, "");
    mjol["uncertain"] = json!(true);
    server_with(json!([
        section(
            10,
            "Frukt & grönt",
            json!([
                item(1, 10, "Äpplen", false, 6, "röda"),
                item(2, 10, "Banan", true, 0, ""),
            ])
        ),
        section(
            20,
            "Mejeri",
            json!([
                item(3, 20, "Mjölk", false, 0, ""),
                mjol,
                item(5, 20, "Smör", true, 1, "normalsaltat"),
            ])
        ),
        section(30, "Bröd", Value::Null),
        section(
            40,
            "Skafferi",
            json!([
                item(6, 40, "Smörgåsgurka", false, 0, ""),
                item(7, 40, "Ägg", false, 6, ""),
            ])
        ),
    ]))
    .await
}

fn tools(server: &MockServer, default_list: Option<&str>) -> Tools {
    Tools::new(
        Client::new(server.uri(), "secret").unwrap(),
        default_list.map(String::from),
    )
}

async fn mock(server: &MockServer, verb: &str, url: &str, status: u16, body: Value, times: u64) {
    Mock::given(method(verb))
        .and(path(url))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn show_renders_open_items_by_section_then_checked_off_line() {
    let server = server_with_mat().await;
    let reply = tools(&server, None)
        .call("shopping_list_show", &json!({}))
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Mat: 5 open of 7\n\
         \n\
         Frukt & grönt\n\
         - Äpplen ×6 (röda)\n\
         \n\
         Mejeri\n\
         - Mjölk\n\
         - Mjöl ×2 (uncertain)\n\
         \n\
         Skafferi\n\
         - Smörgåsgurka\n\
         - Ägg ×6\n\
         \n\
         Checked off: Banan, Smör"
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
        .respond_with(ResponseTemplate::new(201).set_body_json(item(8, 10, "Päron", false, 3, "")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sections"))
        .and(body_json(json!({ "list_id": 1, "name": "Kryddor" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(section(50, "Kryddor", Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    for (id, name) in [(9, "Kanel"), (10, "Kardemumma")] {
        Mock::given(method("POST"))
            .and(path("/api/v1/items"))
            .and(body_json(json!({ "section_id": 50, "name": name })))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(item(id, 50, name, false, 0, "")),
            )
            .expect(1)
            .mount(&server)
            .await;
    }

    let reply = tools(&server, Some("mat"))
        .call(
            "shopping_list_add",
            &json!({ "items": [
                { "name": "Päron", "quantity": 3, "section": "frukt" },
                { "name": "Kanel", "section": "Kryddor" },
                { "name": "Kardemumma", "section": "kryddor" },
            ]}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Mat:\n\
         - Päron: added to Frukt & grönt\n\
         - Kanel: added to Kryddor (new section)\n\
         - Kardemumma: added to Kryddor"
    );
}

#[tokio::test]
async fn add_reuses_same_named_item_anywhere_on_the_list() {
    let server = server_with_mat().await;
    // Nothing new may be created: every name below is already on the list.
    mock(&server, "POST", "/api/v1/items", 201, json!({}), 0).await;
    // "ägg" matches "Ägg" in Skafferi although Koffan's ASCII-only case folding would not.
    Mock::given(method("POST"))
        .and(path("/api/v1/items/7/quantity"))
        .and(body_json(json!({ "quantity": 12 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(item(7, 40, "Ägg", false, 12, "")))
        .expect(1)
        .mount(&server)
        .await;
    // Smör is checked off: reopened, then given the new description.
    mock(
        &server,
        "GET",
        "/api/v1/items/5",
        200,
        item(5, 20, "Smör", true, 1, "normalsaltat"),
        1,
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/items/5/toggle",
        200,
        item(5, 20, "Smör", false, 1, "normalsaltat"),
        1,
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/items/5"))
        .and(body_json(
            json!({ "name": "Smör", "description": "osaltat" }),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(item(5, 20, "Smör", false, 1, "osaltat")),
        )
        .expect(1)
        .mount(&server)
        .await;
    // Banan, given as a bare string, is reopened without an update.
    mock(
        &server,
        "GET",
        "/api/v1/items/2",
        200,
        item(2, 10, "Banan", true, 0, ""),
        1,
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/items/2/toggle",
        200,
        item(2, 10, "Banan", false, 0, ""),
        1,
    )
    .await;
    mock(&server, "PUT", "/api/v1/items/2", 200, json!({}), 0).await;

    let reply = tools(&server, None)
        .call(
            "shopping_list_add",
            &json!({ "items": [
                { "name": "ägg", "quantity": 12, "section": "mejeri" },
                { "name": "SMÖR", "description": "osaltat" },
                "banan",
                { "name": "Mjölk" },
            ]}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Mat:\n\
         - Ägg: already on the list (in Skafferi), quantity set to 12\n\
         - Smör: re-added (was checked off)\n\
         - Banan: re-added (was checked off)\n\
         - Mjölk: already on the list (in Mejeri)"
    );
}

#[tokio::test]
async fn add_without_section_uses_first_section() {
    let server = server_with_mat().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/items"))
        .and(body_json(json!({ "section_id": 10, "name": "Kiwi" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(item(8, 10, "Kiwi", false, 0, "")))
        .expect(1)
        .mount(&server)
        .await;
    let reply = tools(&server, None)
        .call("shopping_list_add", &json!({ "items": ["Kiwi"] }))
        .await;
    assert_eq!(reply.content, "Mat:\n- Kiwi: added to Frukt & grönt");
}

#[tokio::test]
async fn add_creates_fallback_section_when_the_list_has_none() {
    let server = server_with(Value::Null).await;
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
}

#[tokio::test]
async fn add_treats_ambiguous_section_as_error_and_creates_nothing() {
    let server = server_with_mat().await;
    mock(&server, "POST", "/api/v1/sections", 201, json!({}), 0).await;
    mock(&server, "POST", "/api/v1/items", 201, json!({}), 0).await;
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

#[tokio::test]
async fn check_prefers_an_exact_match_over_a_prefix_match_in_another_state() {
    let server = server_with_mat().await;
    // "smör" means the checked-off Smör, not the open Smörgåsgurka it prefixes.
    mock(&server, "POST", "/api/v1/items/6/toggle", 200, json!({}), 0).await;
    mock(&server, "POST", "/api/v1/items/5/toggle", 200, json!({}), 0).await;
    let reply = tools(&server, None)
        .call("shopping_list_check", &json!({ "items": ["smör"] }))
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(reply.content, "Mat:\n- Smör: was already checked off");
}

#[tokio::test]
async fn check_reports_partial_failure_with_every_line() {
    let server = server_with_mat().await;
    mock(
        &server,
        "GET",
        "/api/v1/items/1",
        200,
        item(1, 10, "Äpplen", false, 6, "röda"),
        1,
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/items/1/toggle",
        200,
        item(1, 10, "Äpplen", true, 6, "röda"),
        1,
    )
    .await;

    let reply = tools(&server, None)
        .call(
            "shopping_list_check",
            &json!({ "items": ["mj", "äpp", "banan", "ost"] }),
        )
        .await;
    assert!(reply.is_error, "a failed item must flag the reply");
    assert_eq!(
        reply.content,
        "Mat:\n\
         - mj: \"mj\" matches several items: Mjölk (Mejeri), Mjöl (Mejeri); use a more specific name\n\
         - Äpplen: checked off\n\
         - Banan: was already checked off\n\
         - ost: no item matches \"ost\""
    );
}

#[tokio::test]
async fn check_reports_the_state_the_server_returned() {
    let server = server_with_mat().await;
    // Someone else checks Äpplen off between our read and our toggle, so the
    // toggle reopens it.
    mock(
        &server,
        "GET",
        "/api/v1/items/1",
        200,
        item(1, 10, "Äpplen", false, 6, "röda"),
        1,
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/items/1/toggle",
        200,
        item(1, 10, "Äpplen", false, 6, "röda"),
        1,
    )
    .await;
    let reply = tools(&server, None)
        .call("shopping_list_check", &json!({ "items": ["äpplen"] }))
        .await;
    assert!(reply.is_error);
    assert_eq!(
        reply.content,
        "Mat:\n- Äpplen: now open (changed by someone else meanwhile)"
    );
}

#[tokio::test]
async fn exact_duplicates_are_ambiguous_until_a_section_is_given() {
    let server = server_with(json!([
        section(20, "Mejeri", json!([item(3, 20, "Mjölk", false, 0, "")])),
        section(50, "Kyl", json!([item(8, 50, "mjölk", false, 0, "")])),
    ]))
    .await;
    mock(
        &server,
        "GET",
        "/api/v1/items/8",
        200,
        item(8, 50, "mjölk", false, 0, ""),
        1,
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/items/8/toggle",
        200,
        item(8, 50, "mjölk", true, 0, ""),
        1,
    )
    .await;
    mock(&server, "POST", "/api/v1/items/3/toggle", 200, json!({}), 0).await;
    let tools = tools(&server, None);

    let reply = tools
        .call("shopping_list_check", &json!({ "items": ["Mjölk"] }))
        .await;
    assert!(reply.is_error);
    assert_eq!(
        reply.content,
        "Mat:\n- Mjölk: \"Mjölk\" matches several items: Mjölk (Mejeri), mjölk (Kyl); use a more specific name"
    );

    let reply = tools
        .call(
            "shopping_list_check",
            &json!({ "items": [{ "name": "Mjölk", "section": "kyl" }] }),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(reply.content, "Mat:\n- mjölk: checked off");
}

#[tokio::test]
async fn check_prefers_the_duplicate_not_yet_in_the_target_state() {
    let server = server_with(json!([
        section(20, "Mejeri", json!([item(3, 20, "Mjölk", true, 0, "")])),
        section(50, "Kyl", json!([item(8, 50, "Mjölk", false, 0, "")])),
    ]))
    .await;
    mock(
        &server,
        "GET",
        "/api/v1/items/8",
        200,
        item(8, 50, "Mjölk", false, 0, ""),
        1,
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/items/8/toggle",
        200,
        item(8, 50, "Mjölk", true, 0, ""),
        1,
    )
    .await;
    let reply = tools(&server, None)
        .call("shopping_list_check", &json!({ "items": ["mjölk"] }))
        .await;
    assert_eq!(reply.content, "Mat:\n- Mjölk: checked off");
}

#[tokio::test]
async fn remove_uses_match_tiers() {
    let server = server_with_mat().await;
    mock(&server, "DELETE", "/api/v1/items/5", 204, Value::Null, 1).await;
    mock(&server, "DELETE", "/api/v1/items/6", 204, Value::Null, 0).await;
    let reply = tools(&server, None)
        .call(
            "shopping_list_remove",
            &json!({ "items": ["smör", { "name": "smör", "section": "mejeri" }] }),
        )
        .await;
    assert!(reply.is_error, "nothing named smör is left in Mejeri");
    assert_eq!(
        reply.content,
        "Mat:\n- Smör: removed\n- smör: no item matches \"smör\""
    );
}

#[tokio::test]
async fn clear_checked_deletes_only_items_still_checked_off() {
    let server = server_with_mat().await;
    mock(
        &server,
        "GET",
        "/api/v1/items/2",
        200,
        item(2, 10, "Banan", true, 0, ""),
        1,
    )
    .await;
    // Smör was put back on the list after the list was read.
    mock(
        &server,
        "GET",
        "/api/v1/items/5",
        200,
        item(5, 20, "Smör", false, 1, ""),
        1,
    )
    .await;
    for (id, expected) in [(1, 0), (2, 1), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0)] {
        mock(
            &server,
            "DELETE",
            &format!("/api/v1/items/{id}"),
            204,
            Value::Null,
            expected,
        )
        .await;
    }
    let reply = tools(&server, None)
        .call("shopping_list_clear_checked", &json!({ "list": "MAT" }))
        .await;
    assert!(!reply.is_error, "{}", reply.content);
    assert_eq!(
        reply.content,
        "Removed 1 checked-off item from Mat: Banan.\nKept:\n- Smör: put back on the list meanwhile"
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
    assert_eq!(reply.content, "- Mat (default): 5 open of 7");
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
