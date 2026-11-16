// Original implementation of the published configuration contract; no ShareX code is included.
using System.Globalization;
using System.Text;
using System.Web;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace Screenshots.ShareXRuntime;

internal sealed class Failure(string code) : Exception(code) { public string Code { get; } = code; }
internal static class Limits
{
    internal const int Input = 4 * 1024 * 1024, Output = 2 * 1024 * 1024, Text = 65536, Body = 512 * 1024;
    internal static void Require(bool value, string code) { if (!value) throw new Failure(code); }
    internal static string Bounded(string value, int bytes = Text) { Require(Encoding.UTF8.GetByteCount(value) <= bytes, "expansion_limit"); return value; }
    internal static JObject Object(string json)
    {
        using var reader = new JsonTextReader(new StringReader(json)) { DateParseHandling = DateParseHandling.None, MaxDepth = 64 };
        var value = JObject.Load(reader, new JsonLoadSettings { DuplicatePropertyNameHandling = DuplicatePropertyNameHandling.Error, LineInfoHandling = LineInfoHandling.Ignore });
        while (reader.Read()) { }
        return value;
    }
    internal static string String(JToken? value, int bytes = Text)
    {
        Require(value == null || value.Type is JTokenType.String or JTokenType.Null, "field_type");
        return Bounded(value?.Type == JTokenType.String ? value.Value<string>()! : "", bytes);
    }
}

internal sealed class Profile
{
    internal string Version = "", Name = "", Method = "POST", Url = "", Body = "None", Field = "", Data = "", Link = "", Thumbnail = "", Deletion = "", Error = "";
    internal int Destinations;
    internal Dictionary<string, string> Parameters = new(StringComparer.Ordinal), Headers = new(StringComparer.Ordinal), Arguments = new(StringComparer.Ordinal);
    static readonly string[] Keys = ["Version", "Name", "DestinationType", "RequestMethod", "RequestURL", "Parameters", "Headers", "Body", "Arguments", "FileFormName", "Data", "URL", "ThumbnailURL", "DeletionURL", "ErrorMessage"];
    static readonly string[] Methods = ["GET", "POST", "PUT", "PATCH", "DELETE"], Bodies = ["None", "MultipartFormData", "FormURLEncoded", "JSON", "XML", "Binary"], DestinationsNames = ["ImageUploader", "TextUploader", "FileUploader", "URLShortener", "URLSharingService"];
    internal static string EnumValue(JToken? token, string[] names, string fallback)
    {
        if (token == null || token.Type == JTokenType.Null) return fallback;
        if (token.Type == JTokenType.Integer) { int index = token.Value<int>(); Limits.Require(index >= 0 && index < names.Length, "enum_value"); return names[index]; }
        string value = Limits.String(token, 128).Trim();
        return names.FirstOrDefault(name => name.Equals(value, StringComparison.OrdinalIgnoreCase)) ?? throw new Failure("enum_value");
    }
    static Dictionary<string, string> Map(JToken? token)
    {
        if (token == null || token.Type == JTokenType.Null) return new(StringComparer.Ordinal);
        Limits.Require(token is JObject, "map_type");
        var result = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var property in ((JObject)token).Properties())
        {
            Limits.Require(result.Count < 32 && property.Name.Length is > 0 and <= 128, "map_limit");
            result.Add(property.Name, Limits.String(property.Value));
        }
        return result;
    }
    internal static Profile Read(string json)
    {
        Limits.Bounded(json, 16384);
        var source = Limits.Object(json); var canonical = new JObject();
        foreach (var property in source.Properties())
        {
            string key = Keys.FirstOrDefault(key => key.Equals(property.Name, StringComparison.OrdinalIgnoreCase)) ?? throw new Failure("unknown_field");
            Limits.Require(canonical.Property(key) == null, "duplicate_field"); canonical[key] = property.Value;
        }
        var p = new Profile { Version = Limits.String(canonical["Version"], 64), Name = Limits.String(canonical["Name"]), Url = Limits.String(canonical["RequestURL"]), Field = Limits.String(canonical["FileFormName"], 128), Data = Limits.String(canonical["Data"]), Link = Limits.String(canonical["URL"]), Thumbnail = Limits.String(canonical["ThumbnailURL"]), Deletion = Limits.String(canonical["DeletionURL"]), Error = Limits.String(canonical["ErrorMessage"]), Parameters = Map(canonical["Parameters"]), Headers = Map(canonical["Headers"]), Arguments = Map(canonical["Arguments"]), Method = EnumValue(canonical["RequestMethod"], Methods, "POST"), Body = EnumValue(canonical["Body"], Bodies, "None") };
        Limits.Require(System.Version.TryParse(p.Version, out var parsedVersion), "unsupported_version");
        var version = parsedVersion == null ? throw new Failure("unsupported_version") : new System.Version(parsedVersion.Major, parsedVersion.Minor, Math.Max(0, parsedVersion.Build), Math.Max(0, parsedVersion.Revision));
        Limits.Require(version > new System.Version(12, 3, 1, 0), "unsupported_version");
        JToken? destination = canonical["DestinationType"];
        if (destination?.Type == JTokenType.Integer) p.Destinations = destination.Value<int>();
        else foreach (string name in Limits.String(destination, 256).Split(',', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries))
        {
            if (name.Equals("None", StringComparison.OrdinalIgnoreCase)) continue;
            int index = Array.FindIndex(DestinationsNames, value => value.Equals(name, StringComparison.OrdinalIgnoreCase));
            Limits.Require(index >= 0, "destination_value"); p.Destinations |= 1 << index;
        }
        Limits.Require(p.Destinations is >= 0 and <= 31 && p.Url.Length > 0, "destination_value");
        int query = p.Url.IndexOf('?');
        if (query >= 0)
        {
            var values = HttpUtility.ParseQueryString(p.Url[(query + 1)..]);
            foreach (string? key in values.AllKeys)
            {
                if (key == null) { foreach (string value in values.GetValues(null) ?? []) p.Parameters.TryAdd(value, ""); }
                else p.Parameters.TryAdd(key, values[key] ?? "");
            }
            p.Url = p.Url[..query];
        }
        if (version <= new System.Version(13, 7, 1, 0))
        {
            p.Url = Migrate(p.Url); p.Link = Migrate(p.Link); p.Thumbnail = Migrate(p.Thumbnail); p.Deletion = Migrate(p.Deletion); p.Error = Migrate(p.Error);
            foreach (var map in new[] { p.Parameters, p.Headers, p.Arguments }) foreach (string key in map.Keys.ToArray()) map[key] = Migrate(map[key]);
            p.Data = p.Data.Replace("$input$", "{input}", StringComparison.OrdinalIgnoreCase).Replace("$filename$", "{filename}", StringComparison.OrdinalIgnoreCase);
        }
        return p;
    }
    static string Migrate(string source)
    {
        var output = new StringBuilder(); bool opening = true;
        for (int i = 0; i < source.Length; i++)
        {
            char c = source[i];
            if (c == '$') { output.Append(opening ? '{' : '}'); opening = !opening; }
            else if (c == '\\') { i++; }
            else { if (c is '{' or '}') output.Append('\\'); output.Append(c); }
        }
        return output.ToString();
    }
    internal void AdmitKind(string kind)
    {
        int flag = kind switch { "image" => 1, "text" => 2, "file" => 4, "url" => 8, "url_sharing" => 16, _ => throw new Failure("input_kind") };
        Limits.Require((Destinations & flag) != 0, "input_destination");
        Limits.Require(kind is not ("image" or "file") || Body is "MultipartFormData" or "Binary", "input_body");
        Limits.Require(kind is not ("url" or "url_sharing") || Body != "Binary", "input_body");
        Limits.Require(kind is not ("image" or "file") || Body != "MultipartFormData" || Field.Length > 0, "file_form_name");
    }
}

internal sealed class Context
{
    internal string Filename = "", Input = "", Response = "", ResponseUrl = "", Kind = "file";
    internal int Status = 200;
    internal bool AllowEnvironment;
    internal Dictionary<string, uint> AnswerIndices = new(StringComparer.Ordinal);
    internal Dictionary<string, string> FunctionNames = new(StringComparer.Ordinal);
    internal Dictionary<string, string> Answers = new(StringComparer.Ordinal), Names = new(StringComparer.Ordinal), Headers = new(StringComparer.OrdinalIgnoreCase);
    internal List<object> Events = [];
    string? unknownMarker;
    internal string UnknownExpansionMarker => unknownMarker ??= "9876543210" + string.Concat(Enumerable.Range(0,40).Select(_ => System.Security.Cryptography.RandomNumberGenerator.GetInt32(10).ToString(CultureInfo.InvariantCulture)));
    internal string Answer(string id, string title, string[]? choices = null)
    {
        if (choices != null && AnswerIndices.TryGetValue(id, out uint index)) { Limits.Require(index < choices.Length && !Answers.ContainsKey(id), "selection_index"); return choices[(int)index]; }
        Limits.Require(Answers.TryGetValue(id, out string? answer), "answer_required");
        Limits.Bounded(answer!, 4096);
        Limits.Require(choices == null || choices.Contains(answer!, StringComparer.Ordinal), "selection_answer");
        return answer!;
    }
    internal static Context Read(JObject root)
    {
        var c = new Context { Filename = Limits.String(root["filename"], 128), Input = Limits.String(root["input"], 65536), Kind = Limits.String(root["kind"], 32), AllowEnvironment = root["allow_environment_names"]?.Value<bool>() ?? false };
        foreach (var pair in new[] { ("answers", c.Answers), ("name_values", c.Names), ("function_names", c.FunctionNames) })
        {
            if (root[pair.Item1] is not JObject map) continue;
            foreach (var field in map.Properties()) { Limits.Require(pair.Item2.Count < 128, "context_limit"); pair.Item2.Add(field.Name, Limits.String(field.Value, 4096)); }
        }
        if (root["answer_indices"] is JObject indices) foreach (var item in indices.Properties()) { Limits.Require(c.AnswerIndices.Count + c.Answers.Count < 128 && item.Name.Length is > 0 and <= 256 && item.Value.Type == JTokenType.Integer && item.Value.Value<long>() is >= 0 and < 16384 && !c.Answers.ContainsKey(item.Name), "selection_index"); c.AnswerIndices.Add(item.Name, item.Value.Value<uint>()); }
        foreach (var name in c.FunctionNames.Values) Limits.Require(Syntax.Functions.ContainsKey(name.ToLowerInvariant()), "function_binding");
        if (root["response"] is JObject response)
        {
            c.Status = response["status"]?.Value<int>() ?? 0; c.ResponseUrl = Limits.String(response["url"], 8192);
            string encoded = Limits.String(response["bytes"], 1400000); byte[] bytes = Convert.FromBase64String(encoded); Limits.Require(bytes.Length <= 1048576, "response_limit");
            Encoding encoding = Encoding.UTF8;
            string contentType = Limits.String(response["content_type"], 4096);
            if (System.Net.Http.Headers.MediaTypeHeaderValue.TryParse(contentType, out var media) && !string.IsNullOrWhiteSpace(media.CharSet)) encoding = Encoding.GetEncoding(media.CharSet.Trim('"'));
            using (var text = new StreamReader(new MemoryStream(bytes, false), encoding, true)) c.Response = text.ReadToEnd();
            if (response["headers"] is JObject headers) foreach (var field in headers.Properties())
            {
                Limits.Require(c.Headers.Count < 128 && field.Value is JArray, "response_headers");
                c.Headers.Add(field.Name, string.Join(",", ((JArray)field.Value).Select(value => Limits.String(value, 8192))));
            }
        }
        return c;
    }
}
