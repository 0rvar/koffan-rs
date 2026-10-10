# koffan

Rust client for the [Koffan](https://github.com/PanSalut/Koffan) shopping list REST API (`/api/v1`, written against v2.16.1), plus a set of tools an LLM agent can call to read and edit lists by name.

The server needs `API_TOKEN` set. The token gives access to one workspace.

```toml
[dependencies]
koffan = { git = "https://github.com/0rvar/koffan-rs" }
```

## Client

One async method per endpoint. Errors from the server come back as `Error::Api` with Koffan's error code.

```rust
use koffan::{Client, CreateItem};

let client = Client::new("https://koffan.example.com", token)?;
let list = client.lists().await?.into_iter().find(|l| l.name == "Mat").unwrap();
let sections = client.sections(list.id).await?;
let outcome = client
    .create_item(&CreateItem { section_id: sections[0].id, name: "Mjölk".into(), description: None, quantity: Some(2) })
    .await?;
println!("{:?}", outcome.status); // Created, Reactivated or AlreadyActive
client.set_item_completed(outcome.item.id, true).await?;
```

A few API quirks the client documents on the relevant methods. Adding an item merges it with a same-named item only within the target section, and the server ignores ASCII case only, so "Ägg" and "ägg" end up as two items. `toggle_item` flips rather than sets (use `set_item_completed`). `update_item` always sends the name because the server clears the description otherwise.

## Tools for LLM agents

`Tools` exposes six tools: `shopping_lists`, `shopping_list_show`, `shopping_list_add`, `shopping_list_check`, `shopping_list_remove` and `shopping_list_clear_checked`. Lists, sections and items are matched by name (exact, then prefix, then substring, ignoring case including å, ä and ö), so the model never sees ids. When the same name is in several sections, the model can pass `{ "name", "section" }`. Adding checks the whole list first, so an item already on it is reused or put back instead of duplicated. Replies are plain text meant to be read aloud. A call never returns `Err`: failures are replies with `is_error: true` that say what was ambiguous or which lists exist, and a call where only some items failed still lists every item.

```rust
use koffan::tools::Tools;
use serde_json::json;

let tools = Tools::new(client, Some("Mat".into())); // default list when `list` is omitted
for spec in Tools::specs() {
    register(spec.name, spec.description, spec.input_schema);
}
let reply = tools.call("shopping_list_add", &json!({ "items": [{ "name": "mjölk", "quantity": 2, "section": "mej" }] })).await;
assert!(!reply.is_error);
println!("{}", reply.content); // "Mat:\n- Mjölk: added to Mejeri", or "already on the list (in Kyl)"
```

### Wrapping in a tool trait

For a framework with one object per tool:

```rust
struct KoffanTool { spec: ToolSpec, tools: Arc<Tools> }

#[async_trait]
impl Tool for KoffanTool {
    fn name(&self) -> &str { self.spec.name }
    fn description(&self) -> &str { self.spec.description }
    fn input_schema(&self) -> Value { self.spec.input_schema.clone() }
    async fn call(&self, input: Value) -> ToolResult {
        let reply = self.tools.call(self.spec.name, &input).await;
        ToolResult { content: reply.content, is_error: reply.is_error }
    }
}

let tools = Arc::new(Tools::new(client, default_list));
let registered: Vec<Box<dyn Tool>> = Tools::specs().into_iter()
    .map(|spec| Box::new(KoffanTool { spec, tools: tools.clone() }) as Box<dyn Tool>)
    .collect();
```

### Dispatching from a match

For an agent loop that matches on the tool name:

```rust
let reply = match name.as_str() {
    n if Tools::handles(n) => {
        let r = koffan_tools.call(n, &input).await;
        ToolOutput { text: r.content, is_error: r.is_error }
    }
    "web_search" => web_search(&input).await,
    other => ToolOutput::error(format!("unknown tool {other}")),
};
```

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

Tests run against [wiremock](https://crates.io/crates/wiremock), so no Koffan server is needed. The minimum supported Rust version is 1.88. TLS is rustls with bundled webpki roots; there is no OpenSSL dependency.

## License

MIT
