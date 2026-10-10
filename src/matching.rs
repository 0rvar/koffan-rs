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

/// How a name matched a query, from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Exact,
    Prefix,
    Contains,
}

/// Returns the strongest tier any candidate reaches and every candidate in
/// it, in input order: all exact matches of the normalized query, else all
/// candidates starting with it, else all containing it. `None` when nothing
/// matches or the query is blank.
pub fn best_tier<'a, T: ?Sized>(
    query: &str,
    candidates: impl Iterator<Item = (&'a T, &'a str)>,
) -> Option<(Tier, Vec<(&'a T, &'a str)>)> {
    let q = normalize(query);
    if q.is_empty() {
        return None;
    }
    let mut best: Option<(Tier, Vec<(&'a T, &'a str)>)> = None;
    for (value, name) in candidates {
        let n = normalize(name);
        let tier = if n == q {
            Tier::Exact
        } else if n.starts_with(&q) {
            Tier::Prefix
        } else if n.contains(&q) {
            Tier::Contains
        } else {
            continue;
        };
        match &mut best {
            Some((t, hits)) if *t == tier => hits.push((value, name)),
            Some((t, _)) if *t < tier => {}
            _ => best = Some((tier, vec![(value, name)])),
        }
    }
    best
}

/// Picks the candidate whose name matches `query`, comparing normalized names.
///
/// An exact match wins, the first one if several are equal. Otherwise a
/// single candidate starting with the query wins, and failing that a single
/// candidate containing it. Several prefix or substring hits give
/// [`Error::Ambiguous`] listing their names; no hit, or a blank query, gives
/// [`Error::NotFound`]. `what` names the kind of thing in the error, e.g. `"list"`.
pub fn resolve<'a, T>(
    what: &'static str,
    query: &str,
    candidates: impl Iterator<Item = (&'a T, &'a str)>,
) -> Result<&'a T> {
    match best_tier(query, candidates) {
        None => Err(Error::NotFound {
            what,
            name: query.trim().to_string(),
        }),
        Some((Tier::Exact, hits)) => Ok(hits[0].0),
        Some((_, hits)) if hits.len() == 1 => Ok(hits[0].0),
        Some((_, hits)) => Err(Error::Ambiguous {
            what,
            name: query.trim().to_string(),
            candidates: hits.iter().map(|(_, name)| name.to_string()).collect(),
        }),
    }
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
        assert_eq!(
            pick("frukt", &["Bröd", "Frukt & grönt"]).unwrap(),
            &"Frukt & grönt"
        );
    }

    #[test]
    fn unique_substring_matches_when_no_prefix_does() {
        assert_eq!(
            pick("grönt", &["Bröd", "Frukt & grönt"]).unwrap(),
            &"Frukt & grönt"
        );
    }

    #[test]
    fn prefix_tier_wins_over_substring_tier() {
        assert_eq!(
            pick("ost", &["Ostbågar", "Kvarg och ost"]).unwrap(),
            &"Ostbågar"
        );
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
        assert!(matches!(
            pick("  ", &["Mjölk"]).unwrap_err(),
            Error::NotFound { .. }
        ));
    }

    #[test]
    fn best_tier_returns_every_hit_of_the_strongest_tier() {
        let names = ["Smörgåsgurka", "Smör", "smör", "Jordnötssmör"];
        let (tier, hits) = best_tier("SMÖR", names.iter().map(|n| (n, *n))).unwrap();
        assert_eq!(tier, Tier::Exact);
        assert_eq!(
            hits.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
            vec!["Smör", "smör"]
        );
        let (tier, hits) = best_tier("smörg", names.iter().map(|n| (n, *n))).unwrap();
        assert_eq!((tier, hits.len()), (Tier::Prefix, 1));
        assert!(best_tier(" ", names.iter().map(|n| (n, *n))).is_none());
    }
}
