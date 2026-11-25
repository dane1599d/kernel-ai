//! Spike S10-2's 120 tasks: 80 for one stub app (ten each), 20 whose words
//! fit another app too, and 20 for no app (the OS's own rows, or talk).

/// A task's category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Single,
    Confusable,
    NoApp,
}

impl Cat {
    pub const ALL: [Cat; 3] = [Cat::Single, Cat::Confusable, Cat::NoApp];

    pub fn name(self) -> &'static str {
        match self {
            Cat::Single => "single-app",
            Cat::Confusable => "confusable",
            Cat::NoApp => "no-app",
        }
    }
}

/// An argument's expected value: text the answer must contain (any case),
/// or an exact integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Text(&'static str),
    Int(u64),
}

/// One task: the person's line and the call it should get. A task with no
/// row passes on any call that names no app row.
pub struct Task {
    pub cat: Cat,
    pub line: &'static str,
    pub row: Option<&'static str>,
    pub args: &'static [(&'static str, Want)],
}

const fn one(line: &'static str, row: &'static str, args: &'static [(&'static str, Want)]) -> Task {
    Task {
        cat: Cat::Single,
        line,
        row: Some(row),
        args,
    }
}

const fn near(
    line: &'static str,
    row: &'static str,
    args: &'static [(&'static str, Want)],
) -> Task {
    Task {
        cat: Cat::Confusable,
        line,
        row: Some(row),
        args,
    }
}

const fn none(line: &'static str) -> Task {
    Task {
        cat: Cat::NoApp,
        line,
        row: None,
        args: &[],
    }
}

use Want::{Int, Text};

pub static TASKS: [Task; 120] = [
    // todo
    one(
        "add buy stamps to my to-do list",
        "todo_add",
        &[("text", Text("stamps"))],
    ),
    one(
        "put call the plumber on my todo list",
        "todo_add",
        &[("text", Text("plumber"))],
    ),
    one(
        "new to-do: renew my passport",
        "todo_add",
        &[("text", Text("passport"))],
    ),
    one(
        "add a to-do item to water the plants",
        "todo_add",
        &[("text", Text("water"))],
    ),
    one(
        "mark to-do item 3 as done",
        "todo_done",
        &[("item", Int(3))],
    ),
    one(
        "I finished to-do number 1",
        "todo_done",
        &[("item", Int(1))],
    ),
    one("tick off to-do 2", "todo_done", &[("item", Int(2))]),
    one("what's on my to-do list?", "todo_list", &[]),
    one("show my open to-dos", "todo_list", &[]),
    one("list the to-do items I still have", "todo_list", &[]),
    // shopping
    one(
        "add milk to the shopping list",
        "shopping_add",
        &[("item", Text("milk"))],
    ),
    one(
        "we need eggs, put them on the shopping list",
        "shopping_add",
        &[("item", Text("egg"))],
    ),
    one(
        "put bread on the shopping list",
        "shopping_add",
        &[("item", Text("bread"))],
    ),
    one(
        "shopping list: add rice",
        "shopping_add",
        &[("item", Text("rice"))],
    ),
    one(
        "take butter off the shopping list",
        "shopping_remove",
        &[("item", Text("butter"))],
    ),
    one(
        "remove apples from my shopping list",
        "shopping_remove",
        &[("item", Text("apple"))],
    ),
    one(
        "I already bought the coffee, cross it off the shopping list",
        "shopping_remove",
        &[("item", Text("coffee"))],
    ),
    one("what's on the shopping list?", "shopping_list", &[]),
    one("show me my shopping list", "shopping_list", &[]),
    one("read out the shopping list", "shopping_list", &[]),
    // timer
    one(
        "start a 5 minute timer",
        "timer_start",
        &[("seconds", Int(300))],
    ),
    one(
        "set a timer for 90 seconds",
        "timer_start",
        &[("seconds", Int(90))],
    ),
    one(
        "count down 10 minutes",
        "timer_start",
        &[("seconds", Int(600))],
    ),
    one(
        "timer for half an hour",
        "timer_start",
        &[("seconds", Int(1800))],
    ),
    one(
        "start a 45 second countdown",
        "timer_start",
        &[("seconds", Int(45))],
    ),
    one("cancel the timer", "timer_cancel", &[]),
    one("stop the countdown", "timer_cancel", &[]),
    one("never mind the timer, cancel it", "timer_cancel", &[]),
    one("how long is left on the timer?", "timer_left", &[]),
    one(
        "how many seconds until the timer goes off?",
        "timer_left",
        &[],
    ),
    // alarm
    one(
        "set an alarm for 07:30",
        "alarm_set",
        &[("time", Text("7:30"))],
    ),
    one(
        "wake me up at 6:45 with an alarm",
        "alarm_set",
        &[("time", Text("6:45"))],
    ),
    one(
        "alarm at 22:00 please",
        "alarm_set",
        &[("time", Text("22:00"))],
    ),
    one(
        "set an alarm for 5:15",
        "alarm_set",
        &[("time", Text("5:15"))],
    ),
    one(
        "delete the 07:30 alarm",
        "alarm_clear",
        &[("time", Text("7:30"))],
    ),
    one(
        "remove my alarm at 6:00",
        "alarm_clear",
        &[("time", Text("6:00"))],
    ),
    one(
        "turn off the 9:30 alarm",
        "alarm_clear",
        &[("time", Text("9:30"))],
    ),
    one("which alarms are set?", "alarm_list", &[]),
    one("show my alarms", "alarm_list", &[]),
    one("do I have an alarm set?", "alarm_list", &[]),
    // weather
    one(
        "what's the weather in Paris?",
        "weather_now",
        &[("city", Text("paris"))],
    ),
    one(
        "is it raining in London right now?",
        "weather_now",
        &[("city", Text("london"))],
    ),
    one(
        "how warm is it in Tokyo?",
        "weather_now",
        &[("city", Text("tokyo"))],
    ),
    one(
        "current weather for Oslo",
        "weather_now",
        &[("city", Text("oslo"))],
    ),
    one(
        "weather in Cairo",
        "weather_now",
        &[("city", Text("cairo"))],
    ),
    one(
        "what's the forecast for Berlin this week?",
        "weather_week",
        &[("city", Text("berlin"))],
    ),
    one(
        "will it rain in Madrid in the next few days?",
        "weather_week",
        &[("city", Text("madrid"))],
    ),
    one(
        "weekly forecast for Rome",
        "weather_week",
        &[("city", Text("rome"))],
    ),
    one(
        "show the 7-day forecast for Dublin",
        "weather_week",
        &[("city", Text("dublin"))],
    ),
    one(
        "what will the weather be like in Lima this week?",
        "weather_week",
        &[("city", Text("lima"))],
    ),
    // music
    one("play some jazz", "music_play", &[("song", Text("jazz"))]),
    one(
        "put on Miles Davis",
        "music_play",
        &[("song", Text("miles davis"))],
    ),
    one(
        "play the song Blue Moon",
        "music_play",
        &[("song", Text("blue moon"))],
    ),
    one("pause the music", "music_pause", &[]),
    one("hold the music for a moment", "music_pause", &[]),
    one("skip this song", "music_skip", &[]),
    one("next track", "music_skip", &[]),
    one(
        "set the volume to 40",
        "music_volume",
        &[("level", Int(40))],
    ),
    one(
        "turn the music down to 10",
        "music_volume",
        &[("level", Int(10))],
    ),
    one("music volume 75", "music_volume", &[("level", Int(75))]),
    // contacts
    one(
        "what's Anna's phone number?",
        "contacts_find",
        &[("name", Text("anna"))],
    ),
    one(
        "find Bob in my contacts",
        "contacts_find",
        &[("name", Text("bob"))],
    ),
    one(
        "look up the number for Dr Lee",
        "contacts_find",
        &[("name", Text("lee"))],
    ),
    one(
        "how do I reach Maria? give me her number",
        "contacts_find",
        &[("name", Text("maria"))],
    ),
    one(
        "search contacts for Sam",
        "contacts_find",
        &[("name", Text("sam"))],
    ),
    one(
        "add a contact: Tom, 555-0101",
        "contacts_add",
        &[("name", Text("tom")), ("phone", Text("555-0101"))],
    ),
    one(
        "save Priya's number, 555-0199",
        "contacts_add",
        &[("name", Text("priya")), ("phone", Text("555-0199"))],
    ),
    one(
        "new contact Ken with phone 555-0123",
        "contacts_add",
        &[("name", Text("ken")), ("phone", Text("555-0123"))],
    ),
    one(
        "add Lucy to my contacts, her number is 555-0142",
        "contacts_add",
        &[("name", Text("lucy")), ("phone", Text("555-0142"))],
    ),
    one(
        "store the number 555-0177 for Omar",
        "contacts_add",
        &[("name", Text("omar")), ("phone", Text("555-0177"))],
    ),
    // calendar
    one(
        "add a dentist appointment on Friday to my calendar",
        "calendar_add",
        &[("title", Text("dentist")), ("day", Text("friday"))],
    ),
    one(
        "put team lunch on the calendar for Monday",
        "calendar_add",
        &[("title", Text("lunch")), ("day", Text("monday"))],
    ),
    one(
        "schedule a call with Jo on 2026-12-01",
        "calendar_add",
        &[("title", Text("jo")), ("day", Text("2026-12-01"))],
    ),
    one(
        "calendar: piano lesson on Wednesday",
        "calendar_add",
        &[("title", Text("piano")), ("day", Text("wednesday"))],
    ),
    one(
        "book yoga on Saturday in my calendar",
        "calendar_add",
        &[("title", Text("yoga")), ("day", Text("saturday"))],
    ),
    one(
        "what's on my calendar today?",
        "calendar_day",
        &[("day", Text("today"))],
    ),
    one(
        "what do I have on Thursday?",
        "calendar_day",
        &[("day", Text("thursday"))],
    ),
    one(
        "show my events for 2026-12-24",
        "calendar_day",
        &[("day", Text("2026-12-24"))],
    ),
    one(
        "am I busy tomorrow?",
        "calendar_day",
        &[("day", Text("tomorrow"))],
    ),
    one(
        "list my calendar for Sunday",
        "calendar_day",
        &[("day", Text("sunday"))],
    ),
    // confusable: words another app's rows share
    near(
        "add a to-do to buy milk",
        "todo_add",
        &[("text", Text("milk"))],
    ),
    near(
        "add milk to the list of groceries",
        "shopping_add",
        &[("item", Text("milk"))],
    ),
    near(
        "add buy a cake for Anna to my to-do list",
        "todo_add",
        &[("text", Text("cake"))],
    ),
    near("I've done to-do 4", "todo_done", &[("item", Int(4))]),
    near("list my to-dos, not the shopping", "todo_list", &[]),
    near(
        "remove eggs from the shopping list, it's not a to-do",
        "shopping_remove",
        &[("item", Text("egg"))],
    ),
    near(
        "wake me in 20 minutes",
        "timer_start",
        &[("seconds", Int(1200))],
    ),
    near(
        "alarm at 08:00 tomorrow morning",
        "alarm_set",
        &[("time", Text("8:00"))],
    ),
    near("how long until my countdown ends?", "timer_left", &[]),
    near(
        "what alarms do I have set for the morning?",
        "alarm_list",
        &[],
    ),
    near("cancel the countdown, keep the alarms", "timer_cancel", &[]),
    near(
        "clear the 6:30 alarm, not the timer",
        "alarm_clear",
        &[("time", Text("6:30"))],
    ),
    near(
        "put Anna's birthday on Tuesday in the calendar",
        "calendar_add",
        &[("title", Text("birthday")), ("day", Text("tuesday"))],
    ),
    near(
        "add Anna to contacts with 555-0111",
        "contacts_add",
        &[("name", Text("anna")), ("phone", Text("555-0111"))],
    ),
    near(
        "remind me about the dentist on Friday in my calendar",
        "calendar_add",
        &[("title", Text("dentist")), ("day", Text("friday"))],
    ),
    near(
        "find the number for the pizza place, Luigi's",
        "contacts_find",
        &[("name", Text("luigi"))],
    ),
    near(
        "what's the forecast for my trip to Rome this week?",
        "weather_week",
        &[("city", Text("rome"))],
    ),
    near(
        "what's on my calendar today, not the weather",
        "calendar_day",
        &[("day", Text("today"))],
    ),
    near(
        "play the song Shopping List",
        "music_play",
        &[("song", Text("shopping list"))],
    ),
    near(
        "turn the volume up to 60",
        "music_volume",
        &[("level", Int(60))],
    ),
    // no app: the OS's own rows, or talk
    none("how many CPUs are online?"),
    none("what's the scheduling policy?"),
    none("how long has the system been up?"),
    none("list the running services"),
    none("restart netd"),
    none("how much memory is free?"),
    none("switch the scheduler to round robin"),
    none("show the files in my home folder"),
    none("undo that"),
    none("hello!"),
    none("what can you do?"),
    none("tell me a joke"),
    none("translate thank you into Spanish"),
    none("what's 12 times 7?"),
    none("power off the computer"),
    none("is the network up?"),
    none("install htop"),
    none("show the devices"),
    none("which model are you running?"),
    none("what is the kernel heap using?"),
];
