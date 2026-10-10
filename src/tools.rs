//! Shopping-list tools for LLM agents, independent of any agent framework.
//!
//! [`Tools::specs`] describes the tools as JSON Schema; [`Tools::call`] runs
//! one by name. Lists, sections and items are addressed by name and matched
//! with [`crate::matching::resolve`], so the model never sees or needs ids.
//! Replies are short plain text suitable for reading aloud.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::matching::resolve;
use crate::model::{CreateItem, CreateStatus, Item, List, Section};
use crate::{Client, Error};

/// Name of the section created when items are added to a list that has none.
pub const FALLBACK_SECTION: &str = "Övrigt";

/// A tool definition in the shape most LLM APIs expect.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema for the tool's input object.
    pub input_schema: Value,
}

/// The result of a tool call. Failures are replies with `is_error` set, so
/// they can be passed back to the model as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReply {
    pub content: String,
    pub is_error: bool,
}

impl ToolReply {
    fn ok(content: impl Into<String>) -> Self {
        ToolReply {
            content: content.into(),
            is_error: false,
        }
    }

    fn error(content: impl Into<String>) -> Self {
        ToolReply {
            content: content.into(),
            is_error: true,
        }
    }
}

/// The shopping-list tools bound to one Koffan server.
#[derive(Debug, Clone)]
pub struct Tools {
    client: Client,
    default_list: Option<String>,
}

const LISTS: &str = "shopping_lists";
const SHOW: &str = "shopping_list_show";
const ADD: &str = "shopping_list_add";
const CHECK: &str = "shopping_list_check";
const REMOVE: &str = "shopping_list_remove";
const CLEAR_CHECKED: &str = "shopping_list_clear_checked";

const NAMES: [&str; 6] = [LISTS, SHOW, ADD, CHECK, REMOVE, CLEAR_CHECKED];

fn list_property() -> Value {
    json!({
        "type": "string",
        "description": "List name, matched case-insensitively and by prefix. Omit to use the household's default list."
    })
}

fn item_names_property(description: &str) -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "items": { "type": "string" },
        "description": description
    })
}

#[derive(Deserialize)]
struct ListInput {
    #[serde(default)]
    list: Option<String>,
}

#[derive(Deserialize)]
struct AddInput {
    #[serde(default)]
    list: Option<String>,
    items: Vec<AddItem>,
}

#[derive(Deserialize)]
struct AddItem {
    name: String,
    #[serde(default)]
    quantity: Option<i64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    section: Option<String>,
}

#[derive(Deserialize)]
struct CheckInput {
    #[serde(default)]
    list: Option<String>,
    items: Vec<String>,
    #[serde(default = "default_checked")]
    checked: bool,
}

fn default_checked() -> bool {
    true
}

#[derive(Deserialize)]
struct NamesInput {
    #[serde(default)]
    list: Option<String>,
    items: Vec<String>,
}

type Outcome = std::result::Result<ToolReply, String>;

fn parse<T: for<'de> Deserialize<'de>>(input: &Value) -> std::result::Result<T, String> {
    let input = if input.is_null() {
        json!({})
    } else {
        input.clone()
    };
    serde_json::from_value(input).map_err(|e| format!("invalid input: {e}"))
}

fn names_of(lists: &[List]) -> String {
    lists
        .iter()
        .map(|l| l.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Turns per-item report lines into a reply that is an error only when every item failed.
fn per_item_reply(header: String, lines: Vec<String>, failures: usize) -> ToolReply {
    let content = std::iter::once(header)
        .chain(lines.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n");
    ToolReply {
        content,
        is_error: failures > 0 && failures == lines.len(),
    }
}

/// Renders a list as text: a header with open/total counts, a block per
/// section with items, and one line naming the empty sections.
pub fn render_list(list: &List, sections: &[Section]) -> String {
    let all = sections.iter().flat_map(|s| s.items.iter());
    let total = all.clone().count();
    let open = all.filter(|i| !i.completed).count();
    let mut out = format!("{}: {open} open of {total}", list.name);

    let mut empty = Vec::new();
    for section in sections {
        if section.items.is_empty() {
            empty.push(section.name.as_str());
            continue;
        }
        out.push_str("\n\n");
        out.push_str(&section.name);
        for item in &section.items {
            out.push('\n');
            out.push_str(&render_item(item));
        }
    }
    if sections.is_empty() {
        out.push_str("\n\nThe list has no sections.");
    } else if !empty.is_empty() {
        out.push_str("\n\nEmpty sections: ");
        out.push_str(&empty.join(", "));
    }
    out
}

fn render_item(item: &Item) -> String {
    if item.completed {
        return format!("- [x] {}", item.name);
    }
    let mut line = format!("- [ ] {}", item.name);
    if item.quantity > 0 {
        line.push_str(&format!(" ×{}", item.quantity));
    }
    if !item.description.is_empty() {
        line.push_str(&format!(" ({})", item.description));
    }
    line
}

impl Tools {
    /// `default_list` names the list used when a call omits `list`; it is
    /// matched like any other list name. Without it, a call that omits `list`
    /// works only when the server has exactly one list.
    pub fn new(client: Client, default_list: Option<String>) -> Tools {
        Tools {
            client,
            default_list,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Whether `name` is one of these tools.
    pub fn handles(name: &str) -> bool {
        NAMES.contains(&name)
    }

    /// Definitions of every tool, in a stable order.
    pub fn specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: LISTS,
                description: "List every shopping list with how many items are open and the total. \
                    Other shopping tools take an optional `list` name, matched case-insensitively and \
                    by prefix; omitting it uses the household's default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: SHOW,
                description: "Show a shopping list as text: each section with its items, open items \
                    as [ ] with quantity and note, checked-off items as [x]. Omit `list` for the \
                    household's default list; list names are matched case-insensitively and by prefix.",
                input_schema: json!({
                    "type": "object",
                    "properties": { "list": list_property() },
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: ADD,
                description: "Add items to a shopping list. An item already on the list is left as \
                    is; one that was checked off is put back. `section` is matched case-insensitively \
                    and by prefix, created if nothing matches, and defaults to the list's first \
                    section. Omit `list` for the household's default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "list": list_property(),
                        "items": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "What to buy." },
                                    "quantity": { "type": "integer", "minimum": 1, "description": "How many, if said." },
                                    "description": { "type": "string", "description": "Short note such as brand or size." },
                                    "section": { "type": "string", "description": "Section of the list, e.g. a store aisle. Defaults to the first section." }
                                },
                                "required": ["name"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["items"],
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: CHECK,
                description: "Check items off a shopping list, or set `checked` to false to put them \
                    back. Item names are matched case-insensitively and by prefix against the items \
                    on the list. Omit `list` for the household's default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "list": list_property(),
                        "items": item_names_property("Names of items on the list."),
                        "checked": { "type": "boolean", "default": true, "description": "true to check off, false to uncheck." }
                    },
                    "required": ["items"],
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: REMOVE,
                description: "Delete items from a shopping list entirely. For things that were bought, \
                    prefer shopping_list_check. Item names are matched case-insensitively and by \
                    prefix. Omit `list` for the household's default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "list": list_property(),
                        "items": item_names_property("Names of items to delete.")
                    },
                    "required": ["items"],
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: CLEAR_CHECKED,
                description: "Delete every checked-off item from a shopping list, for example after \
                    shopping. Omit `list` for the household's default list; list names are matched \
                    case-insensitively and by prefix.",
                input_schema: json!({
                    "type": "object",
                    "properties": { "list": list_property() },
                    "additionalProperties": false
                }),
            },
        ]
    }

    /// Runs the tool `name` with `input` (a JSON object, or null for no
    /// arguments). Never panics on bad input or server errors: every failure
    /// is a reply with `is_error` set and a message the model can act on.
    pub async fn call(&self, name: &str, input: &Value) -> ToolReply {
        let outcome = match name {
            LISTS => self.lists().await,
            SHOW => self.show(input).await,
            ADD => self.add(input).await,
            CHECK => self.check(input).await,
            REMOVE => self.remove(input).await,
            CLEAR_CHECKED => self.clear_checked(input).await,
            _ => Err(format!(
                "unknown tool \"{name}\"; available tools: {}",
                NAMES.join(", ")
            )),
        };
        outcome.unwrap_or_else(ToolReply::error)
    }

    /// Resolves the list a call refers to: the named one, else the configured
    /// default, else the only list on the server.
    async fn resolve_list(&self, query: Option<&str>) -> std::result::Result<List, String> {
        let lists = self.client.lists().await.map_err(|e| e.to_string())?;
        let query = query
            .filter(|q| !q.trim().is_empty())
            .or(self.default_list.as_deref());
        match query {
            Some(q) => match resolve("list", q, lists.iter().map(|l| (l, l.name.as_str()))) {
                Ok(list) => Ok(list.clone()),
                Err(e @ Error::NotFound { .. }) => {
                    Err(format!("{e}; existing lists: {}", names_of(&lists)))
                }
                Err(e) => Err(e.to_string()),
            },
            None => match lists.as_slice() {
                [only] => Ok(only.clone()),
                _ => Err(Error::NoDefaultList {
                    lists: lists.iter().map(|l| l.name.clone()).collect(),
                }
                .to_string()),
            },
        }
    }

    async fn list_and_sections(
        &self,
        query: Option<&str>,
    ) -> std::result::Result<(List, Vec<Section>), String> {
        let list = self.resolve_list(query).await?;
        let sections = self
            .client
            .sections(list.id)
            .await
            .map_err(|e| e.to_string())?;
        Ok((list, sections))
    }

    async fn lists(&self) -> Outcome {
        let lists = self.client.lists().await.map_err(|e| e.to_string())?;
        if lists.is_empty() {
            return Ok(ToolReply::ok("There are no shopping lists."));
        }
        let default_id = self.default_list.as_deref().and_then(|d| {
            resolve("list", d, lists.iter().map(|l| (l, l.name.as_str())))
                .ok()
                .map(|l| l.id)
        });
        let lines: Vec<String> = lists
            .iter()
            .map(|l| {
                let marker = if Some(l.id) == default_id {
                    " (default)"
                } else {
                    ""
                };
                format!(
                    "- {}{marker}: {} open of {}",
                    l.name,
                    l.stats.open_items(),
                    l.stats.total_items
                )
            })
            .collect();
        Ok(ToolReply::ok(lines.join("\n")))
    }

    async fn show(&self, input: &Value) -> Outcome {
        let input: ListInput = parse(input)?;
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        Ok(ToolReply::ok(render_list(&list, &sections)))
    }

    async fn add(&self, input: &Value) -> Outcome {
        let input: AddInput = parse(input)?;
        if input.items.is_empty() {
            return Err("`items` is empty; give at least one item to add".into());
        }
        let (list, mut sections) = self.list_and_sections(input.list.as_deref()).await?;

        let mut lines = Vec::new();
        let mut failures = 0;
        for wanted in &input.items {
            let name = wanted.name.trim();
            match self.add_one(&list, &mut sections, wanted).await {
                Ok(line) => lines.push(format!("- {line}")),
                Err(e) => {
                    failures += 1;
                    lines.push(format!("- {name}: not added: {e}"));
                }
            }
        }
        Ok(per_item_reply(format!("{}:", list.name), lines, failures))
    }

    /// Adds one item and returns its report line. Sections created along the
    /// way are appended to `sections` so later items in the same call reuse them.
    async fn add_one(
        &self,
        list: &List,
        sections: &mut Vec<Section>,
        wanted: &AddItem,
    ) -> std::result::Result<String, String> {
        let name = wanted.name.trim();
        if name.is_empty() {
            return Err("the item name is empty".into());
        }
        let found = match wanted
            .section
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(query) => {
                match resolve(
                    "section",
                    query,
                    sections.iter().map(|s| (s, s.name.as_str())),
                ) {
                    Ok(s) => Some((s.id, s.name.clone())),
                    Err(Error::NotFound { .. }) => None,
                    Err(e) => return Err(e.to_string()),
                }
            }
            None => sections.first().map(|s| (s.id, s.name.clone())),
        };
        let (section_id, section_name, created) = match found {
            Some((id, name)) => (id, name, false),
            None => {
                let new_name = wanted
                    .section
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(FALLBACK_SECTION);
                let section = self
                    .client
                    .create_section(list.id, new_name)
                    .await
                    .map_err(|e| e.to_string())?;
                let found = (section.id, section.name.clone(), true);
                sections.push(section);
                found
            }
        };

        let outcome = self
            .client
            .create_item(&CreateItem {
                section_id,
                name: name.to_string(),
                description: wanted
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(String::from),
                quantity: wanted.quantity.filter(|q| *q > 0),
            })
            .await
            .map_err(|e| e.to_string())?;
        let item = &outcome.item.name;
        Ok(match outcome.status {
            CreateStatus::Created if created => {
                format!("{item}: added to {section_name} (new section)")
            }
            CreateStatus::Created => format!("{item}: added to {section_name}"),
            CreateStatus::AlreadyActive => format!("{item}: already on the list"),
            CreateStatus::Reactivated => format!("{item}: re-added (was checked off)"),
        })
    }

    async fn check(&self, input: &Value) -> Outcome {
        let input: CheckInput = parse(input)?;
        if input.items.is_empty() {
            return Err("`items` is empty; name at least one item".into());
        }
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        let mut items: Vec<Item> = sections.into_iter().flat_map(|s| s.items).collect();

        let mut lines = Vec::new();
        let mut failures = 0;
        for query in &input.items {
            // Prefer items not yet in the requested state, so "milk" picks the
            // open milk over an already checked-off one.
            let pending = items.iter().filter(|i| i.completed != input.checked);
            let found = match resolve("item", query, pending.map(|i| (i, i.name.as_str()))) {
                Err(Error::NotFound { .. }) => {
                    resolve("item", query, items.iter().map(|i| (i, i.name.as_str())))
                }
                other => other,
            }
            .map(|i| (i.id, i.name.clone(), i.completed));

            let line = match found {
                Err(e) => Err(e.to_string()),
                Ok((_, name, completed)) if completed == input.checked => Ok(if completed {
                    format!("{name}: was already checked off")
                } else {
                    format!("{name}: was not checked off")
                }),
                Ok((id, name, _)) => {
                    match self.client.set_item_completed(id, input.checked).await {
                        Ok(updated) => {
                            if let Some(local) = items.iter_mut().find(|i| i.id == id) {
                                local.completed = updated.completed;
                            }
                            Ok(if input.checked {
                                format!("{name}: checked off")
                            } else {
                                format!("{name}: back on the list")
                            })
                        }
                        Err(e) => Err(e.to_string()),
                    }
                }
            };
            match line {
                Ok(line) => lines.push(format!("- {line}")),
                Err(e) => {
                    failures += 1;
                    lines.push(format!("- {}: {e}", query.trim()));
                }
            }
        }
        Ok(per_item_reply(format!("{}:", list.name), lines, failures))
    }

    async fn remove(&self, input: &Value) -> Outcome {
        let input: NamesInput = parse(input)?;
        if input.items.is_empty() {
            return Err("`items` is empty; name at least one item".into());
        }
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        let mut items: Vec<Item> = sections.into_iter().flat_map(|s| s.items).collect();

        let mut lines = Vec::new();
        let mut failures = 0;
        for query in &input.items {
            let found = resolve("item", query, items.iter().map(|i| (i, i.name.as_str())))
                .map(|i| (i.id, i.name.clone()));
            let line = match found {
                Err(e) => Err(e.to_string()),
                Ok((id, name)) => match self.client.delete_item(id).await {
                    Ok(()) => {
                        items.retain(|i| i.id != id);
                        Ok(format!("{name}: removed"))
                    }
                    Err(e) => Err(e.to_string()),
                },
            };
            match line {
                Ok(line) => lines.push(format!("- {line}")),
                Err(e) => {
                    failures += 1;
                    lines.push(format!("- {}: {e}", query.trim()));
                }
            }
        }
        Ok(per_item_reply(format!("{}:", list.name), lines, failures))
    }

    async fn clear_checked(&self, input: &Value) -> Outcome {
        let input: ListInput = parse(input)?;
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        let checked: Vec<Item> = sections
            .into_iter()
            .flat_map(|s| s.items)
            .filter(|i| i.completed)
            .collect();
        if checked.is_empty() {
            return Ok(ToolReply::ok(format!(
                "{} has no checked-off items.",
                list.name
            )));
        }

        let mut removed = Vec::new();
        let mut failed = Vec::new();
        for item in &checked {
            match self.client.delete_item(item.id).await {
                Ok(()) => removed.push(item.name.as_str()),
                Err(e) => failed.push(format!("{}: {e}", item.name)),
            }
        }
        let mut content = format!(
            "Removed {} checked-off item{} from {}",
            removed.len(),
            if removed.len() == 1 { "" } else { "s" },
            list.name
        );
        if !removed.is_empty() {
            content.push_str(": ");
            content.push_str(&removed.join(", "));
        }
        content.push('.');
        if !failed.is_empty() {
            content.push_str("\nCould not remove:\n- ");
            content.push_str(&failed.join("\n- "));
        }
        Ok(ToolReply {
            content,
            is_error: removed.is_empty(),
        })
    }
}
