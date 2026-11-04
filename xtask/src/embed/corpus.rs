//! Spike S13-3's fixed corpus (ADR 0013): 200 generated notes and 50 queries
//! with known answers. Each of 25 families writes one template over four
//! subjects and two details, so every answer has seven close neighbours that
//! share its words: the queries paraphrase, never quote. In the last five
//! families the two details are two months, the texts are the same, and the
//! query carries the month as a time filter, as `search` resolves "in March"
//! from the clock rather than the model.

/// A note: its text and the day it was written, as days since 2026-01-01.
pub struct Doc {
    pub text: String,
    pub day: u32,
}

pub struct Query {
    pub text: &'static str,
    /// The note that answers it, an index into [`Corpus::docs`].
    pub answer: usize,
    /// Days since 2026-01-01, inclusive: only notes written then count.
    pub window: Option<(u32, u32)>,
}

pub struct Corpus {
    pub docs: Vec<Doc>,
    pub queries: Vec<Query>,
}

/// One template over four subjects (`{a}`) and two details (`{b}`), and two
/// queries, each naming the subject and detail it asks for.
struct Family {
    template: &'static str,
    a: [&'static str; 4],
    b: [&'static str; 2],
    queries: [(&'static str, usize, usize); 2],
    /// For a time family: the month (1-12) each detail stands for.
    months: Option<[u32; 2]>,
}

const fn family(
    template: &'static str,
    a: [&'static str; 4],
    b: [&'static str; 2],
    queries: [(&'static str, usize, usize); 2],
) -> Family {
    Family {
        template,
        a,
        b,
        queries,
        months: None,
    }
}

const fn timed(
    template: &'static str,
    a: [&'static str; 4],
    months: [u32; 2],
    queries: [(&'static str, usize, usize); 2],
) -> Family {
    Family {
        template,
        a,
        b: ["", ""],
        queries,
        months: Some(months),
    }
}

const FAMILIES: [Family; 25] = [
    family(
        "Reminder: {a} has a {b} at the Northgate dental surgery; arrive ten minutes early and bring the referral letter.",
        ["Sam", "Priya", "Tom", "Elena"],
        ["tooth filling", "routine check-up"],
        [
            ("When is Priya getting a cavity filled?", 1, 0),
            ("Tom's six-monthly dental exam", 2, 1),
        ],
    ),
    family(
        "Recipe card from Grandma's notebook: {a} {b}, serves four.",
        ["sourdough", "rye", "focaccia", "brioche"],
        [
            "loaf with a cold overnight proof in the fridge",
            "rolls shaped by hand and baked in a hot oven",
        ],
        [
            (
                "how to make rye bread that rises slowly in the refrigerator",
                1,
                0,
            ),
            ("hand-shaped brioche buns", 3, 1),
        ],
    ),
    family(
        "Invoice from {a} for {b}; payment is due within thirty days.",
        [
            "Harper Plumbing",
            "Brightline Electrical",
            "Oakwood Joinery",
            "Clearview Glazing",
        ],
        [
            "an emergency call-out on a Sunday",
            "the annual safety inspection",
        ],
        [
            (
                "the electrician's bill for the weekend emergency visit",
                1,
                0,
            ),
            (
                "what the carpenters charged for the yearly inspection",
                2,
                1,
            ),
        ],
    ),
    family(
        "Travel booking: {b} to {a}, window seat, one checked bag.",
        ["Lisbon", "Oslo", "Kraków", "Dublin"],
        [
            "outbound flight on Friday morning",
            "return train on Sunday evening",
        ],
        [
            ("my flight out to Norway", 1, 0),
            ("coming back from Poland by rail", 2, 1),
        ],
    ),
    family(
        "Incident report: {a} {b} for forty minutes after the certificate rotation.",
        [
            "the billing API",
            "the login service",
            "the search cluster",
            "the mail relay",
        ],
        ["returned errors to every caller", "ran very slowly"],
        [
            ("the outage where nobody could sign in", 1, 0),
            ("search was sluggish after the certs changed", 2, 1),
        ],
    ),
    family(
        "Garden log: planted {a} in the {b} bed and watered them twice.",
        ["tomatoes", "courgettes", "runner beans", "lettuce"],
        ["sunny raised", "shady back"],
        [
            (
                "where did I put the zucchini in the bright raised bed",
                1,
                0,
            ),
            ("lettuce in the shaded part of the garden", 3, 1),
        ],
    ),
    family(
        "Piano lesson notes for {a}: practise {b} slowly with the metronome.",
        ["Mia", "Leo", "Ava", "Noah"],
        [
            "the C major scale in contrary motion",
            "the left hand of the Bach minuet",
        ],
        [
            ("which scale exercise Leo was given", 1, 0),
            ("Noah's homework on the Bach piece", 3, 1),
        ],
    ),
    family(
        "Tax note: {a} {b} must reach HMRC before the end of January.",
        [
            "the self-assessment return",
            "the rental income summary",
            "the pension contribution claim",
            "the gift aid form",
        ],
        ["for the flat in Leeds", "for the house in York"],
        [
            ("rent income paperwork for the York property", 1, 1),
            ("self assessment for the Leeds apartment", 0, 0),
        ],
    ),
    family(
        "Book club: we discussed {a}, and {b}.",
        [
            "The Remains of the Day",
            "Piranesi",
            "Middlemarch",
            "Klara and the Sun",
        ],
        [
            "most people loved the ending",
            "nobody had finished it in time",
        ],
        [
            ("the long Victorian novel nobody managed to finish", 2, 1),
            (
                "the book about the robot friend whose ending everyone liked",
                3,
                0,
            ),
        ],
    ),
    family(
        "Training plan for {a}: {b}.",
        ["Monday", "Tuesday", "Thursday", "Saturday"],
        [
            "a 5 km tempo run and stretching",
            "heavy squats and deadlifts in the gym",
        ],
        [
            ("what running I do on Thursday", 2, 0),
            ("Saturday's weightlifting session", 3, 1),
        ],
    ),
    family(
        "Meeting minutes: the {a} team agreed to {b} by the end of the quarter.",
        ["design", "finance", "support", "data"],
        ["hire two more people", "cut the travel budget"],
        [
            ("finance decided to spend less on trips", 1, 1),
            ("support plans to recruit", 2, 0),
        ],
    ),
    family(
        "Pet care: {a} needs {b} at the vet on Elm Street.",
        ["the cat", "the dog", "the rabbit", "the parrot"],
        ["a booster vaccination", "a claw trim"],
        [
            ("the rabbit's jab", 2, 0),
            ("getting the parrot's nails clipped", 3, 1),
        ],
    ),
    family(
        "Dinner idea: {a} curry with {b}, simmered for twenty minutes.",
        ["chickpea", "chicken", "lentil", "prawn"],
        ["coconut milk and lime leaves", "tomatoes and garam masala"],
        [
            ("dal made with coconut", 2, 0),
            ("seafood curry in a tomato sauce", 3, 1),
        ],
    ),
    family(
        "Home repair: the {a} {b}; call the landlord on Monday.",
        [
            "kitchen tap",
            "bathroom extractor fan",
            "bedroom radiator",
            "hallway light",
        ],
        ["drips all the time", "makes a loud rattling noise"],
        [
            ("the noisy heater in the bedroom", 2, 1),
            ("leaking faucet in the kitchen", 0, 0),
        ],
    ),
    family(
        "Birthday: {a} turns {b} on Saturday; the party is at the bowling alley.",
        ["Jamie", "Rosa", "Ollie", "Grace"],
        ["seven", "forty"],
        [
            ("when is Rosa's fortieth", 1, 1),
            ("Ollie's seventh birthday party", 2, 0),
        ],
    ),
    family(
        "Software note: in the {a} repository, {b}.",
        ["kernel", "website", "mobile app", "data pipeline"],
        [
            "the tests fail on Windows because of path separators",
            "a release build needs the signing key from the vault",
        ],
        [
            ("why the phone app's tests break on Windows", 2, 0),
            ("the signing key needed to ship the pipeline", 3, 1),
        ],
    ),
    family(
        "Shopping list for {a}: {b}.",
        [
            "the camping trip",
            "the barbecue",
            "the school lunches",
            "the dinner party",
        ],
        [
            "firelighters, matches and a gas canister",
            "bread rolls, fruit and juice boxes",
        ],
        [
            ("fuel and things to light the grill for the barbecue", 1, 0),
            ("snacks and drinks for the kids' packed lunches", 2, 1),
        ],
    ),
    family(
        "Letter from the surgery: {a}'s blood test shows {b}; recheck in three months.",
        ["Dad", "Aunt Mary", "Grandpa", "Uncle Raj"],
        ["slightly high cholesterol", "low vitamin D"],
        [
            ("Mary needs more of the sunshine vitamin", 1, 1),
            ("Raj's cholesterol result", 3, 0),
        ],
    ),
    family(
        "Bank: set up a standing order of {b} to {a} every month.",
        [
            "the savings account",
            "the mortgage overpayment",
            "the gym",
            "the animal charity",
        ],
        ["£50", "£200"],
        [
            (
                "two hundred pounds a month to pay the mortgage down early",
                1,
                1,
            ),
            ("fifty pounds monthly for the gym membership", 2, 0),
        ],
    ),
    family(
        "Language practice: {a} words and phrases for {b}.",
        ["Spanish", "Japanese", "German", "Italian"],
        [
            "ordering food in a restaurant",
            "asking the way at a railway station",
        ],
        [
            (
                "how to ask for directions at a train station in German",
                2,
                1,
            ),
            ("Japanese for ordering a meal", 1, 0),
        ],
    ),
    timed(
        "Team retrospective: {a} raised the flaky deployment pipeline again.",
        ["Elena", "Marcus", "Wei", "Fatima"],
        [3, 9],
        [
            ("what Marcus brought up at the retro", 1, 1),
            ("Fatima's point in the retrospective", 3, 0),
        ],
    ),
    timed(
        "Electricity meter reading sent by {a}: usage is up on last quarter.",
        ["the landlord", "me", "the smart meter", "the engineer"],
        [2, 8],
        [
            ("the meter reading the engineer took", 3, 1),
            ("the landlord's electricity reading", 0, 0),
        ],
    ),
    timed(
        "Haircut booked with {a} at the salon on the high street.",
        ["Jess", "Kai", "Lou", "Max"],
        [4, 10],
        [
            ("my appointment with Kai the hairdresser", 1, 0),
            ("hair appointment with Lou", 2, 1),
        ],
    ),
    timed(
        "Parents' evening with {a}'s teacher: reading is coming on well.",
        ["Mia", "Leo", "Ava", "Noah"],
        [1, 6],
        [
            ("what Ava's teacher said about her reading", 2, 1),
            ("Leo's parents' evening", 1, 0),
        ],
    ),
    timed(
        "Weekly call with {a} about the house move.",
        [
            "the solicitor",
            "the estate agent",
            "the removal firm",
            "the mortgage adviser",
        ],
        [5, 11],
        [
            ("talking to the movers about moving house", 2, 1),
            ("the lawyer call about the move", 0, 0),
        ],
    ),
];

/// Days before each month of 2026.
const MONTH_START: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const MONTH_DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// The corpus, the same every time.
pub fn build() -> Corpus {
    let mut docs = Vec::new();
    let mut queries = Vec::new();
    for (f, family) in FAMILIES.iter().enumerate() {
        let first = docs.len();
        for (i, a) in family.a.iter().enumerate() {
            for (j, b) in family.b.iter().enumerate() {
                let text = family.template.replace("{a}", a).replace("{b}", b);
                let day = match family.months {
                    Some(months) => MONTH_START[months[j] as usize - 1] + 3 + 5 * i as u32,
                    None => (f as u32 * 37 + (2 * i + j) as u32 * 11) % 300,
                };
                docs.push(Doc { text, day });
            }
        }
        for &(text, a, b) in &family.queries {
            let window = family.months.map(|months| {
                let m = months[b] as usize - 1;
                (MONTH_START[m], MONTH_START[m] + MONTH_DAYS[m] - 1)
            });
            queries.push(Query {
                text,
                answer: first + 2 * a + b,
                window,
            });
        }
    }
    Corpus { docs, queries }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_hundred_notes_and_fifty_queries_ten_with_a_time_filter() {
        let corpus = build();
        assert_eq!(corpus.docs.len(), 200);
        assert_eq!(corpus.queries.len(), 50);
        let timed: Vec<&Query> = corpus
            .queries
            .iter()
            .filter(|q| q.window.is_some())
            .collect();
        assert_eq!(timed.len(), 10);
        // A timed answer's twin has the same text and falls outside the window.
        for q in timed {
            let (since, until) = q.window.unwrap();
            let answer = &corpus.docs[q.answer];
            assert!((since..=until).contains(&answer.day));
            let twin = q.answer ^ 1;
            assert_eq!(corpus.docs[twin].text, answer.text);
            assert!(!(since..=until).contains(&corpus.docs[twin].day));
        }
        // Every note is distinct, apart from the timed twins.
        let mut texts: Vec<&str> = corpus.docs.iter().map(|d| d.text.as_str()).collect();
        texts.sort_unstable();
        texts.dedup();
        assert_eq!(texts.len(), 160 + 20);
    }

    #[test]
    fn answers_name_their_subject() {
        let corpus = build();
        let piano = &corpus.queries[13];
        assert!(corpus.docs[piano.answer].text.contains("Noah"));
        assert!(corpus.docs[piano.answer].text.contains("Bach"));
    }
}
