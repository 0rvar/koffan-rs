/// Errors returned by [`Client`](crate::Client) and the name-matching helpers.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The request could not be sent, timed out, or the response body could not be decoded.
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),

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

    /// The base URL given to [`Client::new`](crate::Client::new) is not an http(s) URL.
    #[error("invalid base URL: {0}")]
    InvalidUrl(String),

    /// No candidate matched a name.
    #[error("no {what} matches \"{name}\"")]
    NotFound { what: &'static str, name: String },

    /// A name matched several candidates by prefix or substring.
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

pub type Result<T, E = Error> = std::result::Result<T, E>;
