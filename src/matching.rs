//! Resolving spoken or typed names to lists, sections and items.

use crate::error::{Error, Result};

/// Normalizes a name for comparison: trims, lowercases (Unicode-aware) and
/// collapses runs of whitespace into one space.
pub fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Picks the candidate whose name matches `query`, comparing normalized names.
///
/// An exact match wins (the first one if several are equal). Otherwise a
/// single candidate starting with the query wins, and failing that a single
/// candidate containing it. Several prefix or substring hits give
/// [`Error::Ambiguous`] listing their names; no hit, or an empty query, gives
/// [`Error::NotFound`]. `what` names the kind of thing in the error, e.g. `"list"`.
pub fn resolve<'a, T>(
    what: &'static str,
    query: &str,
    candidates: impl Iterator<Item = (&'a T, &'a str)>,
) -> Result<&'a T> {
    let q = normalize(query);
    let not_found = || Error::NotFound {
        what,
        name: query.trim().to_string(),
    };
    if q.is_empty() {
        return Err(not_found());
    }
    let candidates: Vec<(&'a T, &'a str, String)> = candidates
        .map(|(value, name)| (value, name, normalize(name)))
        .collect();

    if let Some((value, _, _)) = candidates.iter().find(|(_, _, n)| *n == q) {
        return Ok(value);
    }
    let tiers: [&dyn Fn(&str) -> bool; 2] = [&|n| n.starts_with(&q), &|n| n.contains(&q)];
    for matches in tiers {
        let hits: Vec<_> = candidates.iter().filter(|(_, _, n)| matches(n)).collect();
        match hits.as_slice() {
            [] => continue,
            [(value, _, _)] => return Ok(value),
            many => {
                return Err(Error::Ambiguous {
                    what,
                    name: query.trim().to_string(),
                    candidates: many.iter().map(|(_, name, _)| name.to_string()).collect(),
                })
            }
        }
    }
    Err(not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick<'a>(query: &str, names: &'a [&'a str]) -> Result<&'a &'a str> {
        resolve("item", query, names.iter().map(|n| (n, *n)))
    }

    #[test]
    fn normalize_trims_lowercases_and_collapses_whitespace() {
        assert_eq!(normalize("  Färsk   MJÖLK\t1,5% "), "färsk mjölk 1,5%");
        assert_eq!(normalize("ÅÄÖ"), "åäö");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn exact_match_beats_prefix() {
        assert_eq!(pick("mjölk", &["Mjölkchoklad", "mjölk"]).unwrap(), &"mjölk");
    }

    #[test]
    fn first_of_several_exact_matches_wins() {
        let names = ["Ost", "ost"];
        let got = pick(" OST ", &names).unwrap();
        assert!(std::ptr::eq(got, &names[0]));
    }

    #[test]
    fn unique_prefix_matches() {
        assert_eq!(pick("frukt", &["Bröd", "Frukt & grönt"]).unwrap(), &"Frukt & grönt");
    }

    #[test]
    fn unique_substring_matches_when_no_prefix_does() {
        assert_eq!(pick("grönt", &["Bröd", "Frukt & grönt"]).unwrap(), &"Frukt & grönt");
    }

    #[test]
    fn prefix_tier_wins_over_substring_tier() {
        assert_eq!(pick("ost", &["Ostbågar", "Kvarg och ost"]).unwrap(), &"Ostbågar");
    }

    #[test]
    fn exact_match_resolves_despite_other_prefix_hits() {
        assert_eq!(pick("mjöl", &["Mjölk", "Mjöl", "Bröd"]).unwrap(), &"Mjöl");
    }

    #[test]
    fn several_prefix_hits_are_ambiguous_with_candidate_names() {
        match pick("mj", &["Mjölk", "Mjöl", "Bröd"]).unwrap_err() {
            Error::Ambiguous {
                what,
                name,
                candidates,
            } => {
                assert_eq!(what, "item");
                assert_eq!(name, "mj");
                assert_eq!(candidates, vec!["Mjölk", "Mjöl"]);
            }
            e => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn no_match_is_not_found() {
        assert!(matches!(
            pick("smör", &["Mjölk"]).unwrap_err(),
            Error::NotFound { what: "item", ref name } if name == "smör"
        ));
    }

    #[test]
    fn empty_query_is_not_found() {
        assert!(matches!(pick("  ", &["Mjölk"]).unwrap_err(), Error::NotFound { .. }));
    }
}
