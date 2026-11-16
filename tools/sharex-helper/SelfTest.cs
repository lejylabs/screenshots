using System.Globalization;
using Newtonsoft.Json.Linq;

namespace Screenshots.ShareXRuntime;
internal static class SelfTest
{
    static int checks;
    static void Check(bool value) { checks++; if (!value) throw new Failure("self_test_assertion"); }
    static Profile ProfileFor(string template) => Profile.Read(new JObject { ["Version"] = "17.0.0", ["DestinationType"] = "FileUploader", ["RequestURL"] = "https://example.invalid/upload", ["Body"] = "Binary", ["URL"] = template }.ToString());
    static JObject Execute(string template, Context context) { var interpreter = new Interpreter(ProfileFor(template), context); interpreter.Compile(); return JObject.FromObject(interpreter.Response()); }
    static string Eval(string template, Context context) => Execute(template, context)["url"]!.Value<string>()!;
    static void Reject(Action action) { bool failed = false; try { action(); } catch { failed = true; } Check(failed); }
    static void RejectCode(Action action,string code) { try { action(); } catch(Failure error) { Check(error.Code==code);return; } throw new Failure("expected_preflight_refusal"); }
    static Interpreter InterpreterFor(Profile profile,Context context) {var value=new Interpreter(profile,context);value.Compile();return value;}
    static Profile BodyProfile(string mode,string data,string url="{responseurl}")=>Profile.Read(new JObject{["Version"]="17.0.0",["DestinationType"]="TextUploader",["RequestURL"]="https://example.invalid/upload",["Body"]=mode,["Data"]=data,["URL"]=url}.ToString());
    internal static int Run()
    {
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("en-US");
            Check(Limits.Object("{\"value\":\"2026-11-15T12:34:56\"}")["value"]!.Type == JTokenType.String);
            var context = new Context { Kind = "file", Filename = "my file.png", Input = "hello + world", ResponseUrl = "https://example.invalid/upload", Response = "plain-response" };
            Check(Eval("{filename}", context) == "my%20file.png"); Check(Eval("{INPUT}", context) == "hello%20%2B%20world");
            Check(Eval("{response}", context) == "plain-response"); Check(Eval("{responseurl}", context) == context.ResponseUrl);
            context.Headers["Location"] = "https://example.invalid/result"; Check(Eval("{header:location}", context) == context.Headers["Location"]);
            Check(Eval("{base64:a:b}", context) == "YTpi"); context.Answers["URL#1"] = "se64"; Check(Eval("{ba{select:se64|se64}:a:b}", context) == "YTpi"); Check(Eval("{random:same|same}", context) == "same");
            context.Answers["URL#0"] = "second"; Check(Eval("{select:first|second}", context) == "second"); Check(Eval("{inputbox}", context) == "second"); Check(Eval("{prompt:title|default}", context) == "second");
            var message = Execute("{outputbox:Title|hello}", context); Check(message["url"]!.Value<string>() == ""); Check(((JArray)message["events"]!).Count == 1); context.Events.Clear();
            Check(Eval(@"\{literal\}\|\\", context) == "{literal}|\\");
            context.Response = "{\"files\":[{\"id\":1,\"url\":\"a\"},{\"id\":2,\"url\":\"b\"}],\"nested\":{\"target\":\"c\"},\"url\":\"abc\"}";
            Check(Eval("{json:files[1].url}", context) == "b"); Check(Eval("{json:files[?(@.id == 2)].url}", context) == "b"); Check(Eval("{json:$..target}", context) == "c"); Check(Eval("{base64:{json:url}}", context) == "YWJj");
            Check(Eval("{json:{response}|files[0].url}", context) == "a"); Reject(() => Eval("{json:files[*].url}", context)); Reject(() => Eval("{json:files}", context));
            context.Response = "abab"; Check(Eval(@"{regex:(ab)\\1|1}", context) == "ab");
            context.Response = "id=123"; Check(Eval("{regex:(?<=id=)[0-9]+}", context) == "123");
            context.Response = "word"; Check(Eval("{regex:(?<name>[a-z]+)|name}", context) == "word");
            context.Response = "aaabbb"; Check(Eval("{regex:^(?'open'a)+(?'-open'b)+(?(open)(?!))$}", context) == "aaabbb");
            Check(Eval("{regex:explicit-input|(?<word>[a-z]+)|word}", context) == "explicit");
            context.Response = "<files><file id='a'><url>A</url></file><file id='b'><url>B</url></file></files>";
            Check(Eval("{xml:/files/file[@id='b']/url}", context) == "B"); Check(Eval("{xml:/files/file[2]/preceding-sibling::file/@id}", context) == "a"); Check(Eval("{xml:{response}|/files/file[1]/url}", context) == "A");
            Reject(() => Eval("{xml:count(/files/file)}", context));
            context.Response = "<!DOCTYPE x [<!ENTITY attack SYSTEM 'file:///must-not-read'>]><x>&attack;</x>"; Reject(() => Eval("{xml:/x}", context));
            context.Response = new string('a', 10000) + "!"; Reject(() => Eval("{regex:^(a+)+$}", context));
            Reject(() => Eval("{unknown}", context)); Reject(() => Eval("{base64}", context)); Reject(() => Eval("{random:one}", context)); Reject(() => Eval("{json:unclosed", context));
            var missing = new Context(); Reject(() => Eval("{inputbox}", missing)); Reject(() => Eval("{select:a|b}", missing));
            var names = new Context(); names.Names["now"] = "2026-11-15T14:07:03.123"; names.Names["counter"] = "40"; names.Names["width"] = "640"; names.Names["height"] = "480"; names.Names["title"] = " synthetic title "; names.Names["process"] = " synthetic process "; names.Names["user"] = "synthetic-user"; names.Names["domain"] = "synthetic-domain"; names.Names["computer"] = "synthetic-computer";
            Check(NameCodes.Expand("%y-%yy-%mo-%d %h:%mi:%s.%ms", names, true) == "2026-26-11-15 14:07:03.123"); Check(NameCodes.Expand("%h %pm", names, true) == "02 PM"); Check(NameCodes.Expand("%mon2 %w2 %wy", names, true) == "November Sunday 46");
            Check(NameCodes.Expand("%i{4}-%ia-%iA-%ix-%iX-%iAa-%iaA-%ib{16,3}", names, true) == "0041-15-15-29-29-f-F-029");
            Check(NameCodes.Expand("%width/%height/%t/%pn/%un/%uln/%cn", names, true) == "640/480/synthetic_title/synthetic_process/synthetic-user/synthetic-domain/synthetic-computer");
            Check(NameCodes.Expand("a%rf{/must-not-read}b", names, true) == "ab"); Check(NameCodes.Expand("%rn{8}", names, true).All(char.IsAsciiDigit)); Check(NameCodes.Expand("%rna{8}", names, true).Length == 8); Check(NameCodes.Expand("%guid", names, true).Length == 36); Check(NameCodes.Expand("%radjective %ranimal %remoji{2}", names, true).Length > 5);
            Reject(() => NameCodes.Expand("%un", new Context(), true)); Reject(() => NameCodes.Expand("%rn{999999999}", names, true));
            string modern = "{\"Version\":\"17.0.0\",\"DestinationType\":\"TextUploader\",\"RequestURL\":\"https://example.invalid/upload\",\"Body\":\"JSON\",\"Data\":\"{\\\"text\\\":\\\"{input}\\\"}\"}";
            var p = Profile.Read(modern); var text = new Context { Kind = "text", Input = "a\"b\n" }; var prepare = new Interpreter(p, text); prepare.Compile(); var data = JObject.FromObject(prepare.Prepare())["data"]!.Value<byte[]>()!; Check(JObject.Parse(System.Text.Encoding.UTF8.GetString(data))["text"]!.Value<string>() == text.Input);
            p.AdmitKind("text"); Reject(() => p.AdmitKind("file"));
            var old = Profile.Read("{\"Version\":\"13.7.1\",\"DestinationType\":4,\"RequestURL\":\"example.invalid/upload?a=$filename$\",\"Body\":5,\"URL\":\"$json:url$\"}"); Check(old.Link == "{json:url}"); Check(old.Parameters["a"] == "{filename}");
            Reject(() => Profile.Read("{\"Version\":\"12.3.1.0\",\"RequestURL\":\"https://example.invalid/\"}"));
            Reject(() => Profile.Read("{\"Version\":\"12.3.1\",\"RequestURL\":\"https://example.invalid/\"}")); Reject(() => Profile.Read(modern.Replace("\"Data\":", "\"unknown\":1,\"Data\":")));
            foreach(string keyName in new[]{"auth","key","X-Auth","xAuth","access_key","AccessKey","access_key_id","aws_access_key_id","secret_access_key"})Check(Interpreter.Sensitive(keyName));
            foreach(string publicName in new[]{"author","monkey","keyboard","filename","index"})Check(!Interpreter.Sensitive(publicName));
            foreach(string field in new[]{"URL","ThumbnailURL","DeletionURL","ErrorMessage"})
            {
                foreach(string function in new[]{"{inputbox:Title|default}","{prompt:Title}","{select:first|second}"})
                {
                    var config=JObject.Parse("{\"Version\":\"17.0.0\",\"DestinationType\":\"FileUploader\",\"RequestURL\":\"https://example.invalid/upload\",\"Body\":\"Binary\"}");config[field]=function;
                    RejectCode(()=>InterpreterFor(Profile.Read(config.ToString()),new Context{Kind="file"}).Prepare(),"answer_required");
                }
            }
            var dynamicProfile=ProfileFor("{select:{json:empty}|{json:first}|{json:second}}");
            RejectCode(()=>InterpreterFor(dynamicProfile,new Context{Kind="file"}).Prepare(),"selection_index_required");
            var indexed=new Context{Kind="file",Response="{\"empty\":\"\",\"first\":\"A\",\"second\":\"B\"}"};indexed.AnswerIndices["URL#0"]=1;
            var dynamicInterpreter=InterpreterFor(dynamicProfile,indexed);_=dynamicInterpreter.Prepare();Check(JObject.FromObject(dynamicInterpreter.Response())["url"]!.Value<string>()=="B");
            var computedProfile=ProfileFor("{ba{json:suffix}:value}");
            RejectCode(()=>InterpreterFor(computedProfile,new Context{Kind="file"}).Prepare(),"function_binding_required");
            var bound=new Context{Kind="file",Response="{\"suffix\":\"se64\"}"};bound.FunctionNames["URL#0"]="base64";var boundInterpreter=InterpreterFor(computedProfile,bound);_=boundInterpreter.Prepare();Check(JObject.FromObject(boundInterpreter.Response())["url"]!.Value<string>()=="dmFsdWU=");
            bound.Response="{\"suffix\":\"other\"}";RejectCode(()=>boundInterpreter.Response(),"function_binding_mismatch");
            var frozenContext=new Context{Kind="file"};frozenContext.Answers["URL#1"]="se64";_=InterpreterFor(ProfileFor("{ba{select:se64|se64}:a:b}"),frozenContext).Prepare();Check(!frozenContext.FunctionNames.Any());
            var boundPrompt=new Context{Kind="file"};boundPrompt.FunctionNames["URL#0"]="inputbox";RejectCode(()=>InterpreterFor(ProfileFor("{{json:function}:Title}"),boundPrompt).Prepare(),"answer_required");
            var contracts=JObject.FromObject(InterpreterFor(dynamicProfile,new Context{Kind="file"}).Inspect())["answer_contracts"]!.Children<JObject>().ToArray();Check(contracts.Any(value=>value["id"]!.Value<string>()=="URL#0"&&value["answer_kind"]!.Value<string>()=="index"&&value["stage"]!.Value<string>()=="response"));
            foreach(string encodedName in new[]{"a%75th","%6bey","access%5fkey","X%2dAuth"})Check(Interpreter.Sensitive(Interpreter.DeliveredQueryName(encodedName)));
            Check(Interpreter.DeliveredQueryName("public%2bname")=="public+name");Check(Interpreter.DeliveredQueryName("public+name")=="public name");
            var encodedConfig=ProfileFor("{responseurl}");encodedConfig.Parameters.Add("a%75th","synthetic+private");var encodedInterpreter=InterpreterFor(encodedConfig,new Context{Kind="file"});var encodedRequest=JObject.FromObject(encodedInterpreter.Prepare());Check(encodedRequest["url"]!.Value<string>()=="https://example.invalid/upload?a%75th=synthetic%2bprivate");Check(encodedRequest["secrets"]!.Values<string>().Contains("synthetic+private"));
            var inspectionProfile=ProfileFor("{inputbox:Caption|opaque+value}");inspectionProfile.Headers.Add("Authorization","Bearer opaque+value");inspectionProfile.Headers.Add("Cookie","sid=pad==; other=secondopaque");inspectionProfile.Parameters.Add("a%75th","paramopaque");inspectionProfile.Thumbnail="{inputbox:opaque%2bvalue|public fallback}";inspectionProfile.Deletion="{select:public-first|pad==|public-last}";inspectionProfile.Error="{inputbox:Other|param%6fpaque}";
            var inspectionContext=new Context{Kind="file"};var inspectionInterpreter=InterpreterFor(inspectionProfile,inspectionContext);
            var inspection=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(inspectionInterpreter.Inspect()));var descriptions=inspection["answers"]!.Children<JObject>().ToArray();
            Check(descriptions.Single(value=>value["id"]!.Value<string>()=="URL#0")["default_text"]!.Value<string>()=="[redacted]");Check(descriptions.Single(value=>value["id"]!.Value<string>()=="URL#0")["title"]!.Value<string>()=="Caption");
            var thumb=descriptions.Single(value=>value["id"]!.Value<string>()=="ThumbnailURL#0");Check(thumb["title"]!.Value<string>()=="[redacted]");Check(thumb["default_text"]!.Value<string>()=="public fallback");
            var choice=descriptions.Single(value=>value["id"]!.Value<string>()=="DeletionURL#0")["choices"]!.Values<string>().ToArray();Check(choice.SequenceEqual(new[]{"public-first","[redacted]","public-last"}));
            Check(descriptions.Single(value=>value["id"]!.Value<string>()=="ErrorMessage#0")["default_text"]!.Value<string>()=="[redacted]");Check(inspection["answer_contracts"]!.Children<JObject>().Any(value=>value["id"]!.Value<string>()=="URL#0"));
            inspectionContext.Answers["URL#0"]="https://example.invalid/chosen";inspectionContext.Answers["ThumbnailURL#0"]="https://example.invalid/thumb";inspectionContext.Answers["DeletionURL#0"]="public-first";inspectionContext.Answers["ErrorMessage#0"]="public failure";
            _=inspectionInterpreter.Prepare();Check(JObject.FromObject(inspectionInterpreter.Response())["url"]!.Value<string>()=="https://example.invalid/chosen");
            foreach(var sample in new[]{("XML","<r><password>opaque-xml</password><caption>public</caption></r>","opaque-xml"),("XML","<r password='opaque-attribute'><note>public</note></r>","opaque-attribute"),("JSON","{\"credentials\":{\"password\":\"opaque-json\"},\"caption\":\"public\"}","opaque-json"),("JSON","password=opaque%2Bform&caption=public","opaque+form")})
            {
                var contextBody=new Context{Kind="text"};var bodyInterpreter=InterpreterFor(BodyProfile(sample.Item1,sample.Item2),contextBody);var preparedBody=JObject.FromObject(bodyInterpreter.Prepare());Check(preparedBody["secrets"]!.Values<string>().Contains(sample.Item3));
            }
            foreach(string prefix in new[]{"{caption}","[caption]","<caption>","{\"public\":true}"})
            {
                string rawBody=prefix+"=public&password=opaque%2Bprefix";
                var rawProfile=BodyProfile("JSON",rawBody);var rawInterpreter=InterpreterFor(rawProfile,new Context{Kind="text"});
                Check(JObject.FromObject(rawInterpreter.Prepare())["secrets"]!.Values<string>().Contains("opaque+prefix"));
                rawProfile.Link="{inputbox:Caption|opaque+prefix}";
                var inspected=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(InterpreterFor(rawProfile,new Context{Kind="text"}).Inspect()));
                Check(inspected["answers"]!.Children<JObject>().Single(value=>value["id"]!.Value<string>()=="URL#0")["default_text"]!.Value<string>()=="[redacted]");
            }
            foreach(string mode in new[]{"JSON","XML"})
            {
                string source=mode=="JSON"?"{\"password\":\"opaque%y\",\"caption\":\"public\"}":"<r password='opaque%y'><caption>public</caption></r>";
                var frozenBody=new Context{Kind="text"};frozenBody.Names["now"]="2020-01-02T03:04:05";var profileBody=BodyProfile(mode,source,"{inputbox:Caption|opaque2020}");var interpreterBody=InterpreterFor(profileBody,frozenBody);
                var inspectedBody=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(interpreterBody.Inspect()));Check(inspectedBody["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");Check(inspectedBody["answers"]![0]!["title"]!.Value<string>()=="Caption");Check(profileBody.Data==source);
                frozenBody.Answers["URL#0"]="https://example.invalid/chosen";var preparedBody=JObject.FromObject(interpreterBody.Prepare());Check(preparedBody["secrets"]!.Values<string>().Contains("opaque2020"));
            }
            var formLiteral=InterpreterFor(BodyProfile("JSON","password=opaque%25y","{inputbox:Caption|opaque%y}"),new Context{Kind="text"});var formMetadata=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(formLiteral.Inspect()));Check(formMetadata["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
            var ignoredData=BodyProfile("FormURLEncoded","password=unused-private");ignoredData.Arguments.Add("caption","public");var ignored=JObject.FromObject(InterpreterFor(ignoredData,new Context{Kind="text"}).Prepare());Check(!ignored["secrets"]!.Values<string>().Contains("unused-private"));
            var innocent=JObject.FromObject(InterpreterFor(BodyProfile("JSON","{\"caption\":\"password=public\"}"),new Context{Kind="text"}).Prepare());Check(!innocent["secrets"]!.Any());
            RejectCode(()=>InterpreterFor(BodyProfile("XML","<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///must-not-read'>]><r>&x;</r>"),new Context{Kind="text"}).Prepare(),"body_dtd_forbidden");
            var cookieBody=BodyProfile("JSON","{\"Cookie\":\"sid=opaque-body-cookie==\"}","{inputbox:Caption|opaque-body-cookie==}");var cookieBodyInterpreter=InterpreterFor(cookieBody,new Context{Kind="text"});var cookieBodyMetadata=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(cookieBodyInterpreter.Inspect()));Check(cookieBodyMetadata["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
            var unknownNumber=BodyProfile("JSON","{\"counter\":%rn{4},\"password\":\"known-with-random-sibling\"}","{inputbox:Caption|known-with-random-sibling}");Check(JObject.Parse(System.Text.Json.JsonSerializer.Serialize(InterpreterFor(unknownNumber,new Context{Kind="text"}).Inspect()))["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
            Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new { passed = true, checks })); return 0;
        }
        catch (Exception error) { Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new { passed = false, checks, code = error is Failure failure ? failure.Code : error.GetType().Name })); return 1; }
    }
}
