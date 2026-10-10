use crate::client::TIMEOUT;

/// Errors returned by [`Client`](crate::Client) and the name-matching helpers.
///
/// The `Display` text of every variant is meant to be usable as-is in a reply
/// to a person or a model.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The request could not be sent, timed out, or its body could not be read.
    #[error("{}", http_message(.0))]
    Http(#[from] reqwest::Error),

    /// The server answered 2xx with a body that is not the expected JSON,
    /// which usually means the base URL points at something other than Koffan.
    #[error("unexpected response from {url}; is this a Koffan API URL?")]
    UnexpectedResponse {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    /// The server answered with a non-2xx status. `code` is the machine-readable
    /// `error` field of Koffan's error body (for example `not_found`,
    /// `list_name_exists`, `invalid_token`), or the HTTP reason phrase when the
    /// body was not a Koffan error.
    #[error("server returned {status} {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },

    /// The base URL given to [`Client::new`](crate::Client::new) is not a
    /// usable http(s) URL.
    #[error("invalid base URL: {0}")]
    InvalidUrl(String),

    /// A request was refused before sending because the server would
    /// misinterpret it.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// No candidate matched a name.
    #[error("no {what} matches \"{name}\"")]
    NotFound { what: &'static str, name: String },

    /// A name matched several candidates and none could be preferred.
    #[error("\"{name}\" matches several {what}s: {}; use a more specific name", .candidates.join(", "))]
    Ambiguous {
        what: &'static str,
        name: String,
        candidates: Vec<String>,
    },

    /// No list was named, no default list is configured, and the server has
    /// zero or several lists. `lists` holds the names that exist.
    #[error("no list was named and there is no default list; name one of: {}", .lists.join(", "))]
    NoDefaultList { lists: Vec<String> },
}

fn http_message(e: &reqwest::Error) -> String {
    let url = e.url().map(|u| u.as_str()).unwrap_or("the server");
    if e.is_timeout() {
        format!("Koffan did not answer within {} s", TIMEOUT.as_secs())
    } else if e.is_connect() {
        let host = e.url().and_then(|u| u.host_str()).unwrap_or("the server");
        format!("could not connect to {host}")
    } else if e.is_decode() {
        format!("unexpected response from {url}; is this a Koffan API URL?")
    } else {
        format!("request to {url} failed: {e}")
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
