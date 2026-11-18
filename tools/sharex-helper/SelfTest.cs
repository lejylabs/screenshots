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
            foreach(var sample in new[]{("1e5","100000"),("1.25E+4","12500"),("-2.5e-3","-0.0025"),("1.000","1"),("-0","0")})
            {
                var numericProfile=BodyProfile("JSON","{\"credentials\":{\"values\":["+sample.Item1+"]}}","{inputbox:Caption|"+sample.Item1+"}");
                var numericContext=new Context{Kind="text"};numericContext.Answers["URL#0"]="https://example.invalid/chosen";
                var numericInterpreter=InterpreterFor(numericProfile,numericContext);
                var numericInspection=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(numericInterpreter.Inspect()));
                Check(numericInspection["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
                var numericPrepared=JObject.FromObject(numericInterpreter.Prepare());
                Check(numericPrepared["secrets"]!.Values<string>().Contains(sample.Item1));
                Check(numericPrepared["secrets"]!.Values<string>().Contains(sample.Item2));
            }
            var numericBound="{\"password\":["+string.Join(',',Enumerable.Range(0,65).Select(value=>value+"e0"))+"]}";
            RejectCode(()=>InterpreterFor(BodyProfile("JSON",numericBound),new Context{Kind="text"}).Prepare(),"body_auth_limit");
            var unrelatedNumeric=JObject.FromObject(InterpreterFor(BodyProfile("JSON","{\"caption\":[1e5]}"),new Context{Kind="text"}).Prepare());Check(!unrelatedNumeric["secrets"]!.Any());
            foreach(string ending in new[]{"\r","\n","\r\n"})
            {
                string commentData="{// ignored public 100000.00"+ending+"\"password\":1e5,\"caption\":100000.00}";
                var commentProfile=BodyProfile("JSON",commentData,"{inputbox:Caption|1e5}");
                var commentInterpreter=InterpreterFor(commentProfile,new Context{Kind="text"});
                var commentInspect=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(commentInterpreter.Inspect()));
                Check(commentInspect["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
                var commentContext=new Context{Kind="text"};commentContext.Answers["URL#0"]="https://example.invalid/chosen";
                var commentPrepared=JObject.FromObject(InterpreterFor(commentProfile,commentContext).Prepare());
                Check(commentPrepared["secrets"]!.Values<string>().Contains("1e5"));
                Check(!commentPrepared["secrets"]!.Values<string>().Contains("100000.00"));
            }
            string publicNumbers=string.Join(',',Enumerable.Range(1,127).Select(count=>"100000."+new string('0',count)));
            var scopedNumeric=JObject.FromObject(InterpreterFor(BodyProfile("JSON","{\"password\":1e5,\"caption\":["+publicNumbers+"]}"),new Context{Kind="text"}).Prepare());
            Check(scopedNumeric["secrets"]!.Count()==2);
            Check(scopedNumeric["secrets"]!.Values<string>().Contains("1e5"));
            Check(!scopedNumeric["secrets"]!.Values<string>().Contains("100000.00"));
            string duplicateNumbers="{\"caption\":100000.00,\"password\":1e5,\"password\":10e4,\"credentials\":[2e4]}";
            var duplicateNumeric=JObject.FromObject(InterpreterFor(BodyProfile("JSON",duplicateNumbers),new Context{Kind="text"}).Prepare());
            foreach(string raw in new[]{"1e5","10e4","2e4"})Check(duplicateNumeric["secrets"]!.Values<string>().Contains(raw));
            Check(!duplicateNumeric["secrets"]!.Values<string>().Contains("100000.00"));
            var extensionNumeric=JObject.FromObject(InterpreterFor(BodyProfile("JSON","{password:/* digits 20000.0 */1e5,caption:100000.00,credentials:[2e4]}"),new Context{Kind="text"}).Prepare());
            Check(extensionNumeric["secrets"]!.Values<string>().Contains("1e5"));Check(extensionNumeric["secrets"]!.Values<string>().Contains("2e4"));Check(!extensionNumeric["secrets"]!.Values<string>().Contains("100000.00"));
            foreach(string constructorData in new[]{
                "{\"password\":new Secret(1e5)}",
                "{\"password\": new Secret( 1e5 , 2e4 ) }",
                "{\"credentials\":{\"values\":[new Secret(1e5,2e4)]}}",
                "{\"credentials\":new Outer(new Inner(1e5),2e4)}",
                "{\"caption\":new Secret(1e5,2e4)}",
                "{\"caption\":{\"child\":new Secret(1e5,2e4)},\"password\":3e4}"})
            {
                var constructorProfile=BodyProfile("JSON",constructorData);
                RejectCode(()=>InterpreterFor(constructorProfile,new Context{Kind="text"}).Inspect(),"body_json_constructor");
                RejectCode(()=>InterpreterFor(constructorProfile,new Context{Kind="text"}).Prepare(),"body_json_constructor");
            }
            var constructorText=JObject.FromObject(InterpreterFor(BodyProfile("JSON","{\"caption\":\"new Secret(1e5,2e4)\"}"),new Context{Kind="text"}).Prepare());Check(!constructorText["secrets"]!.Any());
            foreach(var cookieCase in new[]{
                ("{\"Cookie\":{\"nested\":{\"value\":\"sid=opaque-cookie-private==\"}},\"caption\":\"sid=public-outside==\"}",new[]{"opaque-cookie-private=="},new[]{"public-outside=="}),
                ("{\"Cookie\":{\"nested\":[{\"value\":\"sid=\\\"opaque+quoted==\\\"; second=secondary-private==\"} ]},\"caption\":\"sid=public-outside==\"}",new[]{"opaque+quoted==","secondary-private=="},new[]{"public-outside=="}),
                ("{\"Set-Cookie\":{\"nested\":[{\"value\":\"sid=\\\"opaque-set-cookie==\\\"; Path=/; SameSite=None\"} ]},\"caption\":\"sid=public-outside==\"}",new[]{"opaque-set-cookie=="},new[]{"/","None","public-outside=="}),
                ("{\"Cookie\":{\"nested\":{\"Set-Cookie\":{\"value\":\"first=explicit-set==; next=public-attribute\"},\"sibling\":{\"value\":\"sid=parent-cookie==; second=parent-second==\"}}},\"caption\":\"sid=public-outside==\"}",new[]{"explicit-set==","parent-cookie==","parent-second=="},new[]{"public-attribute","public-outside=="}),
                ("{\"Set-Cookie\":{\"nested\":{\"Cookie\":[{\"value\":\"first=explicit-cookie==; next=explicit-second==\"}],\"sibling\":{\"value\":\"sid=parent-set==; extra=public-set-attribute\"}}},\"caption\":\"sid=public-outside==\"}",new[]{"explicit-cookie==","explicit-second==","parent-set=="},new[]{"public-set-attribute","public-outside=="}),
                ("{\"Authorization\":{\"nested\":[{\"value\":\"Bearer nested-bearer==\"}]},\"credentials\":{\"value\":\"Basic dXNlcjpwdw==\"},\"caption\":\"Bearer public-outside==\"}",new[]{"nested-bearer==","dXNlcjpwdw=="},new[]{"public-outside=="})})
            {
                foreach(string bare in cookieCase.Item2)
                {
                    var cookieProfile=BodyProfile("JSON",cookieCase.Item1,"{inputbox:Caption|"+bare+"}");
                    cookieProfile.Thumbnail="{inputbox:Public|public-outside==}";
                    var cookieContext=new Context{Kind="text"};cookieContext.Answers["URL#0"]="https://example.invalid/chosen";cookieContext.Answers["ThumbnailURL#0"]="https://example.invalid/thumb";
                    var cookieInterpreter=InterpreterFor(cookieProfile,cookieContext);
                    var cookieInspection=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(cookieInterpreter.Inspect()));
                    var cookieAnswers=cookieInspection["answers"]!.Children<JObject>().ToArray();
                    Check(cookieAnswers.Single(value=>value["id"]!.Value<string>()=="URL#0")["default_text"]!.Value<string>()=="[redacted]");
                    Check(cookieAnswers.Single(value=>value["id"]!.Value<string>()=="ThumbnailURL#0")["default_text"]!.Value<string>()=="public-outside==");
                    var cookiePrepared=JObject.FromObject(cookieInterpreter.Prepare());
                    foreach(string expected in cookieCase.Item2)Check(cookiePrepared["secrets"]!.Values<string>().Contains(expected));
                    foreach(string publicCookieValue in cookieCase.Item3)Check(!cookiePrepared["secrets"]!.Values<string>().Contains(publicCookieValue));
                }
            }
            foreach(var xmlCookieCase in new[]{
                ("<Cookie><nested value='sid=opaque-xml-cookie-private=='/></Cookie>",new[]{"opaque-xml-cookie-private=="},Array.Empty<string>()),
                ("<Cookie><first>sid=xml-first==</first><second>other=xml-second==</second></Cookie>",new[]{"xml-first==","xml-second=="},new[]{"xml-first==other=xml-second==","sid=xml-first==other=xml-second=="}),
                ("<Set-Cookie><nested value='sid=\"xml-quoted==\"; Path=/; SameSite=None'/></Set-Cookie>",new[]{"xml-quoted=="},new[]{"/","None"}),
                ("<Cookie><Set-Cookie><nested value='sid=xml-explicit-set==; next=public-attribute'/></Set-Cookie><sibling value='sid=xml-parent-cookie==; second=xml-parent-second=='/></Cookie>",new[]{"xml-explicit-set==","xml-parent-cookie==","xml-parent-second=="},new[]{"public-attribute"}),
                ("<Set-Cookie><Cookie><nested>sid=xml-explicit-cookie==; second=xml-explicit-second==</nested></Cookie><sibling value='sid=xml-parent-set==; next=public-attribute'/></Set-Cookie>",new[]{"xml-explicit-cookie==","xml-explicit-second==","xml-parent-set=="},new[]{"public-attribute"}),
                ("<r><nested Cookie='sid=xml-attribute=='/><caption value='sid=public-outside=='/></r>",new[]{"xml-attribute=="},new[]{"public-outside=="}),
                ("<Cookie><nested Set-Cookie='sid=xml-explicit-attribute==; next=public-attribute' value='sid=xml-sibling-cookie==; second=xml-sibling-second=='/></Cookie>",new[]{"xml-explicit-attribute==","xml-sibling-cookie==","xml-sibling-second=="},new[]{"public-attribute"}),
                ("<Set-Cookie><nested Cookie='sid=xml-explicit-attr-cookie==; next=xml-explicit-attr-second==' value='sid=xml-sibling-set==; next=public-attribute'/></Set-Cookie>",new[]{"xml-explicit-attr-cookie==","xml-explicit-attr-second==","xml-sibling-set=="},new[]{"public-attribute"}),
                ("<r><credentials><Authorization><value>Bearer xml-bearer==</value></Authorization><nested value='Basic eG1sOnB3=='/></credentials><caption value='Bearer public-outside=='/></r>",new[]{"xml-bearer==","eG1sOnB3=="},new[]{"public-outside=="})})
            {
                foreach(string bare in xmlCookieCase.Item2)
                {
                    var xmlCookieProfile=BodyProfile("XML",xmlCookieCase.Item1,"{inputbox:Caption|"+bare+"}");
                    var xmlCookieContext=new Context{Kind="text"};xmlCookieContext.Answers["URL#0"]="https://example.invalid/chosen";
                    var xmlCookieInterpreter=InterpreterFor(xmlCookieProfile,xmlCookieContext);
                    var xmlCookieInspection=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(xmlCookieInterpreter.Inspect()));
                    Check(xmlCookieInspection["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
                    var xmlCookiePrepared=JObject.FromObject(xmlCookieInterpreter.Prepare());
                    foreach(string expected in xmlCookieCase.Item2)Check(xmlCookiePrepared["secrets"]!.Values<string>().Contains(expected));
                    foreach(string publicXmlCookieValue in xmlCookieCase.Item3)Check(!xmlCookiePrepared["secrets"]!.Values<string>().Contains(publicXmlCookieValue));
                }
            }
            string repeatedNumbers="{\"password\":["+string.Join(',',Enumerable.Repeat("1e5",128))+"]}";
            var repeatedProfile=BodyProfile("JSON",repeatedNumbers,"{inputbox:Caption|1e5}");
            var repeatedContext=new Context{Kind="text"};repeatedContext.Answers["URL#0"]="https://example.invalid/chosen";
            var repeatedInterpreter=InterpreterFor(repeatedProfile,repeatedContext);
            var repeatedInspect=JObject.Parse(System.Text.Json.JsonSerializer.Serialize(repeatedInterpreter.Inspect()));Check(repeatedInspect["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
            var repeatedPrepared=JObject.FromObject(repeatedInterpreter.Prepare());Check(repeatedPrepared["secrets"]!.Count()==2);Check(repeatedPrepared["secrets"]!.Values<string>().Distinct().Count()==2);
            foreach(var repeatedContextCase in new[]{
                ("JSON","{\"credentials\":\"sid=context-first==; next=context-second==\",\"Set-Cookie\":{\"nested\":\"sid=context-first==; next=context-second==\"},\"Cookie\":{\"nested\":\"sid=context-first==; next=context-second==\"}}"),
                ("XML","<r><credentials value='sid=context-first==; next=context-second=='/><Set-Cookie><nested value='sid=context-first==; next=context-second=='/></Set-Cookie><Cookie><nested value='sid=context-first==; next=context-second=='/></Cookie></r>")})
            {
                var contextual=BodyProfile(repeatedContextCase.Item1,repeatedContextCase.Item2,"{inputbox:Caption|context-second==}");
                var contextualContext=new Context{Kind="text"};contextualContext.Answers["URL#0"]="https://example.invalid/chosen";
                var contextualInterpreter=InterpreterFor(contextual,contextualContext);
                Check(JObject.Parse(System.Text.Json.JsonSerializer.Serialize(contextualInterpreter.Inspect()))["answers"]![0]!["default_text"]!.Value<string>()=="[redacted]");
                var contextualPrepared=JObject.FromObject(contextualInterpreter.Prepare());var contextualValues=contextualPrepared["secrets"]!.Values<string>().ToArray();
                Check(contextualValues.Length==contextualValues.Distinct().Count());Check(contextualValues.Contains("context-first=="));Check(contextualValues.Contains("context-second=="));
            }
            var repeatedMaps=BodyProfile("JSON","{\"credentials\":\"sid=mapped-first==; next=mapped-second==\"}");
            repeatedMaps.Headers.Add("X-Token","sid=mapped-first==; next=mapped-second==");
            repeatedMaps.Arguments.Add("Cookie","sid=mapped-first==; next=mapped-second==");repeatedMaps.Body="MultipartFormData";
            repeatedMaps.Parameters.Add("Co%6fkie","sid=mapped-first==; next=mapped-second==");
            var mapped=JObject.FromObject(InterpreterFor(repeatedMaps,new Context{Kind="text"}).Prepare());var mappedValues=mapped["secrets"]!.Values<string>().ToArray();
            Check(mappedValues.Length==mappedValues.Distinct().Count());Check(mappedValues.Contains("mapped-first=="));Check(mappedValues.Contains("mapped-second=="));
            string uniqueCookieValues="{\"Cookie\":["+string.Join(',',Enumerable.Range(0,64).Select(value=>"\"sid=unique-cookie-"+value+"\""))+"]}";
            var uniqueCookieProfile=BodyProfile("JSON",uniqueCookieValues);uniqueCookieProfile.Headers.Add("Authorization","Bearer extra-unique");
            RejectCode(()=>InterpreterFor(uniqueCookieProfile,new Context{Kind="text"}).Prepare(),"authentication_limit");
            Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new { passed = true, checks })); return 0;
        }
        catch (Exception error) { Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new { passed = false, checks, code = error is Failure failure ? failure.Code : error.GetType().Name })); return 1; }
    }
}
