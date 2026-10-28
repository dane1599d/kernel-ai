use crate::appearance::{Appearance, Refused, Shipped, Themes};
use crate::theme::{self, Theme};
use crate::{Curve, Family, HIGH_CONTRAST, LIGHT, Motion, Token, TypeStyle};
use std::format;
use std::path::PathBuf;
use std::string::String;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../assets/themes")
        .join(format!("{name}.kai-theme"))
}

/// The shipped themes are whole `kai-theme 1` files in assets/themes, which
/// read back to the same themes and pass every check. Run with `KAI_BLESS=1`
/// to rewrite them, and review the change.
#[test]
fn the_shipped_themes_are_their_files() {
    for theme in theme::shipped() {
        let text = theme.encode();
        assert_eq!(Theme::parse(&text), Ok(theme.clone()), "{}", theme.name);
        theme.check().unwrap();
        let path = golden(&theme.name);
        if std::env::var_os("KAI_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        }
        let file = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(
            file, text,
            "{} is not its file; KAI_BLESS=1 rewrites it",
            theme.name
        );
    }
}

#[test]
fn the_legible_theme_is_light_in_atkinson_one_px_larger() {
    let legible = Shipped.theme("legible").unwrap();
    assert_eq!(legible.base.name, "light");
    assert_eq!(legible.colour(Token::Text), LIGHT.text);
    let body = legible.style(TypeStyle::Body);
    assert_eq!(
        (body.family, body.size, body.line),
        (Family::LegibleSans, 15.0, 21.0)
    );
    assert_eq!(legible.style(TypeStyle::Mono).family, Family::LegibleMono);
}

#[test]
fn a_file_changes_only_what_it_names() {
    let text = "kai-theme 1\nname = mine\nbase = high-contrast\n; a comment\n\naccent = #FFFF00\n\
                body = inter 16/22 500\nsheet = 200 bezier 0.2 0 0 1\n";
    let theme = Theme::parse(text).unwrap();
    assert_eq!(theme.name, "mine");
    assert!(theme.dark());
    assert_eq!(theme.colour(Token::Accent), 0xFF_FF00);
    assert_eq!(theme.colour(Token::Text), HIGH_CONTRAST.text);
    assert_eq!(theme.style(TypeStyle::Body).size, 16.0);
    assert_eq!(theme.style(TypeStyle::Small).size, 12.0);
    assert_eq!(
        theme.motion(Motion::Sheet, false),
        (200, Curve::Bezier(0.2, 0.0, 0.0, 1.0))
    );
    assert_eq!(theme.motion(Motion::Sheet, true), (0, Curve::None));
    // Its whole file reads back the same.
    assert_eq!(Theme::parse(&theme.encode()), Ok(theme));
}

/// Each bad file is refused at its line, with a reason.
#[test]
fn bad_files_are_refused_at_their_line() {
    let head = "kai-theme 1\nname = t\nbase = dark\n";
    let cases: [(String, usize, &str); 14] = [
        (
            String::from("kai-theme 2\nname = t\nbase = dark\n"),
            1,
            "first line",
        ),
        (
            String::from("kai-theme 1\nbase = dark\nname = t\n"),
            2,
            "come first",
        ),
        (
            String::from("kai-theme 1\nname = T\nbase = dark\n"),
            2,
            "bad name",
        ),
        (
            String::from("kai-theme 1\nname = t\nbase = sepia\n"),
            3,
            "no such base",
        ),
        (format!("{head}text = #FFF\n"), 4, "#RRGGBB"),
        (format!("{head}text=#FFFFFF\n"), 4, "key = value"),
        (format!("{head}glow = #FFFFFF\n"), 4, "no such token"),
        (
            format!("{head}text = #ECE6DA\ntext = #ECE6DA\n"),
            5,
            "twice",
        ),
        (
            format!("{head}body = comic 14/20 400\n"),
            4,
            "family size/line",
        ),
        (
            format!("{head}body = inter 14/10 400\n"),
            4,
            "family size/line",
        ),
        (format!("{head}sheet = 900 linear\n"), 4, "ms curve"),
        (format!("{head}sheet = 100 bezier 2 0 0 1\n"), 4, "ms curve"),
        (format!("{head}name = u\n"), 4, "come first"),
        (String::from("kai-theme 1\n"), 0, "no `name`"),
    ];
    for (text, line, why) in cases {
        let err = Theme::parse(&text).unwrap_err();
        assert_eq!(err.line, line, "{text:?}: {err:?}");
        assert!(err.why.contains(why), "{text:?}: {err:?}");
    }
}

/// A file that reads but could not be seen, or could fake the AI, is refused.
#[test]
fn contrast_and_the_ais_colour_are_held() {
    let head = "kai-theme 1\nname = t\nbase = dark\n";
    let low = Theme::parse(&format!("{head}muted = #30343C\n")).unwrap_err();
    assert!(low.why.starts_with("muted on window is "), "{low:?}");
    let mark = Theme::parse(&format!("{head}line_strong = #262B35\n")).unwrap_err();
    assert!(mark.why.contains("under 3:1"), "{mark:?}");
    let fake = Theme::parse(&format!("{head}accent = #FF9F5A\n")).unwrap_err();
    assert!(fake.why.contains("ai's colour"), "{fake:?}");
    // `line` is decorative: any colour passes.
    assert!(Theme::parse(&format!("{head}line = #141821\n")).is_ok());
}

#[test]
fn appearance_changes_leave_the_old_one_for_undo() {
    let before = Appearance::default();
    let after = before.set("theme", "light", &Shipped).unwrap();
    assert_eq!(before.theme, "dark");
    assert_eq!(after.theme, "light");
    assert_eq!(after.change(&before), "theme light (was dark)");
    let sized = after.set("text_size", "150%", &Shipped).unwrap();
    let theme = sized.resolve(&Shipped).unwrap();
    assert_eq!(theme.style(TypeStyle::Body).size, 21.0);
    let legible = sized.set("legible", "on", &Shipped).unwrap();
    let theme = legible.resolve(&Shipped).unwrap();
    assert_eq!(theme.style(TypeStyle::Body).family, Family::LegibleSans);
    assert_eq!(theme.style(TypeStyle::Body).size, 22.5);
    assert_eq!(legible.change(&sized), "legible type on (was off)");
    let moved = legible.set("reduced_motion", "on", &Shipped).unwrap();
    let scaled = moved.set("scale", "125", &Shipped).unwrap();
    assert_eq!(scaled.change(&before).matches(" (was ").count(), 5);
}

#[test]
fn a_token_over_the_theme_is_checked_and_a_new_theme_drops_it() {
    let dark = Appearance::default();
    let accent = dark.set("token", "accent #A0C8FF", &Shipped).unwrap();
    assert_eq!(
        accent.resolve(&Shipped).unwrap().colour(Token::Accent),
        0xA0_C8FF
    );
    let refused = |r: Result<Appearance, Refused>| r.unwrap_err().0;
    assert!(refused(dark.set("token", "text #202020", &Shipped)).starts_with("text on"));
    assert!(refused(dark.set("token", "accent #FF9F5A", &Shipped)).contains("ai's colour"));
    assert!(refused(dark.set("token", "glow #FFFFFF", &Shipped)).contains("no token"));
    let dropped = accent.set("token", "accent", &Shipped).unwrap();
    assert_eq!(dropped, dark);
    let light = accent.set("theme", "light", &Shipped).unwrap();
    assert!(light.tokens.is_empty());
}

#[test]
fn settings_out_of_range_are_refused() {
    let a = Appearance::default();
    for (setting, value) in [
        ("theme", "sepia"),
        ("scale", "110"),
        ("scale", "325%"),
        ("text_size", "95"),
        ("text_size", "155"),
        ("legible", "yes"),
        ("contrast", "high"),
    ] {
        assert!(
            a.set(setting, value, &Shipped).is_err(),
            "{setting} {value}"
        );
    }
}

#[test]
fn saved_appearance_reads_back_and_refuses_what_is_gone() {
    let saved = Appearance::default()
        .set("theme", "high-contrast", &Shipped)
        .and_then(|a| a.set("token", "accent #FFFF00", &Shipped))
        .and_then(|a| a.set("scale", "200%", &Shipped))
        .and_then(|a| a.set("legible", "on", &Shipped))
        .unwrap();
    let text = saved.encode();
    assert_eq!(Appearance::decode(&text, &Shipped), Ok(saved));
    let gone = text.replace("high-contrast", "mine");
    assert!(Appearance::decode(&gone, &Shipped).is_err());
    assert!(Appearance::decode("kai-appearance 1\nscale = 100\nscale = 125\n", &Shipped).is_err());
    assert!(Appearance::decode("kai-appearance 2\n", &Shipped).is_err());
}
