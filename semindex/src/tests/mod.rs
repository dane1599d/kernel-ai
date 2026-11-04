//! Host tests on a fixed corpus: a handful of files a person might keep in
//! `/home/user`, with their change times, and a few that must never be
//! indexed. Each module tests one part; `corpus` builds the index they share.

mod chunk;
mod format;
mod scope;
mod search;
mod units;

use crate::embed::{EmbedError, Embedder, EmbedderId};
use crate::format::{Batch, Header, fresh, open};
use crate::index::{Index, prepare};
use crate::scope::Scope;
use crate::since::DAY_NS;
use crate::{Estimate, Opened};
use alloc::string::String;
use alloc::vec::Vec;

/// The clock in the tests: 2026-11-04T12:00:00Z.
pub const NOW_NS: u64 = 1_793_793_600 * 1_000_000_000;

/// One file of the corpus: its path, its text and how many days ago it changed.
pub struct Doc {
    pub path: &'static str,
    pub text: &'static str,
    pub days_ago: u64,
}

pub const CORPUS: &[Doc] = &[
    Doc {
        path: "/home/user/notes/groceries.txt",
        text: "Groceries for the weekend: milk, bread, eggs, two lemons and coffee beans. Ask Sam about the oat milk.",
        days_ago: 1,
    },
    Doc {
        path: "/home/user/notes/trip.md",
        text: "# Lisbon trip\nFlight TP1351 leaves at 07:40 on the 12th. The hotel is near the river; check in after 15:00. Book the tram tour.",
        days_ago: 3,
    },
    Doc {
        path: "/home/user/work/meeting-2026-10-30.md",
        text: "Meeting notes: the quarterly report is due on Friday. Priya owns the budget section; Tom drafts the hiring plan.",
        days_ago: 5,
    },
    Doc {
        path: "/home/user/work/budget.csv",
        text: "item,amount\nrent,1200\nflights,640\nhotel,380\ncoffee,45\n",
        days_ago: 12,
    },
    Doc {
        path: "/home/user/recipes/lemon-cake.txt",
        text: "Lemon cake: 200 g flour, 150 g sugar, three eggs, the zest of two lemons. Bake for 40 minutes at 180 C.",
        days_ago: 40,
    },
    Doc {
        path: "/home/user/c/hello/hello.c",
        text: "#include <kai.h>\nint main(void) { Print(\"hello from C, built in the OS\\n\"); return 0; }\n",
        days_ago: 2,
    },
    Doc {
        path: "/home/user/notes/reading.md",
        text: "Books to read: The Left Hand of Darkness; Piranesi; A Psalm for the Wild-Built. Café recommendations from Ana: Fábrica, Copenhagen Coffee Lab.",
        days_ago: 20,
    },
    Doc {
        path: "/home/user/diary/2026-11-03.txt",
        text: "A private entry about the weekend. Nobody should find this when the diary folder is excluded: lemons, coffee, hotel.",
        days_ago: 1,
    },
    Doc {
        path: "/home/user/work/report-draft.md",
        text: "Quarterly report draft. Revenue grew; the budget for travel stays flat. See the meeting notes for owners.",
        days_ago: 4,
    },
    Doc {
        path: "/home/user/notes/empty.txt",
        text: "   \n\t  \n",
        days_ago: 0,
    },
];

/// Files that must never be indexed, whatever they hold.
pub const OUTSIDE: &[&str] = &[
    "/system/state/keep",
    "/system/models/blob",
    "/home/username/notes.txt",
    "/home/user/../../system/log",
    "/home/user/./notes.txt",
    "home/user/notes.txt",
    "/home/user",
];

pub fn modified(doc: &Doc) -> u64 {
    NOW_NS - doc.days_ago * DAY_NS
}

/// The corpus's index, built by `prepare` and read back through the file
/// format, as the service would have it after a restart, with `embedder`'s
/// vectors if one is given. The object id of a file is its place in
/// [`CORPUS`] plus 1; each file is a batch of its own, generation 10 plus its
/// place.
pub fn corpus_index(mut embedder: Option<&mut (dyn Embedder + '_)>) -> (Index, Vec<u8>) {
    let header = Header {
        embedder: embedder.as_ref().map(|e| e.id()),
    };
    let mut bytes = fresh(&header).unwrap();
    for (i, doc) in CORPUS.iter().enumerate() {
        let prepared = prepare(
            &Scope::new(),
            doc.path,
            i as u64 + 1,
            10 + i as u64,
            modified(doc),
            doc.text.as_bytes(),
            &Estimate,
            embedder.as_deref_mut(),
        )
        .unwrap();
        let mut batch = Batch::new();
        for record in &prepared.records {
            batch.push(record).unwrap();
        }
        bytes.extend(batch.commit(10 + i as u64));
    }
    let Opened { index, torn, .. } = open(&bytes, &header).unwrap();
    assert!(!torn);
    (index, bytes)
}

/// A stand-in for S13-3's embedder: a vector of four topics, each the count
/// of its words in the text, so texts on one topic point the same way whatever
/// words they use.
pub struct Topics;

const TOPICS: [&[&str]; 4] = [
    &[
        "groceries",
        "milk",
        "bread",
        "eggs",
        "eat",
        "food",
        "cake",
        "flour",
        "sugar",
        "lemon",
        "lemons",
    ],
    &[
        "trip", "flight", "flights", "hotel", "tram", "travel", "holiday", "lisbon",
    ],
    &["budget", "revenue", "rent", "money", "amount", "spend"],
    &["meeting", "report", "quarterly", "hiring", "owners", "work"],
];

impl Embedder for Topics {
    fn id(&self) -> EmbedderId {
        EmbedderId {
            name: String::from("topics-test"),
            sha256: [7; 32],
            dims: 4,
        }
    }

    fn embed_document(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        let words = crate::terms::words(text);
        for (value, topic) in out.iter_mut().zip(TOPICS) {
            *value = words.iter().filter(|w| topic.contains(&w.as_str())).count() as f32;
        }
        Ok(())
    }

    fn embed_query(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        self.embed_document(text, out)
    }
}
