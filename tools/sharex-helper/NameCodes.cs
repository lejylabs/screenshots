using System.Globalization;
using System.Security.Cryptography;
using System.Text;

namespace Screenshots.ShareXRuntime;

internal static class NameCodes
{
    const string Alpha = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz", Inverse = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ", Base56 = "23456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz";
    // Independently authored vocabularies; no upstream resource data is bundled.
    static readonly string[] Adjectives = ["Bright", "Calm", "Clever", "Gentle", "Quiet", "Silver", "Swift", "Warm"], Animals = ["Badger", "Falcon", "Fox", "Lynx", "Otter", "Owl", "Panda", "Wolf"], Emoji = ["😀", "😎", "🐺", "🦊", "🐈", "🦉", "🌙", "⭐", "📷", "🎨", "❤️", "🚀"];
    static readonly string[] Codes = new[] { "radjective", "ranimal", "remoji", "filename", "height", "width", "unix", "guid", "GUID", "mon2", "mon", "iAa", "iaA", "rna", "uln", "pn", "yy", "mo", "wy", "w2", "mi", "ms", "pm", "ia", "iA", "ib", "iB", "ix", "iX", "rn", "ra", "rx", "rX", "rf", "un", "cn", "t", "y", "w", "d", "h", "s", "i", "n" }.OrderByDescending(value => value.Length).ToArray();
    static string RandomChars(string alphabet, int count)
    {
        Limits.Require(count is >= 0 and <= 4096, "name_repeat_limit");
        return string.Create(count, alphabet, (span, chars) => { for (int i = 0; i < span.Length; i++) span[i] = chars[RandomNumberGenerator.GetInt32(chars.Length)]; });
    }
    static string Base(int value, int radix, string alphabet)
    {
        Limits.Require(radix is >= 2 and <= 62, "name_base");
        if (value == 0) return "0";
        var result = new StringBuilder();
        while (value > 0) { result.Insert(0, alphabet[value % radix]); value /= radix; }
        return result.ToString();
    }
    static int Number(string? value, int fallback = 0) => int.TryParse(value, NumberStyles.Integer, CultureInfo.InvariantCulture, out int result) ? result : fallback;
    static string Pad(string value, int width) { Limits.Require(width is >= 0 and <= 4096, "name_repeat_limit"); return value.PadLeft(width, '0'); }
    static string Sanitize(string value) => value.Trim().Replace(' ', '_');
    static bool ContainsCode(string value, string code, bool escape)
    {
        for (int i = 0; i < value.Length; i++)
        {
            if (escape && value[i] == '\\' && i + 1 < value.Length && value[i + 1] is '\\' or '%') { i++; continue; }
            if (value.AsSpan(i).StartsWith("%" + code, StringComparison.Ordinal)) return true;
        }
        return false;
    }
    internal static string Expand(string source, Context context, bool preserveEscapes, bool frozenOnly = false)
    {
        DateTime now = context.Names.TryGetValue("now", out string? instant) ? DateTime.Parse(instant, CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind) : frozenOnly ? DateTime.UnixEpoch : DateTime.Now;
        int increment = checked(Number(context.Names.GetValueOrDefault("counter")) + 1);
        bool twelve = ContainsCode(source, "pm", preserveEscapes);
        var result = new StringBuilder();
        for (int position = 0; position < source.Length;)
        {
            if (preserveEscapes && source[position] == '\\' && position + 1 < source.Length && source[position + 1] is '\\' or '%') { result.Append(source, position, 2); position += 2; continue; }
            if (source[position] != '%') { result.Append(source[position++]); continue; }
            string? code = Codes.FirstOrDefault(code => source.AsSpan(position + 1).StartsWith(code, StringComparison.Ordinal));
            if (code == null) { result.Append(source[position++]); continue; }
            int next = position + code.Length + 1; string? argument = null;
            bool accepts = code is "rna" or "rn" or "ra" or "rx" or "rX" or "remoji" or "rf" or "i" or "ia" or "iA" or "iAa" or "iaA" or "ib" or "iB" or "ix" or "iX";
            if (accepts && next < source.Length && source[next] == '{')
            {
                int end = source.IndexOf('}', next + 1); Limits.Require(end >= 0, "name_argument");
                argument = source[(next + 1)..end]; next = end + 1;
            }
            int count = argument == null ? 1 : Number(argument);
            bool random=code is "rna" or "rn" or "ra" or "rx" or "rX" or "remoji" or "guid" or "GUID" or "radjective" or "ranimal";
            bool clock=code is "yy" or "y" or "mon2" or "mon" or "mo" or "d" or "h" or "mi" or "s" or "ms" or "pm" or "wy" or "w2" or "w" or "unix";
            bool identity=code is "un" or "uln" or "cn";
            string identityKey=code=="un"?"user":code=="uln"?"domain":"computer";
            if(frozenOnly&&(random||clock&&!context.Names.ContainsKey("now")||identity&&!context.Names.ContainsKey(identityKey))){result.Append(context.UnknownExpansionMarker);position=next;continue;}

            string? replacement = code switch
            {
                "width" => Positive(context.Names.GetValueOrDefault("width")), "height" => Positive(context.Names.GetValueOrDefault("height")),
                "t" => context.Names.TryGetValue("title", out string? title) ? Sanitize(title) : null,
                "pn" => context.Names.TryGetValue("process", out string? process) ? Sanitize(process) : null,
                "yy" => now.ToString("yy", CultureInfo.CurrentCulture), "y" => now.Year.ToString(CultureInfo.CurrentCulture),
                "mon2" => CultureInfo.InvariantCulture.DateTimeFormat.GetMonthName(now.Month), "mon" => CultureInfo.CurrentCulture.DateTimeFormat.GetMonthName(now.Month),
                "mo" => now.Month.ToString("D2", CultureInfo.CurrentCulture), "d" => now.Day.ToString("D2", CultureInfo.CurrentCulture),
                "h" => (twelve ? (now.Hour % 12 == 0 ? 12 : now.Hour % 12) : now.Hour).ToString("D2", CultureInfo.CurrentCulture),
                "mi" => now.Minute.ToString("D2", CultureInfo.CurrentCulture), "s" => now.Second.ToString("D2", CultureInfo.CurrentCulture), "ms" => now.Millisecond.ToString("D3", CultureInfo.CurrentCulture),
                "pm" => now.Hour >= 12 ? "PM" : "AM", "wy" => CultureInfo.CurrentCulture.Calendar.GetWeekOfYear(now, CalendarWeekRule.FirstDay, DayOfWeek.Monday).ToString(CultureInfo.CurrentCulture),
                "w2" => CultureInfo.InvariantCulture.DateTimeFormat.GetDayName(now.DayOfWeek), "w" => CultureInfo.CurrentCulture.DateTimeFormat.GetDayName(now.DayOfWeek),
                "unix" => (context.Names.ContainsKey("now") ? new DateTimeOffset(now).ToUnixTimeSeconds() : DateTimeOffset.UtcNow.ToUnixTimeSeconds()).ToString(CultureInfo.InvariantCulture),
                "un" => Identity(context, "user", () => Environment.UserName), "uln" => Identity(context, "domain", () => Environment.UserDomainName), "cn" => Identity(context, "computer", () => Environment.MachineName),
                "n" => Environment.NewLine, "guid" => Guid.NewGuid().ToString("D").ToLowerInvariant(), "GUID" => Guid.NewGuid().ToString("D").ToUpperInvariant(),
                "radjective" => Adjectives[RandomNumberGenerator.GetInt32(Adjectives.Length)], "ranimal" => Animals[RandomNumberGenerator.GetInt32(Animals.Length)],
                "rn" => RandomChars("0123456789", count), "ra" => RandomChars(Alpha, count), "rna" => RandomChars(Base56, count), "rx" => RandomChars("0123456789abcdef", count), "rX" => RandomChars("0123456789ABCDEF", count),
                "remoji" => RandomEmoji(count), "rf" when argument != null => "",
                "i" => Pad(increment.ToString(CultureInfo.InvariantCulture), argument == null ? 0 : count),
                "ia" => Pad(Base(increment, 36, Alpha).ToLowerInvariant(), argument == null ? 0 : count), "iA" => Pad(Base(increment, 36, Alpha).ToUpperInvariant(), argument == null ? 0 : count),
                "iAa" => Pad(Base(increment, 62, Alpha), argument == null ? 0 : count), "iaA" => Pad(Base(increment, 62, Inverse), argument == null ? 0 : count),
                "ix" => Pad(increment.ToString("x", CultureInfo.InvariantCulture), argument == null ? 0 : count), "iX" => Pad(increment.ToString("X", CultureInfo.InvariantCulture), argument == null ? 0 : count),
                "ib" or "iB" when argument != null => BaseArgument(increment, argument, code == "iB"),
                "ib" or "iB" => increment.ToString(CultureInfo.InvariantCulture) + code[1],
                _ => null
            };
            result.Append(replacement ?? source[position..next]);
            Limits.Require(result.Length <= Limits.Text, "expansion_limit"); position = next;
        }
        return Limits.Bounded(result.ToString());
    }

    static string Positive(string? value) => Number(value) > 0 ? Number(value).ToString(CultureInfo.CurrentCulture) : "";
    static string Identity(Context context, string name, Func<string> value)
    {
        if (context.Names.TryGetValue(name, out string? supplied)) return supplied;
        Limits.Require(context.AllowEnvironment, "identity_context_required"); return value();
    }
    static string BaseArgument(int number, string argument, bool capitals)
    {
        var pieces = argument.Split(','); int radix = Number(pieces[0]), width = pieces.Length > 1 ? Number(pieces[1]) : 0;
        return Pad(Base(number, radix, capitals ? Alpha : Inverse), width);
    }
    static string RandomEmoji(int count)
    {
        Limits.Require(count is >= 0 and <= 4096, "name_repeat_limit"); var result = new StringBuilder();
        for (int i = 0; i < count; i++) result.Append(Emoji[RandomNumberGenerator.GetInt32(Emoji.Length)]);
        return result.ToString();
    }
}
