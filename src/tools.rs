//! Shopping-list tools for LLM agents, independent of any agent framework.
//!
//! [`Tools::specs`] describes the tools as JSON Schema; [`Tools::call`] runs
//! one by name. Lists, sections and items are addressed by name and matched
//! with [`crate::matching`], so the model never sees or needs ids. Replies
//! are short plain text suitable for reading aloud.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::matching::{best_tier, normalize, resolve};
use crate::model::{CreateItem, CreateStatus, Item, ItemUpdate, List, Section};
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

fn item_refs_property(description: &str) -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "description": description,
        "items": {
            "anyOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "section": { "type": "string", "description": "Section the item is in, to tell apart items with the same name." }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }
            ]
        }
    })
}

/// An item named by the model: a bare name, or a name with the section it is in.
#[derive(Deserialize)]
#[serde(untagged)]
enum ItemRef {
    Name(String),
    Full {
        name: String,
        #[serde(default)]
        section: Option<String>,
    },
}

impl ItemRef {
    fn name(&self) -> &str {
        match self {
            ItemRef::Name(name) | ItemRef::Full { name, .. } => name.trim(),
        }
    }

    fn section(&self) -> Option<&str> {
        match self {
            ItemRef::Name(_) => None,
            ItemRef::Full { section, .. } => {
                section.as_deref().map(str::trim).filter(|s| !s.is_empty())
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AddEntry {
    Name(String),
    Full(AddItem),
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

impl AddEntry {
    fn into_item(self) -> AddItem {
        match self {
            AddEntry::Name(name) => AddItem {
                name,
                quantity: None,
                description: None,
                section: None,
            },
            AddEntry::Full(item) => item,
        }
    }
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
    items: Vec<AddEntry>,
}

#[derive(Deserialize)]
struct CheckInput {
    #[serde(default)]
    list: Option<String>,
    items: Vec<ItemRef>,
    #[serde(default = "default_checked")]
    checked: bool,
}

fn default_checked() -> bool {
    true
}

#[derive(Deserialize)]
struct RemoveInput {
    #[serde(default)]
    list: Option<String>,
    items: Vec<ItemRef>,
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

/// A list's sections and its items, each item paired with its section name.
struct Contents {
    sections: Vec<Section>,
    entries: Vec<Entry>,
}

#[derive(Clone)]
struct Entry {
    item: Item,
    section: String,
}

impl Entry {
    fn label(&self) -> String {
        format!("{} ({})", self.item.name, self.section)
    }
}

impl Contents {
    fn new(mut sections: Vec<Section>) -> Contents {
        let entries = sections
            .iter_mut()
            .flat_map(|s| {
                let section = s.name.clone();
                std::mem::take(&mut s.items)
                    .into_iter()
                    .map(move |item| Entry {
                        item,
                        section: section.clone(),
                    })
            })
            .collect();
        Contents { sections, entries }
    }

    /// Finds the item `r` refers to and returns its index in `entries`.
    ///
    /// Candidates are narrowed to `r`'s section when given, then matched by
    /// tier (exact, prefix, substring). When `prefer_completed` is given,
    /// items in the winning tier whose completed flag differs from it (the
    /// ones the call would change) are preferred. Hits that remain and differ in
    /// name or section are reported as ambiguous with "name (section)"
    /// candidates; hits indistinguishable by name and section resolve to the
    /// first, since the model has no way to tell them apart.
    fn find(
        &self,
        r: &ItemRef,
        prefer_completed: Option<bool>,
    ) -> std::result::Result<usize, String> {
        let section_id = match r.section() {
            Some(q) => Some(
                resolve(
                    "section",
                    q,
                    self.sections.iter().map(|s| (s, s.name.as_str())),
                )
                .map_err(|e| e.to_string())?
                .id,
            ),
            None => None,
        };
        let pool: Vec<(usize, &Entry)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| section_id.is_none_or(|id| e.item.section_id == id))
            .collect();
        let Some((_, hits)) = best_tier(r.name(), pool.iter().map(|p| (p, p.1.item.name.as_str())))
        else {
            return Err(Error::NotFound {
                what: "item",
                name: r.name().to_string(),
            }
            .to_string());
        };
        let mut hits: Vec<&(usize, &Entry)> = hits.into_iter().map(|(p, _)| p).collect();
        if let Some(target) = prefer_completed {
            let pending: Vec<_> = hits
                .iter()
                .copied()
                .filter(|(_, e)| e.item.completed != target)
                .collect();
            if !pending.is_empty() {
                hits = pending;
            }
        }
        let mut labels: Vec<String> = Vec::new();
        for (_, e) in &hits {
            let label = e.label();
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        if labels.len() == 1 {
            Ok(hits[0].0)
        } else {
            Err(Error::Ambiguous {
                what: "item",
                name: r.name().to_string(),
                candidates: labels,
            }
            .to_string())
        }
    }
}

/// Joins a header and per-item lines into one reply, flagged as an error
/// when any item failed so the caller notices partial failures.
fn per_item_reply(header: String, lines: Vec<std::result::Result<String, String>>) -> ToolReply {
    let mut content = header;
    let mut failed = false;
    for line in lines {
        content.push_str("\n- ");
        match line {
            Ok(line) => content.push_str(&line),
            Err(line) => {
                failed = true;
                content.push_str(&line);
            }
        }
    }
    ToolReply {
        content,
        is_error: failed,
    }
}

/// Renders a list for reading aloud: a header with open/total counts, one
/// line naming every section (empty ones included) in list order, the open
/// items grouped under their section names, and one line naming the
/// checked-off items.
pub fn render_list(list: &List, sections: &[Section]) -> String {
    let all = sections.iter().flat_map(|s| s.items.iter());
    let total = all.clone().count();
    let checked: Vec<&str> = all
        .filter(|i| i.completed)
        .map(|i| i.name.as_str())
        .collect();
    let mut out = format!("{}: {} open of {total}", list.name, total - checked.len());
    if !sections.is_empty() {
        let names: Vec<&str> = sections.iter().map(|s| s.name.as_str()).collect();
        out.push_str("\nSections: ");
        out.push_str(&names.join(", "));
    }

    for section in sections {
        let mut open = section.items.iter().filter(|i| !i.completed).peekable();
        if open.peek().is_none() {
            continue;
        }
        out.push_str("\n\n");
        out.push_str(&section.name);
        for item in open {
            out.push_str("\n- ");
            out.push_str(&item.name);
            if item.quantity > 0 {
                out.push_str(&format!(" ×{}", item.quantity));
            }
            if !item.description.is_empty() {
                out.push_str(&format!(" ({})", item.description));
            }
            if item.uncertain {
                out.push_str(" (uncertain)");
            }
        }
    }
    if !checked.is_empty() {
        out.push_str("\n\nChecked off: ");
        out.push_str(&checked.join(", "));
    }
    out
}

fn state_word(completed: bool) -> &'static str {
    if completed {
        "checked off"
    } else {
        "open"
    }
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
                description: "Show a shopping list as text: all section names, open items grouped by section with \
                    quantity and note, then one line naming the checked-off items. Omit `list` for \
                    the household's default list; list names are matched case-insensitively and by prefix.",
                input_schema: json!({
                    "type": "object",
                    "properties": { "list": list_property() },
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: ADD,
                description: "Add items to a shopping list. A name already on the list is not \
                    duplicated: an open item is kept, a checked-off one is put back. `section` is \
                    matched case-insensitively and by prefix, created if nothing matches, and \
                    defaults to the list's first section; shopping_list_show lists the existing \
                    section names. Omit `list` for the default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "list": list_property(),
                        "items": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "anyOf": [
                                    { "type": "string", "description": "What to buy." },
                                    {
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
                                ]
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
                    back. Item names are matched case-insensitively: exact, then prefix, then \
                    substring. When a name is in several sections, give {name, section}. Omit `list` \
                    for the household's default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "list": list_property(),
                        "items": item_refs_property("Items on the list, by name or as {name, section}."),
                        "checked": { "type": "boolean", "default": true, "description": "true to check off, false to uncheck." }
                    },
                    "required": ["items"],
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: REMOVE,
                description: "Delete items from a shopping list entirely. For things that were bought, \
                    prefer shopping_list_check. Item names are matched case-insensitively: exact, then \
                    prefix, then substring. When a name is in several sections, give {name, section}. \
                    Omit `list` for the household's default list.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "list": list_property(),
                        "items": item_refs_property("Items to delete, by name or as {name, section}.")
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
    /// When some items of a multi-item call succeed and others fail, the
    /// reply lists both and is flagged as an error.
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
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        let mut contents = Contents::new(sections);

        let mut lines = Vec::new();
        for entry in input.items {
            let wanted = entry.into_item();
            let name = wanted.name.trim().to_string();
            lines.push(
                self.add_one(&list, &mut contents, &wanted)
                    .await
                    .map_err(|e| format!("{name}: not added: {e}")),
            );
        }
        Ok(per_item_reply(format!("{}:", list.name), lines))
    }

    /// Adds one item and returns its report line.
    ///
    /// An item with the same normalized name anywhere on the list is reused
    /// instead of creating a duplicate, preferring an open one. Koffan's own
    /// merge only looks inside the target section and ignores case for ASCII
    /// letters only, so it would not catch these. Sections and items created
    /// here are added to `contents` so later items in the same call see them.
    async fn add_one(
        &self,
        list: &List,
        contents: &mut Contents,
        wanted: &AddItem,
    ) -> std::result::Result<String, String> {
        let name = wanted.name.trim();
        if name.is_empty() {
            return Err("the item name is empty".into());
        }
        let quantity = wanted.quantity.filter(|q| *q > 0);
        let description = wanted
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(String::from);

        // A same-named item in the requested section wins; otherwise any
        // same-named item on the list is reused. An open one beats a
        // checked-off one in either case.
        let key = normalize(name);
        let wanted_section = wanted
            .section
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|q| {
                resolve(
                    "section",
                    q,
                    contents.sections.iter().map(|s| (s, s.name.as_str())),
                )
                .ok()
                .map(|s| s.id)
            });
        let same_name: Vec<(usize, &Entry)> = contents
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| normalize(&e.item.name) == key)
            .collect();
        let in_section: Vec<(usize, &Entry)> = same_name
            .iter()
            .copied()
            .filter(|(_, e)| Some(e.item.section_id) == wanted_section)
            .collect();
        let pool = if in_section.is_empty() {
            same_name
        } else {
            in_section
        };
        let existing = pool
            .into_iter()
            .min_by_key(|(_, e)| e.item.completed)
            .map(|(i, _)| i);
        if let Some(i) = existing {
            return self
                .reuse_existing(&mut contents.entries[i], quantity, description)
                .await;
        }

        let (section_id, section_name, created) =
            self.target_section(list, contents, wanted).await?;
        let outcome = self
            .client
            .create_item(&CreateItem {
                section_id,
                name: name.to_string(),
                description: description.clone(),
                quantity,
            })
            .await
            .map_err(|e| e.to_string())?;
        let item_name = outcome.item.name.clone();
        contents.entries.push(Entry {
            item: outcome.item,
            section: section_name.clone(),
        });
        let entry = contents.entries.last_mut().expect("just pushed");
        Ok(match outcome.status {
            CreateStatus::Created if created => {
                format!("{item_name}: added to {section_name} (new section)")
            }
            CreateStatus::Created => format!("{item_name}: added to {section_name}"),
            // Someone added the item after our snapshot, or it was checked off
            // in this section; the server may have kept its old quantity or
            // description, so apply the requested ones.
            CreateStatus::AlreadyActive => {
                let changes = self.apply_details(entry, quantity, description).await?;
                format!("{item_name}: already on the list (in {section_name}){changes}")
            }
            CreateStatus::Reactivated => {
                let changes = self.apply_details(entry, quantity, description).await?;
                format!("{item_name}: re-added (was checked off){changes}")
            }
        })
    }

    /// Sets an item's quantity and description to the requested values where
    /// given and different, and returns a reply suffix naming what changed
    /// (empty when nothing did).
    async fn apply_details(
        &self,
        entry: &mut Entry,
        quantity: Option<i64>,
        description: Option<String>,
    ) -> std::result::Result<String, String> {
        let quantity = quantity.filter(|q| *q != entry.item.quantity);
        let description = description.filter(|d| *d != entry.item.description);
        let mut changes = String::new();
        if let Some(d) = &description {
            entry.item = self
                .client
                .update_item(
                    entry.item.id,
                    &ItemUpdate {
                        name: entry.item.name.clone(),
                        description: Some(d.clone()),
                        quantity,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            changes.push_str(&format!(
                ", description set to \"{}\"",
                entry.item.description
            ));
        } else if let Some(q) = quantity {
            entry.item = self
                .client
                .set_quantity(entry.item.id, q)
                .await
                .map_err(|e| e.to_string())?;
        }
        if quantity.is_some() {
            changes.push_str(&format!(", quantity set to {}", entry.item.quantity));
        }
        Ok(changes)
    }

    /// Brings an item that is already on the list in line with an add request:
    /// reopens it if checked off and applies the given quantity and description.
    async fn reuse_existing(
        &self,
        entry: &mut Entry,
        quantity: Option<i64>,
        description: Option<String>,
    ) -> std::result::Result<String, String> {
        let name = entry.item.name.clone();
        if !entry.item.completed {
            let changes = self.apply_details(entry, quantity, description).await?;
            return Ok(format!(
                "{name}: already on the list (in {}){changes}",
                entry.section
            ));
        }

        entry.item = self
            .client
            .set_item_completed(entry.item.id, false)
            .await
            .map_err(|e| e.to_string())?;
        if entry.item.completed {
            return Err("still checked off (changed by someone else meanwhile)".into());
        }
        let changes = self.apply_details(entry, quantity, description).await?;
        Ok(format!("{name}: re-added (was checked off){changes}"))
    }

    /// Picks the section a new item goes into: the named section, created if
    /// nothing matches; else the first section, or a new [`FALLBACK_SECTION`]
    /// when the list has none. Returns its id, its name and whether it was created.
    async fn target_section(
        &self,
        list: &List,
        contents: &mut Contents,
        wanted: &AddItem,
    ) -> std::result::Result<(i64, String, bool), String> {
        let query = wanted
            .section
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let found = match query {
            Some(q) => match resolve(
                "section",
                q,
                contents.sections.iter().map(|s| (s, s.name.as_str())),
            ) {
                Ok(s) => Some(s),
                Err(Error::NotFound { .. }) => None,
                Err(e) => return Err(e.to_string()),
            },
            None => contents.sections.first(),
        };
        if let Some(s) = found {
            return Ok((s.id, s.name.clone(), false));
        }
        let section = self
            .client
            .create_section(list.id, query.unwrap_or(FALLBACK_SECTION))
            .await
            .map_err(|e| e.to_string())?;
        let found = (section.id, section.name.clone(), true);
        contents.sections.push(section);
        Ok(found)
    }

    async fn check(&self, input: &Value) -> Outcome {
        let input: CheckInput = parse(input)?;
        if input.items.is_empty() {
            return Err("`items` is empty; name at least one item".into());
        }
        let target = input.checked;
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        let mut contents = Contents::new(sections);

        let mut lines = Vec::new();
        for r in &input.items {
            let line = match contents.find(r, Some(target)) {
                Err(e) => Err(format!("{}: {e}", r.name())),
                Ok(i) => {
                    let entry = &mut contents.entries[i];
                    let name = entry.item.name.clone();
                    if entry.item.completed == target {
                        Ok(format!("{name}: was already {}", state_word(target)))
                    } else {
                        match self.client.set_item_completed(entry.item.id, target).await {
                            Err(e) => Err(format!("{name}: {e}")),
                            Ok(updated) => {
                                entry.item = updated;
                                if entry.item.completed != target {
                                    Err(format!(
                                        "{name}: now {} (changed by someone else meanwhile)",
                                        state_word(entry.item.completed)
                                    ))
                                } else if target {
                                    Ok(format!("{name}: checked off"))
                                } else {
                                    Ok(format!("{name}: back on the list"))
                                }
                            }
                        }
                    }
                }
            };
            lines.push(line);
        }
        Ok(per_item_reply(format!("{}:", list.name), lines))
    }

    async fn remove(&self, input: &Value) -> Outcome {
        let input: RemoveInput = parse(input)?;
        if input.items.is_empty() {
            return Err("`items` is empty; name at least one item".into());
        }
        let (list, sections) = self.list_and_sections(input.list.as_deref()).await?;
        let mut contents = Contents::new(sections);

        let mut lines = Vec::new();
        for r in &input.items {
            let line = match contents.find(r, None) {
                Err(e) => Err(format!("{}: {e}", r.name())),
                Ok(i) => {
                    let name = contents.entries[i].item.name.clone();
                    match self.client.delete_item(contents.entries[i].item.id).await {
                        Ok(()) => {
                            contents.entries.remove(i);
                            Ok(format!("{name}: removed"))
                        }
                        Err(e) => Err(format!("{name}: {e}")),
                    }
                }
            };
            lines.push(line);
        }
        Ok(per_item_reply(format!("{}:", list.name), lines))
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
        let mut skipped = Vec::new();
        let mut failed = Vec::new();
        for item in &checked {
            // The list may have changed since it was read; only delete items
            // that are still checked off right now. The API has no
            // conditional delete, so an item reopened between this read and
            // the delete is still deleted.
            match self.client.item(item.id).await {
                Ok(current) if !current.completed => {
                    skipped.push(format!("{}: put back on the list meanwhile", item.name));
                    continue;
                }
                Ok(_) => {}
                Err(Error::Api { status: 404, .. }) => {
                    skipped.push(format!("{}: already removed", item.name));
                    continue;
                }
                Err(e) => {
                    failed.push(format!("{}: {e}", item.name));
                    continue;
                }
            }
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
        if !skipped.is_empty() {
            content.push_str("\nKept:\n- ");
            content.push_str(&skipped.join("\n- "));
        }
        if !failed.is_empty() {
            content.push_str("\nCould not remove:\n- ");
            content.push_str(&failed.join("\n- "));
        }
        Ok(ToolReply {
            content,
            is_error: !failed.is_empty(),
        })
    }
}
