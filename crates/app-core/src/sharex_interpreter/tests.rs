//! Port of the former helper self-test, plus protocol envelope checks.
use super::*;

fn ctx(kind: &str) -> Context {
    Context {
        filename: String::new(),
        input: String::new(),
        response: String::new(),
        response_url: String::new(),
        kind: kind.into(),
        status: 200,
        allow_environment: false,
        answer_indices: HashMap::new(),
        function_names: HashMap::new(),
        answers: HashMap::new(),
        names: HashMap::new(),
        headers: HashMap::new(),
        events: Vec::new(),
        marker: std::cell::OnceCell::new(),
        cancel: None,
        deadline: Instant::now() + Duration::from_secs(60),
    }
}
fn code<T>(result: R<T>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(Fail::Code(code)) => code,
        Err(Fail::Abort(_)) => "abort",
    }
}
fn profile_for(template: &str) -> Profile {
    let text = serde_json::json!({"Version":"17.0.0","DestinationType":"FileUploader","RequestURL":"https://example.invalid/upload","Body":"Binary","URL":template}).to_string();
    Profile::read(&text).ok().expect("profile")
}
fn body_profile(mode: &str, data: &str, url: &str) -> Profile {
    let text = serde_json::json!({"Version":"17.0.0","DestinationType":"TextUploader","RequestURL":"https://example.invalid/upload","Body":mode,"Data":data,"URL":url}).to_string();
    Profile::read(&text).ok().expect("profile")
}
fn interpreter(profile: Profile, context: Context) -> Interpreter {
    Interpreter::new(profile, context).ok().expect("compile")
}
fn eval_with(template: &str, context: &mut Context) -> R<String> {
    let mut working = context.clone();
    working.events.clear();
    Interpreter::new(profile_for(template), working)?.response().map(|r| r.url)
}
fn eval(template: &str, context: &mut Context) -> String {
    match eval_with(template, context) {
        Ok(value) => value,
        Err(fail) => panic!("{template}: {}", code::<()>(Err(fail))),
    }
}
fn rejects(template: &str, context: &mut Context) -> bool {
    eval_with(template, context).is_err()
}
fn prepared(interp: &mut Interpreter) -> Prepared {
    interp.prepare().ok().expect("prepare")
}
fn inspected(interp: &mut Interpreter) -> serde_json::Value {
    interp.inspect().ok().expect("inspect")
}
fn answer<'a>(inspection: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    inspection["answers"].as_array().unwrap().iter().find(|a| a["id"] == id).expect("answer")
}

#[test]
fn functions_escape_select_and_extract() {
    let mut c = ctx("file");
    c.filename = "my file.png".into();
    c.input = "hello + world".into();
    c.response_url = "https://example.invalid/upload".into();
    c.response = "plain-response".into();
    assert_eq!(eval("{filename}", &mut c), "my%20file.png");
    assert_eq!(eval("{INPUT}", &mut c), "hello%20%2B%20world");
    assert_eq!(eval("{response}", &mut c), "plain-response");
    assert_eq!(eval("{responseurl}", &mut c), "https://example.invalid/upload");
    c.headers.insert("location".into(), "https://example.invalid/result".into());
    assert_eq!(eval("{header:location}", &mut c), "https://example.invalid/result");
    assert_eq!(eval("{base64:a:b}", &mut c), "YTpi");
    c.answers.insert("URL#1".into(), "se64".into());
    assert_eq!(eval("{ba{select:se64|se64}:a:b}", &mut c), "YTpi");
    assert_eq!(eval("{random:same|same}", &mut c), "same");
    c.answers.insert("URL#0".into(), "second".into());
    assert_eq!(eval("{select:first|second}", &mut c), "second");
    assert_eq!(eval("{inputbox}", &mut c), "second");
    assert_eq!(eval("{prompt:title|default}", &mut c), "second");
    let mut interp = interpreter(profile_for("{outputbox:Title|hello}"), std::mem::replace(&mut c, ctx("file")));
    let message = interp.response().ok().unwrap();
    assert_eq!(message.url, "");
    assert_eq!(message.events.len(), 1);
    c = interp.context;
    assert_eq!(eval(r"\{literal\}\|\\", &mut c), "{literal}|\\");

    c.response = r#"{"files":[{"id":1,"url":"a"},{"id":2,"url":"b"}],"nested":{"target":"c"},"url":"abc"}"#.into();
    assert_eq!(eval("{json:files[1].url}", &mut c), "b");
    assert_eq!(eval("{json:files[?(@.id == 2)].url}", &mut c), "b");
    assert_eq!(eval("{json:$..target}", &mut c), "c");
    assert_eq!(eval("{base64:{json:url}}", &mut c), "YWJj");
    assert_eq!(eval("{json:{response}|files[0].url}", &mut c), "a");
    assert!(rejects("{json:files[*].url}", &mut c));
    assert!(rejects("{json:files}", &mut c));

    c.response = "abab".into();
    assert_eq!(eval(r"{regex:(ab)\\1|1}", &mut c), "ab");
    c.response = "id=123".into();
    assert_eq!(eval("{regex:(?<=id=)[0-9]+}", &mut c), "123");
    c.response = "word".into();
    assert_eq!(eval("{regex:(?<name>[a-z]+)|name}", &mut c), "word");
    assert_eq!(eval("{regex:explicit-input|(?<word>[a-z]+)|word}", &mut c), "explicit");

    c.response = "<files><file id='a'><url>A</url></file><file id='b'><url>B</url></file></files>".into();
    assert_eq!(eval("{xml:/files/file[@id='b']/url}", &mut c), "B");
    assert_eq!(eval("{xml:/files/file[2]/preceding-sibling::file/@id}", &mut c), "a");
    assert_eq!(eval("{xml:{response}|/files/file[1]/url}", &mut c), "A");
    assert_eq!(eval("{xml://url}", &mut c), "A");
    assert!(rejects("{xml:count(/files/file)}", &mut c));
    c.response = "<!DOCTYPE x [<!ENTITY attack SYSTEM 'file:///must-not-read'>]><x>&attack;</x>".into();
    assert!(rejects("{xml:/x}", &mut c));
    // Without backtracking-only syntax the linear engine runs; it cannot blow up.
    c.response = "a".repeat(10000) + "!";
    assert_eq!(eval("{regex:^(a+)+$}", &mut c), "");
    // Backtracking constructs are bounded by the backtrack limit instead of a timer.
    c.response = "a".repeat(64) + "!";
    assert_eq!(code(eval_with(r"{regex:^(?:a\|a)+(?!a)b}", &mut c)), "regex_timeout");
    for bad in ["{unknown}", "{base64}", "{random:one}", "{json:unclosed"] {
        assert!(rejects(bad, &mut c), "{bad}");
    }
    let mut missing = ctx("file");
    assert!(rejects("{inputbox}", &mut missing));
    assert!(rejects("{select:a|b}", &mut missing));
}

#[test]
fn json_path_follows_json_net_values() {
    let mut c = ctx("file");
    c.response = r#"{"a-b":{"n":1.50,"t":true,"z":null,"big":12345678901234567890},"list":[1,2,3,4]}"#.into();
    assert_eq!(eval("{json:a-b.n}", &mut c), "1.5");
    assert_eq!(eval("{json:a-b.t}", &mut c), "True");
    assert_eq!(eval("{json:a-b.z}", &mut c), "");
    assert_eq!(eval("{json:a-b.missing}", &mut c), "");
    assert_eq!(eval("{json:a-b.big}", &mut c), "12345678901234567890");
    assert_eq!(eval("{json:['a-b']['n']}", &mut c), "1.5");
    assert_eq!(eval("{json:list[-1]}", &mut c), "4");
    assert_eq!(eval("{json:list[?(@ > 3)]}", &mut c), "4");
    assert!(rejects("{json:list[1:3]}", &mut c));
}

#[test]
fn name_codes_expand_frozen_context() {
    let mut names = ctx("file");
    for (key, value) in [
        ("now", "2026-11-15T14:07:03.123"),
        ("counter", "40"),
        ("width", "640"),
        ("height", "480"),
        ("title", " synthetic title "),
        ("process", " synthetic process "),
        ("user", "synthetic-user"),
        ("domain", "synthetic-domain"),
        ("computer", "synthetic-computer"),
    ] {
        names.names.insert(key.into(), value.into());
    }
    let expand = |text: &str, c: &Context| names::expand(text, c, true, false).ok().expect(text);
    assert_eq!(expand("%y-%yy-%mo-%d %h:%mi:%s.%ms", &names), "2026-26-11-15 14:07:03.123");
    assert_eq!(expand("%h %pm", &names), "02 PM");
    assert_eq!(expand("%mon2 %w2 %wy", &names), "November Sunday 46");
    assert_eq!(expand("%i{4}-%ia-%iA-%ix-%iX-%iAa-%iaA-%ib{16,3}", &names), "0041-15-15-29-29-f-F-029");
    assert_eq!(
        expand("%width/%height/%t/%pn/%un/%uln/%cn", &names),
        "640/480/synthetic_title/synthetic_process/synthetic-user/synthetic-domain/synthetic-computer"
    );
    assert_eq!(expand("a%rf{/must-not-read}b", &names), "ab");
    assert!(expand("%rn{8}", &names).bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(expand("%rna{8}", &names).chars().count(), 8);
    assert_eq!(expand("%guid", &names).len(), 36);
    assert!(expand("%radjective %ranimal %remoji{2}", &names).chars().count() > 5);
    assert_eq!(code(names::expand("%un", &ctx("file"), true, false)), "identity_context_required");
    assert_eq!(code(names::expand("%rn{999999999}", &names, true, false)), "name_repeat_limit");
    assert!(expand("%unix", &names).len() >= 10);
}

#[test]
fn profiles_admit_migrate_and_reject() {
    let modern = r#"{"Version":"17.0.0","DestinationType":"TextUploader","RequestURL":"https://example.invalid/upload","Body":"JSON","Data":"{\"text\":\"{input}\"}"}"#;
    let profile = Profile::read(modern).ok().unwrap();
    let mut text = ctx("text");
    text.input = "a\"b\n".into();
    let mut interp = interpreter(profile, text);
    let data = base64::engine::general_purpose::STANDARD.decode(prepared(&mut interp).data).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(parsed["text"], "a\"b\n");
    let profile = Profile::read(modern).ok().unwrap();
    assert!(profile.admit_kind("text").is_ok());
    assert!(profile.admit_kind("file").is_err());
    let old = Profile::read(r#"{"Version":"13.7.1","DestinationType":4,"RequestURL":"example.invalid/upload?a=$filename$","Body":5,"URL":"$json:url$"}"#).ok().unwrap();
    assert_eq!(old.link, "{json:url}");
    assert_eq!(old.parameters, vec![("a".to_owned(), "{filename}".to_owned())]);
    assert!(Profile::read(r#"{"Version":"12.3.1.0","RequestURL":"https://example.invalid/"}"#).is_err());
    assert!(Profile::read(r#"{"Version":"12.3.1","RequestURL":"https://example.invalid/"}"#).is_err());
    assert_eq!(code(Profile::read(&modern.replace("\"Data\":", "\"unknown\":1,\"Data\":"))), "unknown_field");
    for name in ["auth", "key", "X-Auth", "xAuth", "access_key", "AccessKey", "access_key_id", "aws_access_key_id", "secret_access_key"] {
        assert!(sensitive(name), "{name}");
    }
    for name in ["author", "monkey", "keyboard", "filename", "index"] {
        assert!(!sensitive(name), "{name}");
    }
}

#[test]
fn answers_bindings_and_contracts_are_enforced() {
    for field in ["URL", "ThumbnailURL", "DeletionURL", "ErrorMessage"] {
        for function in ["{inputbox:Title|default}", "{prompt:Title}", "{select:first|second}"] {
            let mut config = serde_json::json!({"Version":"17.0.0","DestinationType":"FileUploader","RequestURL":"https://example.invalid/upload","Body":"Binary"});
            config[field] = function.into();
            let mut interp = interpreter(Profile::read(&config.to_string()).ok().unwrap(), ctx("file"));
            assert_eq!(code(interp.prepare()), "answer_required", "{field} {function}");
        }
    }
    let dynamic = "{select:{json:empty}|{json:first}|{json:second}}";
    assert_eq!(code(interpreter(profile_for(dynamic), ctx("file")).prepare()), "selection_index_required");
    let mut indexed = ctx("file");
    indexed.response = r#"{"empty":"","first":"A","second":"B"}"#.into();
    indexed.answer_indices.insert("URL#0".into(), 1);
    let mut interp = interpreter(profile_for(dynamic), indexed);
    prepared(&mut interp);
    assert_eq!(interp.response().ok().unwrap().url, "B");

    let computed = "{ba{json:suffix}:value}";
    assert_eq!(code(interpreter(profile_for(computed), ctx("file")).prepare()), "function_binding_required");
    let mut bound = ctx("file");
    bound.response = r#"{"suffix":"se64"}"#.into();
    bound.function_names.insert("URL#0".into(), "base64".into());
    let mut interp = interpreter(profile_for(computed), bound);
    prepared(&mut interp);
    assert_eq!(interp.response().ok().unwrap().url, "dmFsdWU=");
    interp.context.response = r#"{"suffix":"other"}"#.into();
    assert_eq!(code(interp.response()), "function_binding_mismatch");

    let mut prompt = ctx("file");
    prompt.function_names.insert("URL#0".into(), "inputbox".into());
    assert_eq!(code(interpreter(profile_for("{{json:function}:Title}"), prompt).prepare()), "answer_required");

    let inspection = inspected(&mut interpreter(profile_for(dynamic), ctx("file")));
    assert!(inspection["answer_contracts"].as_array().unwrap().iter().any(|v| v["id"] == "URL#0"
        && v["answer_kind"] == "index"
        && v["stage"] == "response"));
}

#[test]
fn query_names_and_inspection_redaction() {
    for name in ["a%75th", "%6bey", "access%5fkey", "X%2dAuth"] {
        assert!(sensitive(&delivered_query_name(name).ok().unwrap()), "{name}");
    }
    assert_eq!(delivered_query_name("public%2bname").ok().unwrap(), "public+name");
    assert_eq!(delivered_query_name("public+name").ok().unwrap(), "public name");

    let mut config = profile_for("{responseurl}");
    config.parameters.push(("a%75th".into(), "synthetic+private".into()));
    let request = prepared(&mut interpreter(config, ctx("file")));
    assert_eq!(request.url, "https://example.invalid/upload?a%75th=synthetic%2bprivate");
    assert!(request.secrets.contains(&"synthetic+private".to_owned()));

    let mut profile = profile_for("{inputbox:Caption|opaque+value}");
    profile.headers.push(("Authorization".into(), "Bearer opaque+value".into()));
    profile.headers.push(("Cookie".into(), "sid=pad==; other=secondopaque".into()));
    profile.parameters.push(("a%75th".into(), "paramopaque".into()));
    profile.thumbnail = "{inputbox:opaque%2bvalue|public fallback}".into();
    profile.deletion = "{select:public-first|pad==|public-last}".into();
    profile.error = "{inputbox:Other|param%6fpaque}".into();
    let mut interp = interpreter(profile, ctx("file"));
    let inspection = inspected(&mut interp);
    assert_eq!(answer(&inspection, "URL#0")["default_text"], "[redacted]");
    assert_eq!(answer(&inspection, "URL#0")["title"], "Caption");
    assert_eq!(answer(&inspection, "ThumbnailURL#0")["title"], "[redacted]");
    assert_eq!(answer(&inspection, "ThumbnailURL#0")["default_text"], "public fallback");
    assert_eq!(answer(&inspection, "DeletionURL#0")["choices"], serde_json::json!(["public-first", "[redacted]", "public-last"]));
    assert_eq!(answer(&inspection, "ErrorMessage#0")["default_text"], "[redacted]");
}

fn secrets_of(mode: &str, data: &str) -> Vec<String> {
    prepared(&mut interpreter(body_profile(mode, data, "{responseurl}"), ctx("text"))).secrets
}
fn redacts(mode: &str, data: &str, caption: &str) -> bool {
    let mut interp = interpreter(body_profile(mode, data, &format!("{{inputbox:Caption|{caption}}}")), ctx("text"));
    answer(&inspected(&mut interp), "URL#0")["default_text"] == "[redacted]"
}

#[test]
fn body_credentials_are_classified() {
    for (mode, data, secret) in [
        ("XML", "<r><password>opaque-xml</password><caption>public</caption></r>", "opaque-xml"),
        ("XML", "<r password='opaque-attribute'><note>public</note></r>", "opaque-attribute"),
        ("JSON", r#"{"credentials":{"password":"opaque-json"},"caption":"public"}"#, "opaque-json"),
        ("JSON", "password=opaque%2Bform&caption=public", "opaque+form"),
    ] {
        assert!(secrets_of(mode, data).contains(&secret.to_owned()), "{data}");
    }
    for prefix in ["{caption}", "[caption]", "<caption>", "{\"public\":true}"] {
        let raw = format!("{prefix}=public&password=opaque%2Bprefix");
        assert!(secrets_of("JSON", &raw).contains(&"opaque+prefix".to_owned()), "{raw}");
        assert!(redacts("JSON", &raw, "opaque+prefix"), "{raw}");
    }
    for mode in ["JSON", "XML"] {
        let source = if mode == "JSON" {
            r#"{"password":"opaque%y","caption":"public"}"#
        } else {
            "<r password='opaque%y'><caption>public</caption></r>"
        };
        let mut frozen = ctx("text");
        frozen.names.insert("now".into(), "2020-01-02T03:04:05".into());
        let profile = body_profile(mode, source, "{inputbox:Caption|opaque2020}");
        assert_eq!(profile.data, source);
        let mut interp = interpreter(profile, frozen);
        let inspection = inspected(&mut interp);
        assert_eq!(inspection["answers"][0]["default_text"], "[redacted]");
        assert_eq!(inspection["answers"][0]["title"], "Caption");
        interp.context.answers.insert("URL#0".into(), "https://example.invalid/chosen".into());
        assert!(prepared(&mut interp).secrets.contains(&"opaque2020".to_owned()));
    }
    assert!(redacts("JSON", "password=opaque%25y", "opaque%y"));
    let mut ignored = body_profile("FormURLEncoded", "password=unused-private", "{responseurl}");
    ignored.arguments.push(("caption".into(), "public".into()));
    assert!(!prepared(&mut interpreter(ignored, ctx("text"))).secrets.contains(&"unused-private".to_owned()));
    assert!(secrets_of("JSON", r#"{"caption":"password=public"}"#).is_empty());
    assert_eq!(
        code(interpreter(body_profile("XML", "<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///must-not-read'>]><r>&x;</r>", "{responseurl}"), ctx("text")).prepare()),
        "body_dtd_forbidden"
    );
    assert!(redacts("JSON", r#"{"Cookie":"sid=opaque-body-cookie=="}"#, "opaque-body-cookie=="));
    assert!(redacts("JSON", r#"{"counter":%rn{4},"password":"known-with-random-sibling"}"#, "known-with-random-sibling"));
}

#[test]
fn body_numbers_keep_raw_and_normalized_forms() {
    for (raw, normalized) in [("1e5", "100000"), ("1.25E+4", "12500"), ("-2.5e-3", "-0.0025"), ("1.000", "1"), ("-0", "0")] {
        let data = format!(r#"{{"credentials":{{"values":[{raw}]}}}}"#);
        assert!(redacts("JSON", &data, raw), "{raw}");
        let secrets = secrets_of("JSON", &data);
        assert!(secrets.contains(&raw.to_owned()) && secrets.contains(&normalized.to_owned()), "{raw} {secrets:?}");
    }
    let bound = format!(r#"{{"password":[{}]}}"#, (0..65).map(|v| format!("{v}e0")).collect::<Vec<_>>().join(","));
    assert_eq!(code(interpreter(body_profile("JSON", &bound, "{responseurl}"), ctx("text")).prepare()), "body_auth_limit");
    assert!(secrets_of("JSON", r#"{"caption":[1e5]}"#).is_empty());
    for ending in ["\r", "\n", "\r\n"] {
        let data = format!("{{// ignored public 100000.00{ending}\"password\":1e5,\"caption\":100000.00}}");
        assert!(redacts("JSON", &data, "1e5"));
        let secrets = secrets_of("JSON", &data);
        assert!(secrets.contains(&"1e5".to_owned()) && !secrets.contains(&"100000.00".to_owned()));
    }
    let public: Vec<String> = (1..128).map(|n| format!("100000.{}", "0".repeat(n))).collect();
    let secrets = secrets_of("JSON", &format!(r#"{{"password":1e5,"caption":[{}]}}"#, public.join(",")));
    assert_eq!(secrets.len(), 2);
    assert!(secrets.contains(&"1e5".to_owned()) && !secrets.contains(&"100000.00".to_owned()));
    let secrets = secrets_of("JSON", r#"{"caption":100000.00,"password":1e5,"password":10e4,"credentials":[2e4]}"#);
    for raw in ["1e5", "10e4", "2e4"] {
        assert!(secrets.contains(&raw.to_owned()), "{raw}");
    }
    assert!(!secrets.contains(&"100000.00".to_owned()));
    let secrets = secrets_of("JSON", "{password:/* digits 20000.0 */1e5,caption:100000.00,credentials:[2e4]}");
    assert!(secrets.contains(&"1e5".to_owned()) && secrets.contains(&"2e4".to_owned()));
    assert!(!secrets.contains(&"100000.00".to_owned()));
    for data in [
        r#"{"password":new Secret(1e5)}"#,
        r#"{"password": new Secret( 1e5 , 2e4 ) }"#,
        r#"{"credentials":{"values":[new Secret(1e5,2e4)]}}"#,
        r#"{"credentials":new Outer(new Inner(1e5),2e4)}"#,
        r#"{"caption":new Secret(1e5,2e4)}"#,
        r#"{"caption":{"child":new Secret(1e5,2e4)},"password":3e4}"#,
    ] {
        assert_eq!(code(interpreter(body_profile("JSON", data, "{responseurl}"), ctx("text")).inspect()), "body_json_constructor");
        assert_eq!(code(interpreter(body_profile("JSON", data, "{responseurl}"), ctx("text")).prepare()), "body_json_constructor");
    }
    assert!(secrets_of("JSON", r#"{"caption":"new Secret(1e5,2e4)"}"#).is_empty());
    let repeated = format!(r#"{{"password":[{}]}}"#, vec!["1e5"; 128].join(","));
    assert!(redacts("JSON", &repeated, "1e5"));
    assert_eq!(secrets_of("JSON", &repeated).len(), 2);
}

#[test]
fn cookie_scopes_follow_json_and_xml_nesting() {
    let cases: [(&str, &str, &[&str], &[&str]); 15] = [
        ("JSON", r#"{"Cookie":{"nested":{"value":"sid=opaque-cookie-private=="}},"caption":"sid=public-outside=="}"#, &["opaque-cookie-private=="], &["public-outside=="]),
        ("JSON", r#"{"Cookie":{"nested":[{"value":"sid=\"opaque+quoted==\"; second=secondary-private=="} ]},"caption":"sid=public-outside=="}"#, &["opaque+quoted==", "secondary-private=="], &["public-outside=="]),
        ("JSON", r#"{"Set-Cookie":{"nested":[{"value":"sid=\"opaque-set-cookie==\"; Path=/; SameSite=None"} ]},"caption":"sid=public-outside=="}"#, &["opaque-set-cookie=="], &["/", "None", "public-outside=="]),
        ("JSON", r#"{"Cookie":{"nested":{"Set-Cookie":{"value":"first=explicit-set==; next=public-attribute"},"sibling":{"value":"sid=parent-cookie==; second=parent-second=="}}},"caption":"sid=public-outside=="}"#, &["explicit-set==", "parent-cookie==", "parent-second=="], &["public-attribute", "public-outside=="]),
        ("JSON", r#"{"Set-Cookie":{"nested":{"Cookie":[{"value":"first=explicit-cookie==; next=explicit-second=="}],"sibling":{"value":"sid=parent-set==; extra=public-set-attribute"}}},"caption":"sid=public-outside=="}"#, &["explicit-cookie==", "explicit-second==", "parent-set=="], &["public-set-attribute", "public-outside=="]),
        ("JSON", r#"{"Authorization":{"nested":[{"value":"Bearer nested-bearer=="}]},"credentials":{"value":"Basic dXNlcjpwdw=="},"caption":"Bearer public-outside=="}"#, &["nested-bearer==", "dXNlcjpwdw=="], &["public-outside=="]),
        ("XML", "<Cookie><nested value='sid=opaque-xml-cookie-private=='/></Cookie>", &["opaque-xml-cookie-private=="], &[]),
        ("XML", "<Cookie><first>sid=xml-first==</first><second>other=xml-second==</second></Cookie>", &["xml-first==", "xml-second=="], &["xml-first==other=xml-second==", "sid=xml-first==other=xml-second=="]),
        ("XML", "<Set-Cookie><nested value='sid=\"xml-quoted==\"; Path=/; SameSite=None'/></Set-Cookie>", &["xml-quoted=="], &["/", "None"]),
        ("XML", "<Cookie><Set-Cookie><nested value='sid=xml-explicit-set==; next=public-attribute'/></Set-Cookie><sibling value='sid=xml-parent-cookie==; second=xml-parent-second=='/></Cookie>", &["xml-explicit-set==", "xml-parent-cookie==", "xml-parent-second=="], &["public-attribute"]),
        ("XML", "<Set-Cookie><Cookie><nested>sid=xml-explicit-cookie==; second=xml-explicit-second==</nested></Cookie><sibling value='sid=xml-parent-set==; next=public-attribute'/></Set-Cookie>", &["xml-explicit-cookie==", "xml-explicit-second==", "xml-parent-set=="], &["public-attribute"]),
        ("XML", "<r><nested Cookie='sid=xml-attribute=='/><caption value='sid=public-outside=='/></r>", &["xml-attribute=="], &["public-outside=="]),
        ("XML", "<Cookie><nested Set-Cookie='sid=xml-explicit-attribute==; next=public-attribute' value='sid=xml-sibling-cookie==; second=xml-sibling-second=='/></Cookie>", &["xml-explicit-attribute==", "xml-sibling-cookie==", "xml-sibling-second=="], &["public-attribute"]),
        ("XML", "<Set-Cookie><nested Cookie='sid=xml-explicit-attr-cookie==; next=xml-explicit-attr-second==' value='sid=xml-sibling-set==; next=public-attribute'/></Set-Cookie>", &["xml-explicit-attr-cookie==", "xml-explicit-attr-second==", "xml-sibling-set=="], &["public-attribute"]),
        ("XML", "<r><credentials><Authorization><value>Bearer xml-bearer==</value></Authorization><nested value='Basic eG1sOnB3=='/></credentials><caption value='Bearer public-outside=='/></r>", &["xml-bearer==", "eG1sOnB3=="], &["public-outside=="]),
    ];
    for (mode, data, expected, public) in cases {
        for bare in expected {
            let mut profile = body_profile(mode, data, &format!("{{inputbox:Caption|{bare}}}"));
            profile.thumbnail = "{inputbox:Public|public-outside==}".into();
            let mut c = ctx("text");
            c.answers.insert("URL#0".into(), "https://example.invalid/chosen".into());
            c.answers.insert("ThumbnailURL#0".into(), "https://example.invalid/thumb".into());
            let mut interp = interpreter(profile, c);
            let inspection = inspected(&mut interp);
            assert_eq!(answer(&inspection, "URL#0")["default_text"], "[redacted]", "{data} {bare}");
            assert_eq!(answer(&inspection, "ThumbnailURL#0")["default_text"], "public-outside==", "{data}");
            let secrets = prepared(&mut interp).secrets;
            for value in expected {
                assert!(secrets.contains(&value.to_string()), "{data} {value} {secrets:?}");
            }
            for value in public {
                assert!(!secrets.contains(&value.to_string()), "{data} {value}");
            }
        }
    }
    for (mode, data) in [
        ("JSON", r#"{"credentials":"sid=context-first==; next=context-second==","Set-Cookie":{"nested":"sid=context-first==; next=context-second=="},"Cookie":{"nested":"sid=context-first==; next=context-second=="}}"#),
        ("XML", "<r><credentials value='sid=context-first==; next=context-second=='/><Set-Cookie><nested value='sid=context-first==; next=context-second=='/></Set-Cookie><Cookie><nested value='sid=context-first==; next=context-second=='/></Cookie></r>"),
    ] {
        assert!(redacts(mode, data, "context-second=="));
        let secrets = secrets_of(mode, data);
        let unique: HashSet<_> = secrets.iter().collect();
        assert_eq!(unique.len(), secrets.len());
        assert!(secrets.contains(&"context-first==".to_owned()) && secrets.contains(&"context-second==".to_owned()));
    }
    let mut maps = body_profile("JSON", r#"{"credentials":"sid=mapped-first==; next=mapped-second=="}"#, "{responseurl}");
    maps.headers.push(("X-Token".into(), "sid=mapped-first==; next=mapped-second==".into()));
    maps.arguments.push(("Cookie".into(), "sid=mapped-first==; next=mapped-second==".into()));
    maps.body = "MultipartFormData";
    maps.parameters.push(("Co%6fkie".into(), "sid=mapped-first==; next=mapped-second==".into()));
    let secrets = prepared(&mut interpreter(maps, ctx("text"))).secrets;
    let unique: HashSet<_> = secrets.iter().collect();
    assert_eq!(unique.len(), secrets.len());
    assert!(secrets.contains(&"mapped-first==".to_owned()) && secrets.contains(&"mapped-second==".to_owned()));
    let cookies = format!(r#"{{"Cookie":[{}]}}"#, (0..64).map(|v| format!("\"sid=unique-cookie-{v}\"")).collect::<Vec<_>>().join(","));
    let mut profile = body_profile("JSON", &cookies, "{responseurl}");
    profile.headers.push(("Authorization".into(), "Bearer extra-unique".into()));
    assert_eq!(code(interpreter(profile, ctx("text")).prepare()), "authentication_limit");
}

#[test]
fn protocol_envelopes_match_the_former_helper() {
    let cancel = crate::UploadCancellation::default();
    let request = serde_json::json!({
        "version": 1, "stage": "prepare", "kind": "file", "filename": "a b.png", "input": "",
        "profile_json": r#"{"Version":"17.0.0","DestinationType":"FileUploader","RequestURL":"https://example.invalid/{filename}","Body":"MultipartFormData","FileFormName":"file","Headers":{"Z":"1","A":"2"}}"#,
    });
    let output = interpret(&serde_json::to_vec(&request).unwrap(), &cancel).unwrap();
    let text = std::str::from_utf8(&output).unwrap();
    assert!(text.find("\"Z\"").unwrap() < text.find("\"A\"").unwrap(), "header order is kept: {text}");
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["url"], "https://example.invalid/a%20b.png");
    assert_eq!(value["result"]["attach_file"], true);
    assert_eq!(value["result"]["data"], "");
    let refused = interpret(br#"{"version":2}"#, &cancel).unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&refused).unwrap(), serde_json::json!({"ok":false,"code":"protocol_version"}));
    let duplicate = interpret(br#"{"version":1,"version":1}"#, &cancel).unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&duplicate).unwrap()["ok"], false);
    cancel.cancel();
    assert!(interpret(&serde_json::to_vec(&request).unwrap(), &cancel).is_err());
}
