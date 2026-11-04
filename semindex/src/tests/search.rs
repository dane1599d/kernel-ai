//! Search over the corpus: keywords, time and folder filters, excluded folders
//! honoured at query time, and the vector slot merged in when the index and
//! the embedder agree.

use super::{NOW_NS, Topics, corpus_index};
use crate::embed::{Embedder, EmbedderId};
use crate::scope::{Scope, Skip};
use crate::search::{MAX_HITS, Query, search};
use crate::since;
use alloc::string::String;
use alloc::vec::Vec;

fn paths(hits: &[crate::Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.path.as_str()).collect()
}

fn ask(text: &str) -> Query<'_> {
    Query {
        text,
        ..Query::default()
    }
}

#[test]
fn a_keyword_finds_the_files_that_hold_it_best_first() {
    let (index, _) = corpus_index(None);
    let scope = Scope::new();
    let hits = search(&index, &scope, &ask("lisbon hotel"), None).unwrap();
    assert_eq!(hits[0].path, "/home/user/notes/trip.md");
    // The diary and the budget say "hotel" too, but not "lisbon".
    let found = paths(&hits);
    assert!(found.contains(&"/home/user/work/budget.csv"));
    assert!(found.contains(&"/home/user/diary/2026-11-03.txt"));
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(hits.iter().all(|h| (0.0..=1.0).contains(&h.score)));
    // Case and punctuation do not matter; a word no file holds finds nothing.
    let upper = search(&index, &scope, &ask("LISBON, Hotel!"), None).unwrap();
    assert_eq!(paths(&upper), paths(&hits));
    assert!(
        search(&index, &scope, &ask("zeppelin"), None)
            .unwrap()
            .is_empty()
    );
    assert!(search(&index, &scope, &ask(""), None).unwrap().is_empty());
}

#[test]
fn a_hit_is_its_files_best_chunk_and_its_place_is_the_snippet() {
    let (index, _) = corpus_index(None);
    let hits = search(&index, &Scope::new(), &ask("quarterly report"), None).unwrap();
    // The two files that hold both words come first, in either order.
    let mut first: Vec<&str> = paths(&hits)[..2].to_vec();
    first.sort_unstable();
    assert_eq!(
        first,
        [
            "/home/user/work/meeting-2026-10-30.md",
            "/home/user/work/report-draft.md"
        ]
    );
    let doc = super::CORPUS
        .iter()
        .find(|d| d.path == hits[0].path)
        .unwrap();
    let snippet = &doc.text[hits[0].offset as usize..][..hits[0].len as usize];
    assert!(
        snippet.to_lowercase().contains("quarterly report"),
        "{snippet}"
    );
}

#[test]
fn a_time_filter_keeps_only_files_changed_since_it() {
    let (index, _) = corpus_index(None);
    let scope = Scope::new();
    let all = search(&index, &scope, &ask("lemons"), None).unwrap();
    assert!(paths(&all).contains(&"/home/user/recipes/lemon-cake.txt"));
    let since = since::resolve("last week", NOW_NS);
    let query = Query {
        since_ns: since,
        ..ask("lemons")
    };
    let recent = search(&index, &scope, &query, None).unwrap();
    // The cake is 40 days old; the groceries list and the diary are new.
    let found = paths(&recent);
    assert!(
        found.contains(&"/home/user/notes/groceries.txt"),
        "{found:?}"
    );
    assert!(
        found.contains(&"/home/user/diary/2026-11-03.txt"),
        "{found:?}"
    );
    assert!(!found.contains(&"/home/user/recipes/lemon-cake.txt"));
    assert!(recent.iter().all(|h| h.modified_ns >= since.unwrap()));
}

#[test]
fn a_folder_filter_keeps_only_files_under_it_and_must_be_under_home() {
    let (index, _) = corpus_index(None);
    let scope = Scope::new();
    let query = Query {
        folder: Some("/home/user/work"),
        ..ask("budget")
    };
    let hits = search(&index, &scope, &query, None).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| h.path.starts_with("/home/user/work/")));
    for (folder, why) in [
        ("/system", Skip::Outside),
        ("/home/user/../system", Skip::Unclean),
        ("work", Skip::Unclean),
    ] {
        let query = Query {
            folder: Some(folder),
            ..ask("budget")
        };
        assert_eq!(search(&index, &scope, &query, None), Err(why), "{folder}");
    }
}

#[test]
fn a_folder_excluded_after_it_was_indexed_is_never_a_result() {
    let (index, _) = corpus_index(None);
    let mut scope = Scope::new();
    let before = search(&index, &scope, &ask("weekend"), None).unwrap();
    assert!(paths(&before).contains(&"/home/user/diary/2026-11-03.txt"));
    scope.exclude("/home/user/diary").unwrap();
    let after = search(&index, &scope, &ask("weekend"), None).unwrap();
    assert!(
        !paths(&after)
            .iter()
            .any(|p| p.starts_with("/home/user/diary/"))
    );
    assert!(paths(&after).contains(&"/home/user/notes/groceries.txt"));
}

#[test]
fn the_limit_caps_the_results_and_ties_go_by_path() {
    let (index, _) = corpus_index(None);
    let scope = Scope::new();
    let query = Query {
        limit: 2,
        ..ask("coffee lemons hotel budget")
    };
    assert_eq!(search(&index, &scope, &query, None).unwrap().len(), 2);
    let all = search(&index, &scope, &ask("coffee lemons hotel budget"), None).unwrap();
    assert!(all.len() <= MAX_HITS);
    let again = search(&index, &scope, &ask("coffee lemons hotel budget"), None).unwrap();
    assert_eq!(all, again, "the same query gives the same order");
}

#[test]
fn with_vectors_a_query_finds_files_by_meaning_as_well_as_by_word() {
    let (index, _) = corpus_index(Some(&Topics));
    let scope = Scope::new();
    // No file holds "eat" or "food", but the groceries and the cake are about
    // food, and the stand-in embedder says so.
    let keyword = search(&index, &scope, &ask("what can I eat"), None).unwrap();
    assert!(keyword.is_empty());
    let meant = search(&index, &scope, &ask("what can I eat"), Some(&Topics)).unwrap();
    let found = paths(&meant);
    assert!(
        found.contains(&"/home/user/notes/groceries.txt"),
        "{found:?}"
    );
    assert!(
        found.contains(&"/home/user/recipes/lemon-cake.txt"),
        "{found:?}"
    );
    assert!(!found.contains(&"/home/user/work/meeting-2026-10-30.md"));
}

#[test]
fn an_embedder_that_did_not_make_the_index_is_not_used_and_search_falls_back() {
    let (index, _) = corpus_index(Some(&Topics));
    struct Other;
    impl Embedder for Other {
        fn id(&self) -> EmbedderId {
            EmbedderId {
                name: String::from("other"),
                sha256: [9; 32],
                dims: 4,
            }
        }
        fn document(&self, _: &str) -> Result<alloc::vec::Vec<f32>, String> {
            Ok(alloc::vec![1.0, 0.0, 0.0, 0.0])
        }
        fn query(&self, _: &str) -> Result<alloc::vec::Vec<f32>, String> {
            Ok(alloc::vec![1.0, 0.0, 0.0, 0.0])
        }
    }
    let scope = Scope::new();
    let hits = search(&index, &scope, &ask("what can I eat"), Some(&Other)).unwrap();
    assert!(
        hits.is_empty(),
        "no keyword, and Other's vectors are not used"
    );
    let keyword = search(&index, &scope, &ask("lisbon"), Some(&Other)).unwrap();
    assert_eq!(paths(&keyword), ["/home/user/notes/trip.md"]);
}
