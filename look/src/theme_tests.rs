use crate::appearance::{Appearance, Refused, Shipped, Themes};
use crate::theme::{self, Theme};
use crate::{Curve, DARK, Family, HIGH_CONTRAST, LIGHT, Motion, Token, TypeStyle};
use std::format;
use std::path::PathBuf;
use std::string::String;
use std::vec::Vec;

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

/// A `kai-theme 1` file written before `shell` and `hover` existed has neither.
/// It still parses and takes its base's two, whichever base it names.
#[test]
fn a_file_without_shell_or_hover_takes_its_bases() {
    for base in [&DARK, &LIGHT, &HIGH_CONTRAST] {
        let whole = Theme::from_palette(base, false).encode();
        let old: String = whole
            .lines()
            .filter(|l| !l.starts_with("shell = ") && !l.starts_with("hover = "))
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(
            old.lines().count() + 2,
            whole.lines().count(),
            "{}",
            base.name
        );
        let theme = Theme::parse(&old).unwrap();
        assert_eq!(theme.colour(Token::Shell), base.shell, "{}", base.name);
        assert_eq!(theme.colour(Token::Hover), base.hover, "{}", base.name);
        // Whole, it reads back the same as the old file did.
        assert_eq!(Theme::parse(&whole), Ok(theme), "{}", base.name);
    }
    // A short file, as a person would write one.
    let text = "kai-theme 1\nname = mine\nbase = light\naccent = #1F5FBF\n";
    let theme = Theme::parse(text).unwrap();
    assert_eq!(theme.colour(Token::Shell), LIGHT.shell);
    assert_eq!(theme.colour(Token::Hover), LIGHT.hover);
}

#[test]
fn shell_and_hover_are_named_in_a_file_once_and_checked() {
    let head = "kai-theme 1\nname = t\nbase = dark\n";
    // They are tokens of a file like the rest: set, given once.
    let theme = Theme::parse(&format!("{head}shell = #10131C\nhover = #262C38\n")).unwrap();
    assert_eq!(theme.colour(Token::Shell), 0x10_131C);
    assert_eq!(theme.colour(Token::Hover), 0x26_2C38);
    let twice = Theme::parse(&format!("{head}shell = #10131C\nshell = #10131C\n")).unwrap_err();
    assert_eq!((twice.line, twice.why.contains("twice")), (5, true));
    // `shell` is a full ground: text and marks must hold on it.
    let bright = Theme::parse(&format!("{head}shell = #FFFFFF\n")).unwrap_err();
    assert!(bright.why.starts_with("text on shell is "), "{bright:?}");
    let faint = Theme::parse(&format!("{head}shell = #1C2230\n")).unwrap_err();
    assert!(faint.why.contains(" on shell is "), "{faint:?}");
    // `hover` is a restricted ground: what is drawn on it must hold there.
    let pale = Theme::parse(&format!("{head}hover = #8890A0\n")).unwrap_err();
    assert!(pale.why.contains(" on hover is "), "{pale:?}");
    // What is never drawn on it is not held to it: `dim` is 4.09:1 on this
    // theme's `hover`, and the theme is whole all the same.
    assert!(crate::contrast(DARK.dim, DARK.hover) < 4.5);
    Theme::from_palette(&DARK, false).check().unwrap();
}

#[test]
fn a_token_over_the_theme_may_be_shell_or_hover() {
    let dark = Appearance::default();
    let shell = dark.set("token", "shell #10131C", &Shipped).unwrap();
    assert_eq!(
        shell.resolve(&Shipped).unwrap().colour(Token::Shell),
        0x10_131C
    );
    let refused = |r: Result<Appearance, Refused>| r.unwrap_err().0;
    assert!(refused(dark.set("token", "shell #FFFFFF", &Shipped)).starts_with("text on shell"));
    assert!(refused(dark.set("token", "hover #8890A0", &Shipped)).contains(" on hover "));
    // In the order of `Token::ALL`, so a saved file reads back the same.
    let both = shell.set("token", "hover #262C38", &Shipped).unwrap();
    assert_eq!(both.tokens[0].0, Token::Shell);
    assert_eq!(Appearance::decode(&both.encode(), &Shipped), Ok(both));
}

/// What a build from before `shell` and `hover` saved: a light theme with a
/// `muted` that clears every ground it knew (4.53:1 on the desk, the least) and
/// is 4.32:1 on `hover`, with the person's scale, text size and legible type.
const MAIN_ERA_LIGHT: &str = "kai-appearance 1\ntheme = light\ntoken = muted #666666\n\
                              scale = 150\ntext_size = 120\nreduced_motion = on\nlegible = on\n";
/// The same in dark: `text` is 4.68:1 on `raised`, the least, and 4.07:1 on
/// `hover`; `accent` is held on every ground.
const MAIN_ERA_DARK: &str = "kai-appearance 1\ntheme = dark\ntoken = text #888898\n\
                             token = accent #A0C8FF\nscale = 125\nlegible = on\n";

/// A save made before the two tokens may hold a colour that falls short on one
/// of them. Replaying it drops that one colour and keeps the rest, with a line
/// for the audit log: refusing the whole save would reset the theme, scale and
/// legible type to the defaults.
#[test]
fn a_save_from_before_shell_and_hover_loses_only_the_colour_that_falls_short() {
    // The colour really is fine on every earlier ground and short on `hover`.
    for (text, theme, token, rgb) in [
        (MAIN_ERA_LIGHT, &LIGHT, Token::Muted, 0x66_6666),
        (MAIN_ERA_DARK, &DARK, Token::Text, 0x88_8898),
    ] {
        for ground in [theme.window, theme.raised, theme.desk] {
            assert!(crate::contrast(rgb, ground) >= 4.5, "{}", theme.name);
        }
        let short = Theme::from_palette(theme, false)
            .with_colour(token, rgb)
            .shortfall()
            .unwrap();
        assert_eq!((short.token, short.ground), (token, Token::Hover));
        // Set now, it is refused like any other short colour, naming the pair.
        let name = crate::theme::TOKEN_NAMES[Token::ALL.iter().position(|t| *t == token).unwrap()];
        let now = Appearance::default()
            .set("theme", theme.name, &Shipped)
            .and_then(|a| a.set("token", &format!("{name} #{rgb:06X}"), &Shipped));
        assert!(
            now.unwrap_err()
                .0
                .starts_with(&format!("{name} on hover is ")),
            "{text:?}"
        );
        assert!(Appearance::decode(text, &Shipped).is_ok(), "{text:?}");
    }

    let (light, dropped) = Appearance::decode_audited(MAIN_ERA_LIGHT, &Shipped).unwrap();
    assert_eq!(light.theme, "light");
    assert!(light.tokens.is_empty());
    assert_eq!((light.scale, light.text_size), (150, 120));
    assert!(light.reduced_motion && light.legible);
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(
        dropped[0].starts_with("the saved colour muted #666666 is dropped: muted on hover is 4.3"),
        "{dropped:?}"
    );
    light.resolve(&Shipped).unwrap();

    // One of two goes, the other stays.
    let (dark, dropped) = Appearance::decode_audited(MAIN_ERA_DARK, &Shipped).unwrap();
    assert_eq!(dark.tokens, [(Token::Accent, 0xA0_C8FF)]);
    assert_eq!(
        (dark.theme.as_str(), dark.scale, dark.legible),
        ("dark", 125, true)
    );
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(dropped[0].contains("text #888898"), "{dropped:?}");
    // `decode` gives the same appearance, and what it saves next has no trace of
    // the colour, so the save reads back whole and with nothing dropped.
    assert_eq!(
        Appearance::decode(MAIN_ERA_DARK, &Shipped),
        Ok(dark.clone())
    );
    let again = Appearance::decode_audited(&dark.encode(), &Shipped).unwrap();
    assert_eq!(again, (dark, Vec::new()));
}

/// Only a shortfall on `shell` or `hover` is let go. One on an earlier ground was
/// one before too, and a save that holds it, or anything else out of shape, is
/// refused whole as it always was.
#[test]
fn a_save_that_was_short_before_is_still_refused_whole() {
    let muted = "kai-appearance 1\ntheme = dark\ntoken = muted #30343C\nscale = 150\n";
    let err = Appearance::decode_audited(muted, &Shipped).unwrap_err();
    assert!(err.0.starts_with("muted on window is "), "{err:?}");
    let ai = "kai-appearance 1\ntheme = dark\ntoken = accent #FF9F5A\n";
    assert!(
        Appearance::decode_audited(ai, &Shipped)
            .unwrap_err()
            .0
            .contains("ai's colour")
    );
    let gone = "kai-appearance 1\ntheme = sepia\n";
    assert!(Appearance::decode_audited(gone, &Shipped).is_err());
    // A save with nothing short reports nothing dropped.
    let fine = "kai-appearance 1\ntheme = dark\ntoken = accent #A0C8FF\n";
    assert_eq!(
        Appearance::decode_audited(fine, &Shipped).unwrap().1,
        Vec::<String>::new()
    );
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
