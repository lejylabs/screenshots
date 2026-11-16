using System.Security;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using System.Web;
using System.Xml;
using System.Xml.XPath;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace Screenshots.ShareXRuntime;

internal abstract record Node;
internal sealed record Literal(string Text) : Node;
internal sealed record Call(List<Node> Name, string Id, List<List<Node>> Arguments) : Node;
internal sealed class Budget { internal int Calls; internal long Materialized; internal List<object> Inspection = []; }
internal sealed class Syntax(string source, string field, Budget budget)
{
    int offset, ordinal;
    internal List<Node> Parse() { var result = Sequence(0, false); Limits.Require(offset == source.Length, "template_delimiter"); return result; }
    List<Node> Sequence(int depth, bool argument, bool nameMode = false)
    {
        Limits.Require(depth <= 16, "template_depth"); var result = new List<Node>(); var text = new StringBuilder();
        void Flush() { if (text.Length > 0) { result.Add(new Literal(text.ToString())); text.Clear(); } }
        while (offset < source.Length)
        {
            char c = source[offset];
            if (c == '\\') { offset++; if (offset < source.Length) text.Append(source[offset++]); continue; }
            if (c is '}' or '|' || nameMode && c == ':') { Limits.Require(argument, "template_delimiter"); break; }
            if (c != '{') { text.Append(c); offset++; continue; }
            Flush(); offset++; Limits.Require(++budget.Calls <= 128, "template_calls");
            string id = field + "#" + ordinal++;
            var name = Sequence(depth + 1, true, true);
            string? function = name.All(node => node is Literal) ? string.Concat(name.Cast<Literal>().Select(node => node.Text)).ToLowerInvariant() : null;
            if (function != null) Limits.Require(Functions.ContainsKey(function), "function_name");
            var args = new List<List<Node>>();
            if (offset < source.Length && source[offset] == ':')
            {
                do { offset++; args.Add(Sequence(depth + 1, true)); } while (offset < source.Length && source[offset] == '|');
            }
            Limits.Require(offset < source.Length && source[offset] == '}', "template_unclosed"); offset++;
            if (function != null) Limits.Require(args.Count >= Functions[function], "function_arity");
            if (function is "inputbox" or "prompt" or "select")
            {
                string? Static(List<Node> nodes) => nodes.All(node => node is Literal) ? string.Concat(nodes.Cast<Literal>().Select(node => node.Text)) : null;
                string? title = function == "select" ? "Select" : args.Count == 0 ? "Input" : Static(args[0]);
                bool redacted = Interpreter.SensitiveField(field) || title != null && Interpreter.Sensitive(title);
                string? defaultText = args.Count > 1 ? Static(args[1]) : "";
                string[]? choices = function == "select" && args.All(arg => Static(arg) != null) ? args.Select(arg => Static(arg)!).Where(value => value.Length > 0).ToArray() : null;
                budget.Inspection.Add(new { id, function, parameters = args.Count, stage = field is "URL" or "ThumbnailURL" or "DeletionURL" or "ErrorMessage" ? "response" : "request", answer_kind = function == "select" ? choices == null ? "index_unless_frozen_context" : "value_or_index" : "value", title = redacted ? null : title, default_text = redacted ? null : defaultText, choices = redacted ? null : choices, redacted, requires_runtime_context = title == null || function == "select" && choices == null });
            }
            if (function == null) budget.Inspection.Add(new { id, function = "computed", parameters = args.Count, stage = field is "URL" or "ThumbnailURL" or "DeletionURL" or "ErrorMessage" ? "response" : "request", answer_kind = "function_name_if_not_frozen_context" });
            result.Add(new Call(name, id, args));
        }
        Flush(); return result;
    }
    internal static readonly Dictionary<string, int> Functions = new(StringComparer.Ordinal) { ["filename"] = 0, ["input"] = 0, ["response"] = 0, ["responseurl"] = 0, ["header"] = 1, ["base64"] = 1, ["random"] = 2, ["select"] = 1, ["inputbox"] = 0, ["prompt"] = 0, ["outputbox"] = 1, ["json"] = 1, ["regex"] = 1, ["xml"] = 1 };
}

internal sealed class Interpreter(Profile profile, Context context)
{
    readonly Budget budget = new();
    readonly Dictionary<string, List<Node>> trees = new(StringComparer.Ordinal);
    readonly List<string> secrets = [];
    const int Intermediate = 2 * 1024 * 1024;
    internal void Compile()
    {
        void Add(string field, string value, bool names) { trees.Add(field, new Syntax(names ? NameCodes.Expand(value, context, true) : value, field, budget).Parse()); }
        Add("RequestURL", profile.Url, false);
        foreach (var pair in profile.Parameters) Add("Parameters." + pair.Key, pair.Value, true);
        foreach (var pair in profile.Headers) Add("Headers." + pair.Key, pair.Value, true);
        foreach (var pair in profile.Arguments) Add("Arguments." + pair.Key, pair.Value, true);
        Add("URL", profile.Link, false); Add("ThumbnailURL", profile.Thumbnail, false); Add("DeletionURL", profile.Deletion, false); Add("ErrorMessage", profile.Error, false);
    }
    static string? LiteralValue(List<Node> tree) => tree.All(node => node is Literal) ? string.Concat(tree.Cast<Literal>().Select(node => node.Text)) : null;
    string? StaticValue(List<Node> tree, bool url, int depth = 0)
    {
        Limits.Require(depth <= 16, "template_depth"); var output = new StringBuilder();
        foreach (var node in tree)
        {
            string? value;
            if (node is Literal literal) value = literal.Text;
            else
            {
                var call = (Call)node;
                string? name = StaticValue(call.Name, url, depth + 1)?.ToLowerInvariant();
                if (name == null) return null;
                if (name is "response" or "responseurl" or "header") return null;
                if (name is "inputbox" or "prompt") value = context.Answers.GetValueOrDefault(call.Id);
                else if (name == "outputbox") value = "";
                else if (name == "filename") value = url ? Uri.EscapeDataString(context.Filename) : context.Filename;
                else if (name == "input") value = url ? Uri.EscapeDataString(context.Input) : context.Input;
                else
                {
                    string?[] args = call.Arguments.Select(arg => StaticValue(arg, url, depth + 1)).ToArray();
                    if (args.Any(value => value == null)) return null;
                    string[] known = args.Select(value => value!).ToArray();
                    if (name == "random") value = known.Length >= 2 && known.All(value => value == known[0]) ? known[0] : null;
                    else if (name == "select") { string[] choices = known.Where(value => value.Length > 0).ToArray(); value = choices.Length == 0 ? "" : context.AnswerIndices.ContainsKey(call.Id) || context.Answers.ContainsKey(call.Id) ? context.Answer(call.Id, "Select", choices) : null; }
                    else if ((name is "json" or "xml") && known.Length <= 1 || name == "regex" && known.Length <= 2) return null;
                    else if (name is "base64" or "json" or "regex" or "xml") value = Invoke(name, call, known, url);
                    else return null;
                }
            }
            if (value == null) return null;
            Limits.Require((long)output.Length + value.Length <= Intermediate, "expansion_limit"); output.Append(value);
        }
        string result=Limits.Bounded(output.ToString(), Intermediate); budget.Materialized += Encoding.UTF8.GetByteCount(result); Limits.Require(budget.Materialized <= 16 * 1024 * 1024, "materialization_limit"); return result;
    }
    internal void PreflightAnswers()
    {
        void Walk(List<Node> nodes, bool url)
        {
            foreach (var node in nodes) if (node is Call call)
            {
                Walk(call.Name, url); foreach (var arg in call.Arguments) Walk(arg, url);
                string? name = StaticValue(call.Name, url)?.ToLowerInvariant();
                if (name == null) { Limits.Require(context.FunctionNames.TryGetValue(call.Id, out name), "function_binding_required"); name = name!.ToLowerInvariant(); }
                if (context.FunctionNames.TryGetValue(call.Id, out string? binding)) Limits.Require(name.Equals(binding, StringComparison.OrdinalIgnoreCase), "function_binding_mismatch");
                Limits.Require(Syntax.Functions.TryGetValue(name, out int minimum) && call.Arguments.Count >= minimum, "function_name_or_arity");
                if (name is "inputbox" or "prompt") Limits.Require(context.Answers.ContainsKey(call.Id) && !context.AnswerIndices.ContainsKey(call.Id), "answer_required");
                else if (name == "select")
                {
                    string?[] known = call.Arguments.Select(arg => StaticValue(arg, url)).ToArray();
                    if (known.Any(value => value == null)) Limits.Require(context.AnswerIndices.ContainsKey(call.Id) && !context.Answers.ContainsKey(call.Id), "selection_index_required");
                    else { string[] choices = known.Select(value => value!).Where(value => value.Length > 0).ToArray(); if (choices.Length > 0) _ = context.Answer(call.Id, "Select", choices); }
                }
            }
        }
        foreach (var pair in trees) Walk(pair.Value, pair.Key is "RequestURL" or "URL" or "ThumbnailURL" or "DeletionURL" or "ErrorMessage");
    }
    void AddKnownAuthentication(string name, string value)
    {
        if(value.Length==0)return;secrets.Add(value);
        int space=value.IndexOf(' ');if(space>0&&(value[..space].Equals("Bearer",StringComparison.OrdinalIgnoreCase)||value[..space].Equals("Basic",StringComparison.OrdinalIgnoreCase))&&space+1<value.Length)secrets.Add(value[(space+1)..]);
        if(name.Equals("Cookie",StringComparison.OrdinalIgnoreCase)||name.Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase))
        {
            int count=0;foreach(string pair in value.Split(';')){if(name.Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase)&&count>0)break;Limits.Require(count++<128,"cookie_pair_limit");int equals=pair.IndexOf('=');if(equals>0){string token=pair[(equals+1)..].Trim();if(token.Length>0){secrets.Add(token);if(token.Length>=2&&token[0]=='"'&&token[^1]=='"')secrets.Add(token[1..^1]);}}}
        }
    }
    void CollectInspectionAuthentication()
    {
        foreach(var pair in trees)if(pair.Key.StartsWith("Headers.",StringComparison.Ordinal)||pair.Key.StartsWith("Parameters.",StringComparison.Ordinal)||pair.Key.StartsWith("Arguments.",StringComparison.Ordinal))
        {
            if(SensitiveField(pair.Key)){string? value=StaticValue(pair.Value,false);if(value!=null)AddKnownAuthentication(pair.Key[(pair.Key.IndexOf('.')+1)..],value);}
        }
        if(profile.Body is "JSON" or "XML")BodyAuthentication.Scan(Data(true),context.UnknownExpansionMarker,AddKnownAuthentication);
        Limits.Require(secrets.Sum(value=>(long)Encoding.UTF8.GetByteCount(value))<=256*1024,"inspection_auth_limit");
    }
    static string InspectionProjection(string value,bool form)
    {
        byte[] source=Encoding.UTF8.GetBytes(value);var output=new List<byte>(source.Length);int Hex(byte b)=>b is >= (byte)'0' and <= (byte)'9'?b-'0':b is >= (byte)'a' and <= (byte)'f'?b-'a'+10:b is >= (byte)'A' and <= (byte)'F'?b-'A'+10:-1;
        for(int i=0;i<source.Length;i++){if(source[i]=='%'&&i+2<source.Length&&Hex(source[i+1])>=0&&Hex(source[i+2])>=0){output.Add((byte)(Hex(source[i+1])*16+Hex(source[i+2])));i+=2;}else output.Add(form&&source[i]=='+'?(byte)' ':source[i]);}return Encoding.UTF8.GetString(output.ToArray());
    }
    System.Text.Json.JsonElement SanitizedInspectionAnswers()
    {
        CollectInspectionAuthentication();var answers=JArray.FromObject(budget.Inspection);
        bool Contains(string value){if(secrets.Any(secret=>secret.Length>0&&value.Contains(secret,StringComparison.Ordinal)))return true;string uri=InspectionProjection(value,false),form=InspectionProjection(value,true);return secrets.Any(secret=>secret.Length>0&&(uri.Contains(secret,StringComparison.Ordinal)||form.Contains(secret,StringComparison.Ordinal)));}
        foreach(JObject answer in answers.Children<JObject>())
        {
            bool redacted=answer["redacted"]?.Value<bool>()??false;
            foreach(string key in new[]{"title","default_text"})if(answer[key]?.Type==JTokenType.String&&Contains(answer[key]!.Value<string>()!)){answer[key]="[redacted]";redacted=true;}
            if(answer["choices"] is JArray choices)for(int i=0;i<choices.Count;i++)if(choices[i].Type==JTokenType.String&&Contains(choices[i].Value<string>()!)){choices[i]="[redacted]";redacted=true;}
            if(answer.Property("redacted")!=null)answer["redacted"]=redacted;
        }
        using var document=System.Text.Json.JsonDocument.Parse(answers.ToString(Newtonsoft.Json.Formatting.None));return document.RootElement.Clone();
    }
    internal object Inspect()
    {
        var contracts = new List<object>();
        void Walk(List<Node> nodes, string field, bool url)
        {
            foreach (var node in nodes) if (node is Call call)
            {
                Walk(call.Name, field, url); foreach (var arg in call.Arguments) Walk(arg, field, url);
                string? name=StaticValue(call.Name,url)?.ToLowerInvariant();
                bool computed=LiteralValue(call.Name)==null;
                if(name==null && context.FunctionNames.TryGetValue(call.Id,out string? bound))name=bound.ToLowerInvariant();
                string? answerKind = name is "inputbox" or "prompt" ? "value" : name=="select" ? call.Arguments.Any(arg=>StaticValue(arg,url)==null) ? "index" : "value_or_index" : null;
                if(answerKind!=null||computed)contracts.Add(new{id=call.Id,stage=field is "URL" or "ThumbnailURL" or "DeletionURL" or "ErrorMessage"?"response":"request",function=name!=null&&Syntax.Functions.ContainsKey(name)?name:null,answer_kind=answerKind,function_name_binding_required=computed&&name==null});
            }
        }
        foreach(var pair in trees)Walk(pair.Value,pair.Key,pair.Key is "RequestURL" or "URL" or "ThumbnailURL" or "DeletionURL" or "ErrorMessage");
        return new { calls = budget.Calls, answers = SanitizedInspectionAnswers(), answer_contracts=contracts, functions = Syntax.Functions.Keys.ToArray() };
    }
    string Evaluate(List<Node> tree, bool url, int limit)
    {
        var text = new StringBuilder();
        foreach (Node node in tree)
        {
            string value;
            if (node is Literal literal) value = literal.Text;
            else { var call = (Call)node; string function = Evaluate(call.Name, url, 128).ToLowerInvariant(); string[] args = call.Arguments.Select(arg => Evaluate(arg, url, Intermediate)).ToArray(); value = Invoke(function, call, args, url); }
            Limits.Require((long)text.Length + value.Length <= limit, "expansion_limit"); text.Append(value);
        }
        string result = Limits.Bounded(text.ToString(), limit); budget.Materialized += Encoding.UTF8.GetByteCount(result);
        Limits.Require(budget.Materialized <= 16 * 1024 * 1024, "materialization_limit"); return result;
    }
    string Field(string field, bool url = false, int limit = Limits.Text) => Evaluate(trees[field], url, limit);
    string Invoke(string name, Call call, string[] args, bool url)
    {
        if (context.FunctionNames.TryGetValue(call.Id, out string? binding)) Limits.Require(name.Equals(binding, StringComparison.OrdinalIgnoreCase), "function_binding_mismatch");
        string first = args.FirstOrDefault() ?? "";
        Limits.Require(Syntax.Functions.TryGetValue(name, out int minimum) && args.Length >= minimum, "function_name_or_arity");
        switch (name)
        {
            case "filename": return url ? Uri.EscapeDataString(context.Filename) : context.Filename;
            case "input": return url ? Uri.EscapeDataString(context.Input) : context.Input;
            case "response": return context.Response;
            case "responseurl": return context.ResponseUrl;
            case "header": return context.Headers.GetValueOrDefault(first) ?? "";
            case "base64": return Convert.ToBase64String(Encoding.UTF8.GetBytes(first));
            case "random": return args[RandomNumberGenerator.GetInt32(args.Length)];
            case "select": var choices = args.Where(value => value.Length > 0).ToArray(); return choices.Length == 0 ? "" : context.Answer(call.Id, "Select", choices);
            case "prompt": case "inputbox": string title = args.Length == 0 ? "Input" : first; string answer = context.Answer(call.Id, title); if (answer.Length > 0 && (Sensitive(title) || SensitiveField(call.Id[..call.Id.LastIndexOf('#')]))) secrets.Add(answer); return answer;
            case "outputbox": string message = args.Length > 1 ? args[1] : first; if (message.Length > 0) { Limits.Require(context.Events.Count < 16, "output_event_limit"); context.Events.Add(new { title = Limits.Bounded(args.Length > 1 && first.Length > 0 ? first : "Output", 256), text = Limits.Bounded(message, 4096) }); } return "";
            case "json":
            {
                string input = args.Length > 1 ? first : context.Response, path = args.Length > 1 ? args[1] : first;
                if (input.Length == 0 || path.Length == 0) return "";
                Limits.Bounded(input, Intermediate); Limits.Bounded(path, 8192);
                if (!path.StartsWith("$.", StringComparison.Ordinal)) path = "$." + path;
                using var reader = new JsonTextReader(new StringReader(input)) { MaxDepth = 64 };
                JToken document = JToken.ReadFrom(reader); while (reader.Read()) { }
                var selected = document.SelectToken(path, new JsonSelectSettings { RegexMatchTimeout = TimeSpan.FromMilliseconds(100) });
                return (string?)selected ?? "";
            }
            case "regex":
            {
                string input = args.Length > 2 ? first : context.Response, pattern = args.Length > 2 ? args[1] : first, group = args.Length > 2 ? args[2] : args.Length > 1 ? args[1] : "";
                if (input.Length == 0 || pattern.Length == 0) return "";
                Limits.Bounded(input, Intermediate); Limits.Bounded(pattern, 8192);
                Match match = Regex.Match(input, pattern, RegexOptions.None, TimeSpan.FromMilliseconds(100));
                if (!match.Success) return "";
                return group.Length == 0 ? match.Value : int.TryParse(group, out int index) ? match.Groups[index].Value : match.Groups[group].Value;
            }
            case "xml":
            {
                string input = args.Length > 1 ? first : context.Response, path = args.Length > 1 ? args[1] : first;
                if (input.Length == 0 || path.Length == 0) return "";
                Limits.Bounded(input, Intermediate); Limits.Bounded(path, 8192);
                var settings = new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null, MaxCharactersInDocument = Intermediate, MaxCharactersFromEntities = 0 };
                using (var depth = XmlReader.Create(new StringReader(input), settings)) while (depth.Read()) Limits.Require(depth.Depth <= 64, "xml_depth");
                using var reader = XmlReader.Create(new StringReader(input), settings);
                var document = new XPathDocument(reader);
                return document.CreateNavigator().SelectSingleNode(path)?.Value ?? "";
            }
            default: throw new Failure("function_name");
        }
    }
    internal object Prepare()
    {
        profile.AdmitKind(context.Kind);
        PreflightAnswers();
        string url = Field("RequestURL", true, 8192);
        if (!url.StartsWith("http://", StringComparison.OrdinalIgnoreCase) && !url.StartsWith("https://", StringComparison.OrdinalIgnoreCase)) url = "https://" + url;
        var headers = Map(profile.Headers, "Headers."); var arguments = Map(profile.Arguments, "Arguments."); var parameters = Map(profile.Parameters, "Parameters.");
        var query = parameters.Select(pair => pair.Value.Length == 0 ? pair.Key : pair.Key + "=" + HttpUtility.UrlEncode(pair.Value));
        string suffix = string.Join("&", query);
        if (suffix.Length > 0) url += (url.Contains('?') ? "&" : "?") + suffix;
        Limits.Bounded(url, 8192);
        foreach (var map in new[] { headers, arguments }) foreach (var pair in map) if (Sensitive(pair.Key) && pair.Value.Length > 0) secrets.Add(pair.Value);
        foreach (var pair in parameters) if (Sensitive(DeliveredQueryName(pair.Key)) && pair.Value.Length > 0) secrets.Add(pair.Value);
        bool attach = profile.Body == "MultipartFormData" && (context.Kind is "image" or "file" || context.Kind == "text" && profile.Field.Length > 0);
        byte[] data = profile.Body is "JSON" or "XML" ? Encoding.UTF8.GetBytes(Data()) : [];
        if (profile.Body == "FormURLEncoded") { using var form = new FormUrlEncodedContent(arguments); data = form.ReadAsByteArrayAsync().GetAwaiter().GetResult(); }
        Limits.Require(data.Length <= Limits.Body, "body_limit");
        if(data.Length>0)BodyAuthentication.Scan(Encoding.UTF8.GetString(data),null,AddKnownAuthentication);
        else if(profile.Body=="Binary"&&context.Kind=="text")BodyAuthentication.Scan(context.Input,null,AddKnownAuthentication);

        return new { url, method = profile.Method, body = profile.Body, file_field = profile.Field, attach_file = attach, headers, arguments, data, events = context.Events, secrets };
    }
    Dictionary<string, string> Map(Dictionary<string, string> source, string prefix)
    {
        var output = new Dictionary<string, string>(StringComparer.Ordinal); long bytes = 0;
        foreach (var pair in source) { string value = Field(prefix + pair.Key); bytes += Encoding.UTF8.GetByteCount(pair.Key) + Encoding.UTF8.GetByteCount(value); Limits.Require(bytes <= 65536, "map_expansion_limit"); output.Add(pair.Key, value); }
        return output;
    }
    string? resolvedData;
    string Data(bool frozenOnly = false)
    {
        if(!frozenOnly&&resolvedData!=null)return resolvedData;
        string source = NameCodes.Expand(profile.Data, context, false, frozenOnly);
        if(frozenOnly){source=source.Replace("{input}",context.UnknownExpansionMarker,StringComparison.OrdinalIgnoreCase).Replace("{filename}",context.UnknownExpansionMarker,StringComparison.OrdinalIgnoreCase);if(source.TrimStart().StartsWith('<'))source=source.Replace(context.UnknownExpansionMarker,"x"+context.UnknownExpansionMarker,StringComparison.Ordinal);return source;}
        string Encode(string text) => profile.Body == "JSON" ? JsonConvert.ToString(text)[1..^1] : SecurityElement.Escape(text) ?? "";
        string input = Encode(context.Input), filename = Encode(context.Filename); var result = new StringBuilder();
        for (int i = 0; i < source.Length;)
        {
            string? replacement = null; int consumed = 1;
            if (source.AsSpan(i).StartsWith("{input}", StringComparison.OrdinalIgnoreCase)) { replacement = input; consumed = 7; }
            else if (source.AsSpan(i).StartsWith("{filename}", StringComparison.OrdinalIgnoreCase)) { replacement = filename; consumed = 10; }
            replacement ??= source[i].ToString(); Limits.Require((long)result.Length + replacement.Length <= Limits.Body, "body_limit"); result.Append(replacement); i += consumed;
        }
        return resolvedData=Limits.Bounded(result.ToString(), Limits.Body);
    }
    internal object Response()
    {
        bool success = context.Status is >= 200 and <= 299;
        string link = "", thumbnail = "", deletion = "", error = "";
        if (success)
        {
            link = profile.Link.Length == 0 ? context.Response : Field("URL", true);
            thumbnail = Field("ThumbnailURL", true); deletion = Field("DeletionURL", true);
        }
        else if (profile.Error.Length > 0) error = Field("ErrorMessage", true, 4096);
        return new { success, status = context.Status, url = Limits.Bounded(link, 2048), thumbnail = Limits.Bounded(thumbnail, 2048), deletion = Limits.Bounded(deletion, 2048), error_message = error, events = context.Events, secrets };
    }
    internal static string DeliveredQueryName(string value)
    {
        Limits.Bounded(value,16384);byte[] source=Encoding.UTF8.GetBytes(value);var result=new List<byte>(source.Length);
        int Hex(byte b)=>b is >= (byte)'0' and <= (byte)'9'?b-'0':b is >= (byte)'a' and <= (byte)'f'?b-'a'+10:b is >= (byte)'A' and <= (byte)'F'?b-'A'+10:-1;
        for(int i=0;i<source.Length;i++){if(source[i]=='%'&&i+2<source.Length&&Hex(source[i+1])>=0&&Hex(source[i+2])>=0){result.Add((byte)(Hex(source[i+1])*16+Hex(source[i+2])));i+=2;}else result.Add(source[i]=='+'?(byte)' ':source[i]);}
        return new UTF8Encoding(false,true).GetString(result.ToArray());
    }
    internal static bool SensitiveField(string field)
    {
        int dot=field.IndexOf('.');string name=dot<0?field:field[(dot+1)..];
        return Sensitive(field.StartsWith("Parameters.",StringComparison.Ordinal)?DeliveredQueryName(name):name);
    }
    internal static bool Sensitive(string key) {
        string normalized = key.ToLowerInvariant().Replace("-", "").Replace("_", "").Replace(" ", "");
        string[] known = ["password","passwd","secret","token","accesstoken","refreshtoken","authtoken","apikey","authorization","auth","clientsecret","key","accesskey","accesskeyid","secretaccesskey","credential","credentials","session","sessionid","xauth","xkey"];
        return known.Contains(normalized, StringComparer.Ordinal) || Regex.Split(key.ToLowerInvariant(), "[^a-z0-9]+").Any(part => part is "auth" or "key" or "credential" or "credentials") || new[] { "authorization", "cookie", "apikey", "token", "password", "passwd", "secret", "signature" }.Any(normalized.Contains);
    }
}
