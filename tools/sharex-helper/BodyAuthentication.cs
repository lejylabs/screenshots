using System.Globalization;
using System.Text;
using System.Xml;
using System.Xml.XPath;
using Newtonsoft.Json;

namespace Screenshots.ShareXRuntime;

// Shared configured/resolved-body policy. It never rewrites the transmitted body.
internal static class BodyAuthentication
{
    internal static void Scan(string body, string? unknownMarker, Action<string,string> emit)
    {
        Limits.Bounded(body,Limits.Body);string trimmed=body.TrimStart();int count=0;long bytes=0;var known=new HashSet<string>(StringComparer.Ordinal);
        void Add(string name,string value)
        {
            if(value.Length==0 || unknownMarker!=null&&value.Contains(unknownMarker,StringComparison.Ordinal))return;
            if(known.Add(value))Limits.Require(++count<=128&&(bytes+=Encoding.UTF8.GetByteCount(value))<=256*1024,"body_auth_limit");
            emit(name,value);
        }
        if(trimmed.StartsWith('{')||trimmed.StartsWith('['))
        {
            try
            {
                int sourceLine=1,lineStart=0;
                string RawNumber(JsonTextReader token)
                {
                    while(sourceLine<token.LineNumber)
                    {
                        Limits.Require(lineStart<body.Length,"body_number_position");
                        char ch=body[lineStart++];
                        if(ch=='\r'){if(lineStart<body.Length&&body[lineStart]=='\n')lineStart++;sourceLine++;}
                        else if(ch=='\n')sourceLine++;
                    }
                    int end=checked(lineStart+token.LinePosition);
                    Limits.Require(end>lineStart&&end<=body.Length,"body_number_position");
                    int start=end;
                    while(start>lineStart&&!char.IsWhiteSpace(body[start-1])&&body[start-1] is not (',' or ':' or '[' or ']' or '{' or '}' or '/' or '\"' or '\''))start--;
                    Limits.Require(start<end,"body_number_position");
                    return body[start..end];
                }
                using var reader=new JsonTextReader(new StringReader(body)){DateParseHandling=DateParseHandling.None,MaxDepth=64};
                var context=new Stack<bool>();context.Push(false);var cookies=new Stack<byte>();cookies.Push(0);byte pendingCookie=0;bool property=false;string propertyName="body";
                while(reader.Read())
                {
                    switch(reader.TokenType)
                    {
                        case JsonToken.StartConstructor:case JsonToken.EndConstructor:throw new Failure("body_json_constructor");
                        case JsonToken.PropertyName: propertyName=(string)reader.Value!;property=context.Peek()||Interpreter.Sensitive(propertyName);pendingCookie=propertyName.Equals("Cookie",StringComparison.OrdinalIgnoreCase)?(byte)1:propertyName.Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase)?(byte)2:cookies.Peek();break;
                        case JsonToken.StartObject:case JsonToken.StartArray:context.Push(context.Peek()||property);cookies.Push(pendingCookie==0?cookies.Peek():pendingCookie);property=false;pendingCookie=0;break;
                        case JsonToken.EndObject:case JsonToken.EndArray:if(context.Count>1){context.Pop();cookies.Pop();}property=false;pendingCookie=0;propertyName="body";break;
                        case JsonToken.String:case JsonToken.Integer:case JsonToken.Float:case JsonToken.Boolean:case JsonToken.Date:
                            if(context.Peek()||property)
                            {
                                string value=reader.TokenType==JsonToken.Boolean?(reader.Value is true?"true":"false"):Convert.ToString(reader.Value,CultureInfo.InvariantCulture)??"";
                                byte cookie=pendingCookie==0?cookies.Peek():pendingCookie;Add(cookie==1?"Cookie":cookie==2?"Set-Cookie":"body",value);
                                if(reader.TokenType is JsonToken.Integer or JsonToken.Float)
                                {
                                    string raw=RawNumber(reader);
                                    if(!known.Contains(raw))Add("body",raw);
                                }
                            }
                            property=false;pendingCookie=0;break;
                        case JsonToken.Null:case JsonToken.Undefined:property=false;pendingCookie=0;break;
                    }
                }
                return;
            }
            catch(JsonReaderException error){if(error.Message.Contains("MaxDepth",StringComparison.OrdinalIgnoreCase))throw new Failure("body_depth");}
        }
        if(trimmed.StartsWith('<'))
        {
            Limits.Require(!body.Contains("<!DOCTYPE",StringComparison.OrdinalIgnoreCase)&&!body.Contains("<!ENTITY",StringComparison.OrdinalIgnoreCase),"body_dtd_forbidden");
            var settings=new XmlReaderSettings{DtdProcessing=DtdProcessing.Prohibit,XmlResolver=null,MaxCharactersInDocument=Limits.Body,MaxCharactersFromEntities=0};
            try
            {
                using(var depth=XmlReader.Create(new StringReader(body),settings))while(depth.Read())Limits.Require(depth.Depth<=64,"body_depth");
                using var reader=XmlReader.Create(new StringReader(body),settings);var document=new XPathDocument(reader);var nodes=document.CreateNavigator().SelectDescendants(XPathNodeType.Element,true);int elements=0;
                while(nodes.MoveNext())
                {
                    Limits.Require(++elements<=65536,"body_nodes");var node=nodes.Current!;var ancestor=node.Clone();bool auth=false;string? cookie=null;
                    do
                    {
                        if(ancestor.NodeType!=XPathNodeType.Element)continue;
                        auth|=Interpreter.Sensitive(ancestor.LocalName);
                        if(cookie==null&&(ancestor.LocalName.Equals("Cookie",StringComparison.OrdinalIgnoreCase)||ancestor.LocalName.Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase)))cookie=ancestor.LocalName;
                    }while(ancestor.MoveToParent());
                    if(auth)
                    {
                        if(cookie==null)Add(node.LocalName,node.Value);
                        else
                        {
                            // Individual text values are headers; never concatenate sibling elements.
                            var texts=node.SelectChildren(XPathNodeType.All);
                            while(texts.MoveNext())if(texts.Current!.NodeType is XPathNodeType.Text or XPathNodeType.Whitespace or XPathNodeType.SignificantWhitespace)
                            {
                                string value=texts.Current.Value;if(!string.IsNullOrWhiteSpace(value))Add(cookie,value);
                            }
                        }
                    }
                    var attributes=node.Clone();if(attributes.MoveToFirstAttribute())do
                    {
                        if(auth||Interpreter.Sensitive(attributes.LocalName))
                        {
                            string name=attributes.LocalName;
                            string scope=name.Equals("Cookie",StringComparison.OrdinalIgnoreCase)||name.Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase)?name:cookie??name;
                            Add(scope,attributes.Value);
                        }
                    }while(attributes.MoveToNextAttribute());
                }
                return;
            }
            catch(XmlException){}
        }
        // Raw form data is classified even under a custom Content-Type. Unrelated keys stay data.
        foreach(string pair in body.Split('&'))
        {
            int equals=pair.IndexOf('=');if(equals<0)continue;string name=Interpreter.DeliveredQueryName(pair[..equals]);
            if(Interpreter.Sensitive(name))Add(name,Interpreter.DeliveredQueryName(pair[(equals+1)..]));
        }
    }
}
