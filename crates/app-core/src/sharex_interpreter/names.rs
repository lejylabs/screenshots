//! ShareX name-pattern codes. Vocabularies are original; English calendar names are used.
use super::{Context, Fail, R, limits, random_below};
use jiff::civil::{DateTime, Weekday};

const ALPHA: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const INVERSE: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const BASE56: &str = "23456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz";
const ADJECTIVES: [&str; 8] = ["Bright", "Calm", "Clever", "Gentle", "Quiet", "Silver", "Swift", "Warm"];
const ANIMALS: [&str; 8] = ["Badger", "Falcon", "Fox", "Lynx", "Otter", "Owl", "Panda", "Wolf"];
const EMOJI: [&str; 12] = ["😀", "😎", "🐺", "🦊", "🐈", "🦉", "🌙", "⭐", "📷", "🎨", "❤️", "🚀"];
/// Longest first so prefixes never shadow longer codes.
const CODES: [&str; 44] = [
    "radjective", "ranimal", "filename", "remoji", "height", "width", "unix", "guid", "GUID", "mon2", "mon", "iAa",
    "iaA", "rna", "uln", "pn", "yy", "mo", "wy", "w2", "mi", "ms", "pm", "ia", "iA", "ib", "iB", "ix", "iX", "rn",
    "ra", "rx", "rX", "rf", "un", "cn", "t", "y", "w", "d", "h", "s", "i", "n",
];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

fn random_chars(alphabet: &str, count: i32) -> R<String> {
    limits::require((0..=4096).contains(&count), "name_repeat_limit")?;
    let chars: Vec<char> = alphabet.chars().collect();
    Ok((0..count).map(|_| chars[random_below(chars.len())]).collect())
}
fn base(mut value: i64, radix: i32, alphabet: &str) -> R<String> {
    limits::require((2..=62).contains(&radix), "name_base")?;
    if value == 0 {
        return Ok("0".into());
    }
    let chars: Vec<char> = alphabet.chars().collect();
    let mut out = Vec::new();
    while value > 0 {
        out.push(chars[(value % radix as i64) as usize]);
        value /= radix as i64;
    }
    Ok(out.into_iter().rev().collect())
}
/// .NET `int.TryParse(..., Integer, Invariant)` with a fallback.
fn number(value: Option<&str>, fallback: i32) -> i32 {
    value.and_then(|text| text.trim().parse().ok()).unwrap_or(fallback)
}
fn pad(value: String, width: i32) -> R<String> {
    limits::require((0..=4096).contains(&width), "name_repeat_limit")?;
    let width = width as usize;
    let len = value.chars().count();
    Ok(if len >= width { value } else { "0".repeat(width - len) + &value })
}
fn sanitize(value: &str) -> String {
    value.trim().replace(' ', "_")
}
fn contains_code(value: &[char], code: &str, escape: bool) -> bool {
    let pattern: Vec<char> = std::iter::once('%').chain(code.chars()).collect();
    let mut i = 0;
    while i < value.len() {
        if escape && value[i] == '\\' && matches!(value.get(i + 1), Some('\\' | '%')) {
            i += 2;
            continue;
        }
        if value[i..].starts_with(&pattern) {
            return true;
        }
        i += 1;
    }
    false
}

struct Now {
    civil: DateTime,
    unix: i64,
}
fn now(context: &Context, frozen_only: bool) -> R<Now> {
    use std::str::FromStr;
    let system = jiff::tz::TimeZone::system();
    if let Some(text) = context.names.get("now") {
        let text = text.trim();
        if let Ok(stamp) = jiff::Timestamp::from_str(text) {
            let utc = text.ends_with(['Z', 'z']);
            let civil = if utc {
                stamp.to_zoned_datetime(jiff::tz::TimeZone::UTC).datetime()
            } else {
                stamp.to_zoned_datetime(system).datetime()
            };
            return Ok(Now { civil, unix: stamp.as_second() });
        }
        let civil = DateTime::from_str(text).map_err(|_| Fail::invalid())?;
        let unix = civil.to_zoned_datetime(system).map_err(|_| Fail::invalid())?.timestamp().as_second();
        return Ok(Now { civil, unix });
    }
    if frozen_only {
        return Ok(Now { civil: DateTime::constant(1970, 1, 1, 0, 0, 0, 0), unix: 0 });
    }
    let zoned = jiff::ZonedDateTime::now();
    Ok(Now { civil: zoned.datetime(), unix: zoned.timestamp().as_second() })
}
fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}
/// `Calendar.GetWeekOfYear(date, CalendarWeekRule.FirstDay, DayOfWeek.Monday)`.
fn week_of_year(date: DateTime) -> i32 {
    let first = DateTime::constant(date.year(), 1, 1, 0, 0, 0, 0);
    let offset = first.weekday().to_monday_zero_offset() as i32;
    (date.day_of_year() as i32 - 1 + offset) / 7 + 1
}
fn identity(context: &Context, name: &str) -> R<String> {
    if let Some(value) = context.names.get(name) {
        return Ok(value.clone());
    }
    limits::require(context.allow_environment, "identity_context_required")?;
    let var = |names: &[&str]| names.iter().find_map(|n| std::env::var(n).ok()).unwrap_or_default();
    Ok(match name {
        "user" => var(&["USERNAME", "USER", "LOGNAME"]),
        "domain" if cfg!(windows) => var(&["USERDOMAIN", "COMPUTERNAME"]),
        _ => machine_name(),
    })
}
#[cfg(windows)]
fn machine_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}
#[cfg(unix)]
fn machine_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its full length and gethostname NUL-terminates on success.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    let name = String::from_utf8_lossy(&buffer[..end]).into_owned();
    name.split('.').next().unwrap_or_default().to_owned()
}

pub(super) fn expand(source: &str, context: &Context, preserve_escapes: bool, frozen_only: bool) -> R<String> {
    let source: Vec<char> = source.chars().collect();
    let now = now(context, frozen_only)?;
    let increment = number(context.names.get("counter").map(String::as_str), 0)
        .checked_add(1)
        .ok_or_else(Fail::invalid)? as i64;
    let twelve = contains_code(&source, "pm", preserve_escapes);
    let mut result = String::new();
    let mut length = 0usize;
    let mut position = 0;
    while position < source.len() {
        if preserve_escapes && source[position] == '\\' && matches!(source.get(position + 1), Some('\\' | '%')) {
            result.push(source[position]);
            result.push(source[position + 1]);
            length += 2;
            position += 2;
            continue;
        }
        if source[position] != '%' {
            result.push(source[position]);
            length += 1;
            position += 1;
            continue;
        }
        let rest = &source[position + 1..];
        let Some(code) = CODES.iter().copied().find(|code| {
            let code: Vec<char> = code.chars().collect();
            rest.starts_with(&code)
        }) else {
            result.push('%');
            length += 1;
            position += 1;
            continue;
        };
        let mut next = position + code.len() + 1;
        let mut argument: Option<String> = None;
        let accepts = matches!(
            code,
            "rna" | "rn" | "ra" | "rx" | "rX" | "remoji" | "rf" | "i" | "ia" | "iA" | "iAa" | "iaA" | "ib" | "iB" | "ix" | "iX"
        );
        if accepts && source.get(next) == Some(&'{') {
            let end = source[next + 1..].iter().position(|ch| *ch == '}').map(|n| n + next + 1);
            let end = end.ok_or(Fail::Code("name_argument"))?;
            argument = Some(source[next + 1..end].iter().collect());
            next = end + 1;
        }
        let count = if argument.is_some() { number(argument.as_deref(), 0) } else { 1 };
        let width = if argument.is_some() { count } else { 0 };
        let random = matches!(code, "rna" | "rn" | "ra" | "rx" | "rX" | "remoji" | "guid" | "GUID" | "radjective" | "ranimal");
        let clock = matches!(
            code,
            "yy" | "y" | "mon2" | "mon" | "mo" | "d" | "h" | "mi" | "s" | "ms" | "pm" | "wy" | "w2" | "w" | "unix"
        );
        let identity_key = match code {
            "un" => Some("user"),
            "uln" => Some("domain"),
            "cn" => Some("computer"),
            _ => None,
        };
        if frozen_only
            && (random
                || clock && !context.names.contains_key("now")
                || identity_key.is_some_and(|key| !context.names.contains_key(key)))
        {
            result.push_str(context.unknown_marker());
            length += 50;
            position = next;
            continue;
        }
        let civil = now.civil;
        let positive = |key: &str| {
            let value = number(context.names.get(key).map(String::as_str), 0);
            if value > 0 { value.to_string() } else { String::new() }
        };
        let replacement: Option<String> = match code {
            "width" => Some(positive("width")),
            "height" => Some(positive("height")),
            "t" => context.names.get("title").map(|v| sanitize(v)),
            "pn" => context.names.get("process").map(|v| sanitize(v)),
            "yy" => Some(format!("{:02}", civil.year().rem_euclid(100))),
            "y" => Some(civil.year().to_string()),
            "mon2" | "mon" => Some(MONTHS[civil.month() as usize - 1].into()),
            "mo" => Some(format!("{:02}", civil.month())),
            "d" => Some(format!("{:02}", civil.day())),
            "h" => {
                let hour = civil.hour();
                let hour = if twelve { if hour % 12 == 0 { 12 } else { hour % 12 } } else { hour };
                Some(format!("{hour:02}"))
            }
            "mi" => Some(format!("{:02}", civil.minute())),
            "s" => Some(format!("{:02}", civil.second())),
            "ms" => Some(format!("{:03}", civil.millisecond())),
            "pm" => Some(if civil.hour() >= 12 { "PM" } else { "AM" }.into()),
            "wy" => Some(week_of_year(civil).to_string()),
            "w2" | "w" => Some(weekday_name(civil.weekday()).into()),
            "unix" => Some(
                if context.names.contains_key("now") { now.unix } else { jiff::Timestamp::now().as_second() }.to_string(),
            ),
            "un" | "uln" | "cn" => Some(identity(context, identity_key.unwrap_or("computer"))?),
            "n" => Some(if cfg!(windows) { "\r\n" } else { "\n" }.into()),
            "guid" => Some(uuid::Uuid::new_v4().hyphenated().to_string()),
            "GUID" => Some(uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()),
            "radjective" => Some(ADJECTIVES[random_below(ADJECTIVES.len())].into()),
            "ranimal" => Some(ANIMALS[random_below(ANIMALS.len())].into()),
            "rn" => Some(random_chars("0123456789", count)?),
            "ra" => Some(random_chars(ALPHA, count)?),
            "rna" => Some(random_chars(BASE56, count)?),
            "rx" => Some(random_chars("0123456789abcdef", count)?),
            "rX" => Some(random_chars("0123456789ABCDEF", count)?),
            "remoji" => {
                limits::require((0..=4096).contains(&count), "name_repeat_limit")?;
                Some((0..count).map(|_| EMOJI[random_below(EMOJI.len())]).collect())
            }
            "rf" if argument.is_some() => Some(String::new()),
            "i" => Some(pad(increment.to_string(), width)?),
            "ia" => Some(pad(base(increment, 36, ALPHA)?.to_lowercase(), width)?),
            "iA" => Some(pad(base(increment, 36, ALPHA)?.to_uppercase(), width)?),
            "iAa" => Some(pad(base(increment, 62, ALPHA)?, width)?),
            "iaA" => Some(pad(base(increment, 62, INVERSE)?, width)?),
            "ix" => Some(pad(format!("{increment:x}"), width)?),
            "iX" => Some(pad(format!("{increment:X}"), width)?),
            "ib" | "iB" => Some(match &argument {
                Some(argument) => {
                    let mut pieces = argument.split(',');
                    let radix = number(pieces.next(), 0);
                    let width = number(pieces.next(), 0);
                    pad(base(increment, radix, if code == "iB" { ALPHA } else { INVERSE })?, width)?
                }
                None => format!("{increment}{}", &code[1..]),
            }),
            _ => None,
        };
        match replacement {
            Some(text) => {
                length += text.chars().count();
                result.push_str(&text);
            }
            None => {
                let raw: String = source[position..next].iter().collect();
                length += next - position;
                result.push_str(&raw);
            }
        }
        limits::require(length <= limits::TEXT, "expansion_limit")?;
        position = next;
    }
    limits::bounded(result, limits::TEXT)
}
