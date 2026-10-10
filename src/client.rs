use std::fmt;
use std::time::Duration;

use reqwest::{Method, RequestBuilder, Response, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::model::*;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(15);

/// Longest non-Koffan error body kept in [`Error::Api::message`].
const MAX_ERROR_TEXT: usize = 200;

/// Async client for Koffan's `/api/v1` REST API.
///
/// The token is the server's `API_TOKEN`. It gives access to exactly one
/// workspace; ids from any other workspace answer 404 like missing ones.
/// Cloning is cheap and shares the connection pool. `Debug` output omits the token.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    /// The server URL with `/api/v1` appended; endpoint paths are added as segments.
    base: Url,
    token: String,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct ListsBody {
    #[serde(default, deserialize_with = "null_as_empty")]
    lists: Vec<List>,
}

#[derive(Deserialize)]
struct SectionsBody {
    #[serde(default, deserialize_with = "null_as_empty")]
    sections: Vec<Section>,
}

#[derive(Deserialize)]
struct ItemsBody {
    #[serde(default, deserialize_with = "null_as_empty")]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct HistoryBody {
    #[serde(default, deserialize_with = "null_as_empty")]
    items: Vec<HistoryItem>,
}

#[derive(Deserialize)]
struct HistoryAddedBody {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct DeletedBody {
    deleted: u64,
}

#[derive(Serialize)]
struct NewList<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<&'a str>,
}

impl Client {
    /// Creates a client for the Koffan server at `base_url` (for example
    /// `https://koffan.example.com`, or `https://example.com/koffan` behind a
    /// path prefix); `/api/v1` is appended. The URL must be http(s) and must
    /// not carry a query or fragment.
    pub fn new(base_url: impl AsRef<str>, token: impl Into<String>) -> Result<Client> {
        let raw = base_url.as_ref().trim();
        let invalid = |why: &str| Error::InvalidUrl(format!("{raw}: {why}"));
        let mut base = Url::parse(raw).map_err(|e| invalid(&e.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(invalid("scheme must be http or https"));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(invalid("must not contain a query or fragment"));
        }
        if base.cannot_be_a_base() {
            return Err(invalid("cannot be used as a base URL"));
        }
        let prefix = base.path().trim_end_matches('/').to_string();
        base.set_path(&format!("{prefix}/api/v1"));
        let http = reqwest::Client::builder().timeout(TIMEOUT).build()?;
        Ok(Client {
            http,
            base,
            token: token.into(),
        })
    }

    /// The URL for an endpoint path such as `/lists/3/sections`.
    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("the base URL was checked in Client::new")
            .extend(path.split('/').filter(|s| !s.is_empty()));
        url
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, self.endpoint(path))
            .bearer_auth(&self.token)
    }

    async fn check(resp: Response) -> Result<Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let reason = status
            .canonical_reason()
            .unwrap_or("unknown status")
            .to_string();
        let text = resp.text().await.unwrap_or_default();
        Err(match serde_json::from_str::<ErrorBody>(&text) {
            Ok(body) => Error::Api {
                status: status.as_u16(),
                code: body.error,
                message: body.message,
            },
            Err(_) => {
                let text = text.trim();
                Error::Api {
                    status: status.as_u16(),
                    code: reason.clone(),
                    message: if text.is_empty() {
                        reason
                    } else {
                        text.chars().take(MAX_ERROR_TEXT).collect()
                    },
                }
            }
        })
    }

    async fn send<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T> {
        let resp = Self::check(req.send().await?).await?;
        let url = resp.url().to_string();
        let body = resp.bytes().await?;
        serde_json::from_slice(&body).map_err(|source| Error::UnexpectedResponse { url, source })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.request(Method::GET, path)).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.request(Method::POST, path)).await
    }

    async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send(self.request(Method::POST, path).json(body)).await
    }

    async fn put_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send(self.request(Method::PUT, path).json(body)).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        Self::check(self.request(Method::DELETE, path).send().await?).await?;
        Ok(())
    }

    // Lists

    /// All lists in display order, with item stats.
    pub async fn lists(&self) -> Result<Vec<List>> {
        Ok(self.get::<ListsBody>("/lists").await?.lists)
    }

    pub async fn list(&self, id: i64) -> Result<List> {
        self.get(&format!("/lists/{id}")).await
    }

    /// Creates a list. Names are unique ignoring ASCII case; a duplicate fails
    /// with `Error::Api { status: 400, code: "list_name_exists", .. }`. `icon` is
    /// an emoji or an alias such as `cart` or `home`; anything else becomes 🛒.
    pub async fn create_list(&self, name: &str, icon: Option<&str>) -> Result<List> {
        self.post_json("/lists", &NewList { name, icon }).await
    }

    pub async fn update_list(&self, id: i64, update: &ListUpdate) -> Result<List> {
        self.put_json(&format!("/lists/{id}"), update).await
    }

    /// Deletes a list with all its sections and items.
    pub async fn delete_list(&self, id: i64) -> Result<()> {
        self.delete(&format!("/lists/{id}")).await
    }

    pub async fn move_list_up(&self, id: i64) -> Result<List> {
        self.post(&format!("/lists/{id}/move-up")).await
    }

    pub async fn move_list_down(&self, id: i64) -> Result<List> {
        self.post(&format!("/lists/{id}/move-down")).await
    }

    // Sections

    /// The sections of a list in order, each with its items nested.
    pub async fn sections(&self, list_id: i64) -> Result<Vec<Section>> {
        Ok(self
            .get::<SectionsBody>(&format!("/lists/{list_id}/sections"))
            .await?
            .sections)
    }

    /// One section with its items.
    pub async fn section(&self, id: i64) -> Result<Section> {
        self.get(&format!("/sections/{id}")).await
    }

    pub async fn section_items(&self, id: i64) -> Result<Vec<Item>> {
        Ok(self
            .get::<ItemsBody>(&format!("/sections/{id}/items"))
            .await?
            .items)
    }

    /// Appends a section to a list. Section names need not be unique.
    pub async fn create_section(&self, list_id: i64, name: &str) -> Result<Section> {
        self.post_json("/sections", &json!({ "list_id": list_id, "name": name }))
            .await
    }

    pub async fn update_section(&self, id: i64, name: &str) -> Result<Section> {
        self.put_json(&format!("/sections/{id}"), &json!({ "name": name }))
            .await
    }

    /// Deletes a section and its items.
    pub async fn delete_section(&self, id: i64) -> Result<()> {
        self.delete(&format!("/sections/{id}")).await
    }

    /// Marks every item in the section completed.
    pub async fn check_all(&self, section_id: i64) -> Result<CheckAllOutcome> {
        self.post(&format!("/sections/{section_id}/check-all"))
            .await
    }

    /// Marks every item in the section open.
    pub async fn uncheck_all(&self, section_id: i64) -> Result<CheckAllOutcome> {
        self.post(&format!("/sections/{section_id}/uncheck-all"))
            .await
    }

    pub async fn set_sort_mode(&self, section_id: i64, mode: SortMode) -> Result<Section> {
        self.post_json(
            &format!("/sections/{section_id}/sort-mode"),
            &json!({ "sort_mode": mode }),
        )
        .await
    }

    pub async fn move_section_up(&self, id: i64) -> Result<Section> {
        self.post(&format!("/sections/{id}/move-up")).await
    }

    pub async fn move_section_down(&self, id: i64) -> Result<Section> {
        self.post(&format!("/sections/{id}/move-down")).await
    }

    // Items

    pub async fn item(&self, id: i64) -> Result<Item> {
        self.get(&format!("/items/{id}")).await
    }

    /// Adds an item to a section, merging with a same-named item in that
    /// section: an open one is left unchanged, a completed one is
    /// un-completed. [`CreateItemOutcome::status`] says which.
    ///
    /// The server's name comparison covers only the target section and folds
    /// case for ASCII letters only, so "Ägg" and "ägg", or the same name in
    /// another section, become separate rows. Check the list first (as
    /// [`crate::tools::Tools`] does) when duplicates matter.
    pub async fn create_item(&self, item: &CreateItem) -> Result<CreateItemOutcome> {
        self.post_json("/items", item).await
    }

    /// Updates an item's name, description and quantity.
    ///
    /// Koffan clears the description when an update carries neither a name
    /// nor a description, so [`ItemUpdate`] always carries the name; pass the
    /// current name to keep it. A blank name is refused with
    /// [`Error::InvalidInput`] without contacting the server. The completed
    /// and uncertain flags cannot be set here: use
    /// [`Client::set_item_completed`], [`Client::toggle_item`] or
    /// [`Client::toggle_uncertain`].
    pub async fn update_item(&self, id: i64, update: &ItemUpdate) -> Result<Item> {
        if update.name.trim().is_empty() {
            return Err(Error::InvalidInput(
                "update_item needs the item's name; without it Koffan clears the description"
                    .into(),
            ));
        }
        self.put_json(&format!("/items/{id}"), update).await
    }

    pub async fn delete_item(&self, id: i64) -> Result<()> {
        self.delete(&format!("/items/{id}")).await
    }

    /// Flips the completed flag. Not idempotent: calling it twice restores the
    /// original state. To reach a specific state use [`Client::set_item_completed`].
    pub async fn toggle_item(&self, id: i64) -> Result<Item> {
        self.post(&format!("/items/{id}/toggle")).await
    }

    /// Flips the uncertain flag. Not idempotent.
    pub async fn toggle_uncertain(&self, id: i64) -> Result<Item> {
        self.post(&format!("/items/{id}/uncertain")).await
    }

    /// Sets the item's completed flag to `completed`.
    ///
    /// The API has no "set completed" or conditional toggle call, so this is
    /// two requests: it reads the item and toggles it only when the state
    /// differs. It is not atomic; a concurrent change between the read and the
    /// toggle can be lost, leaving the item in the opposite state. The
    /// returned item shows the state the server ended up with.
    pub async fn set_item_completed(&self, id: i64, completed: bool) -> Result<Item> {
        let item = self.item(id).await?;
        if item.completed == completed {
            return Ok(item);
        }
        self.toggle_item(id).await
    }

    /// Sets the quantity; the server clamps negative values to 0.
    pub async fn set_quantity(&self, id: i64, quantity: i64) -> Result<Item> {
        self.post_json(
            &format!("/items/{id}/quantity"),
            &json!({ "quantity": quantity }),
        )
        .await
    }

    /// Adds `delta` to the quantity, clamping at 0. The server rejects a zero
    /// `delta` with a 400.
    pub async fn adjust_quantity(&self, id: i64, delta: i64) -> Result<Item> {
        self.post_json(&format!("/items/{id}/quantity"), &json!({ "delta": delta }))
            .await
    }

    /// Moves an item to the end of another section, which may belong to another list.
    pub async fn move_item(&self, id: i64, section_id: i64) -> Result<Item> {
        self.post_json(
            &format!("/items/{id}/move"),
            &json!({ "section_id": section_id }),
        )
        .await
    }

    pub async fn move_item_up(&self, id: i64) -> Result<Item> {
        self.post(&format!("/items/{id}/move-up")).await
    }

    pub async fn move_item_down(&self, id: i64) -> Result<Item> {
        self.post(&format!("/items/{id}/move-down")).await
    }

    // History

    /// Up to 100 suggestion entries, most used first.
    pub async fn history(&self) -> Result<Vec<HistoryItem>> {
        Ok(self.get::<HistoryBody>("/history").await?.items)
    }

    /// Records a use of `name` in the suggestion history (upsert ignoring
    /// ASCII case) and returns the stored name.
    pub async fn add_history(&self, name: &str, section_id: Option<i64>) -> Result<String> {
        let body = match section_id {
            Some(section_id) => json!({ "name": name, "section_id": section_id }),
            None => json!({ "name": name }),
        };
        Ok(self
            .post_json::<_, HistoryAddedBody>("/history", &body)
            .await?
            .name)
    }

    pub async fn delete_history(&self, id: i64) -> Result<()> {
        self.delete(&format!("/history/{id}")).await
    }

    /// Deletes several history entries and returns how many were removed.
    /// `ids` must not be empty.
    pub async fn batch_delete_history(&self, ids: &[i64]) -> Result<u64> {
        Ok(self
            .post_json::<_, DeletedBody>("/history/batch-delete", &json!({ "ids": ids }))
            .await?
            .deleted)
    }

    // Batch

    /// Creates a list with sections and items in one transaction. Unlike
    /// [`Client::create_list`] this does not reject duplicate list names, and
    /// unlike [`Client::create_item`] it does not merge same-named items.
    pub async fn batch_create_list(&self, list: &BatchList) -> Result<BatchListCreated> {
        self.post_json("/batch", &json!({ "list": list })).await
    }

    /// Appends sections with their items to a list in one transaction.
    /// `sections` must not be empty.
    pub async fn batch_add_sections(
        &self,
        list_id: i64,
        sections: &[BatchSection],
    ) -> Result<BatchSectionsAdded> {
        self.post_json(
            "/batch",
            &json!({ "list_id": list_id, "sections": sections }),
        )
        .await
    }

    /// Appends items to a section in one transaction, without merging
    /// same-named items. `items` must not be empty.
    pub async fn batch_add_items(
        &self,
        section_id: i64,
        items: &[BatchItem],
    ) -> Result<BatchItemsAdded> {
        self.post_json(
            "/batch",
            &json!({ "section_id": section_id, "items": items }),
        )
        .await
    }
}
