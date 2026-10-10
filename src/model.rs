//! Types matching Koffan's `/api/v1` JSON.
//!
//! Every struct deserializes leniently: missing fields take their default, and
//! array fields accept JSON `null` (which Koffan sends for empty collections)
//! as an empty `Vec`. Timestamps are kept as they arrive: `created_at` is an
//! RFC 3339 string and `updated_at` is unix seconds.

use serde::{Deserialize, Deserializer, Serialize};

pub(crate) fn null_as_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct List {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub sort_order: i64,
    /// Whether this is the list currently open in the web UI. The API never changes it.
    pub is_active: bool,
    pub show_completed: bool,
    pub created_at: String,
    pub updated_at: i64,
    pub stats: Stats,
}

/// Item counts across every section of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub total_items: i64,
    pub completed_items: i64,
    pub percentage: i64,
}

impl Stats {
    /// Items not yet checked off.
    pub fn open_items(&self) -> i64 {
        self.total_items - self.completed_items
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Section {
    pub id: i64,
    pub list_id: i64,
    pub name: String,
    pub sort_order: i64,
    pub sort_mode: SortMode,
    pub created_at: String,
    pub updated_at: i64,
    /// Items in display order: open items first, then completed, each group by the section's sort mode.
    #[serde(deserialize_with = "null_as_empty")]
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub id: i64,
    pub section_id: i64,
    pub name: String,
    pub description: String,
    pub completed: bool,
    pub uncertain: bool,
    /// 0 means no quantity was given.
    pub quantity: i64,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: i64,
}

/// An entry in the workspace's suggestion history.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryItem {
    pub id: i64,
    pub name: String,
    pub last_section_id: i64,
    pub last_section_name: String,
    pub usage_count: i64,
}

/// How a section orders its items (open items always come before completed ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    Manual,
    Alphabetical,
    AlphabeticalDesc,
    /// A mode this crate does not know about. Sending it to the server fails.
    #[serde(other)]
    Unknown,
}

/// Body of `POST /items`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreateItem {
    pub section_id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
}

/// What `POST /items` did. Koffan matches the name case-insensitively against
/// items in the same section before creating a new row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateItemOutcome {
    pub item: Item,
    pub status: CreateStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateStatus {
    /// A new item was created.
    Created,
    /// A completed item with the same name was un-completed. Its description and
    /// quantity were replaced by the request's, where those were non-empty.
    Reactivated,
    /// An open item with the same name already existed and was left unchanged.
    AlreadyActive,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawCreateItemResponse {
    Wrapped {
        item: Item,
        #[serde(default)]
        reactivated: bool,
        #[serde(default)]
        already_active: bool,
    },
    Bare(Item),
}

impl<'de> Deserialize<'de> for CreateItemOutcome {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match RawCreateItemResponse::deserialize(deserializer)? {
            RawCreateItemResponse::Bare(item) => CreateItemOutcome {
                item,
                status: CreateStatus::Created,
            },
            RawCreateItemResponse::Wrapped {
                item,
                reactivated,
                already_active,
            } => CreateItemOutcome {
                item,
                status: if already_active {
                    CreateStatus::AlreadyActive
                } else if reactivated {
                    CreateStatus::Reactivated
                } else {
                    CreateStatus::Created
                },
            },
        })
    }
}

/// Body of `PUT /items/:id`.
///
/// `name` is required because Koffan clears an item's description when an
/// update carries neither a name nor a description. An empty `description`
/// keeps the existing one, so a description cannot be cleared through this call.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ItemUpdate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
}

/// Body of `PUT /lists/:id`. `None` keeps the current value; the server also
/// keeps the current name or icon when given an empty string.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_completed: Option<bool>,
}

/// Response of check-all and uncheck-all on a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckAllOutcome {
    /// How many items changed state.
    pub count: i64,
    pub section_id: i64,
}

/// An item inside a batch request. Batch requests create every item as a new
/// row; they do not merge with same-named items the way [`CreateItem`] does.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BatchItem {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BatchSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<BatchItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BatchList {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<BatchSection>,
}

/// Response of [`Client::batch_create_list`](crate::Client::batch_create_list).
/// `list.show_completed` is always `false` in this response regardless of the stored value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchListCreated {
    pub list: List,
    #[serde(deserialize_with = "null_as_empty")]
    pub sections: Vec<Section>,
    /// Every created item, flat.
    #[serde(deserialize_with = "null_as_empty")]
    pub items: Vec<Item>,
}

/// Response of [`Client::batch_add_sections`](crate::Client::batch_add_sections).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchSectionsAdded {
    #[serde(deserialize_with = "null_as_empty")]
    pub sections: Vec<Section>,
    #[serde(deserialize_with = "null_as_empty")]
    pub items: Vec<Item>,
}

/// Response of [`Client::batch_add_items`](crate::Client::batch_add_items).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchItemsAdded {
    #[serde(deserialize_with = "null_as_empty")]
    pub items: Vec<Item>,
}
