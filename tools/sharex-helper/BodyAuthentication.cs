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
        Limits.Bounded(body,Limits.Body);string trimmed=body.TrimStart();int count=0;long bytes=0;
        void Add(string name,string value)
        {
            if(value.Length==0 || unknownMarker!=null&&value.Contains(unknownMarker,StringComparison.Ordinal))return;
            Limits.Require(++count<=128&&(bytes+=Encoding.UTF8.GetByteCount(value))<=256*1024,"body_auth_limit");emit(name,value);
        }
        if(trimmed.StartsWith('{')||trimmed.StartsWith('['))
        {
            try
            {
                using var reader=new JsonTextReader(new StringReader(body)){DateParseHandling=DateParseHandling.None,MaxDepth=64};
                var context=new Stack<bool>();context.Push(false);var names=new Stack<string>();names.Push("body");bool property=false;string propertyName="body";
                while(reader.Read())
                {
                    switch(reader.TokenType)
                    {
                        case JsonToken.PropertyName: propertyName=(string)reader.Value!;property=context.Peek()||Interpreter.Sensitive(propertyName);break;
                        case JsonToken.StartObject:case JsonToken.StartArray:context.Push(context.Peek()||property);names.Push(property?propertyName:names.Peek());property=false;break;
                        case JsonToken.EndObject:case JsonToken.EndArray:if(context.Count>1){context.Pop();names.Pop();}property=false;propertyName="body";break;
                        case JsonToken.String:case JsonToken.Integer:case JsonToken.Float:case JsonToken.Boolean:case JsonToken.Date:
                            if(context.Peek()||property)Add(propertyName.Equals("Cookie",StringComparison.OrdinalIgnoreCase)||propertyName.Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase)?propertyName:names.Peek().Equals("Cookie",StringComparison.OrdinalIgnoreCase)||names.Peek().Equals("Set-Cookie",StringComparison.OrdinalIgnoreCase)?names.Peek():property?propertyName:names.Peek(),reader.TokenType==JsonToken.Boolean?(reader.Value is true?"true":"false"):Convert.ToString(reader.Value,CultureInfo.InvariantCulture)??"");property=false;break;
                        case JsonToken.Null:case JsonToken.Undefined:property=false;break;
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
                    Limits.Require(++elements<=65536,"body_nodes");var node=nodes.Current!;var ancestor=node.Clone();bool auth=Interpreter.Sensitive(node.LocalName);while(!auth&&ancestor.MoveToParent())auth=ancestor.NodeType==XPathNodeType.Element&&Interpreter.Sensitive(ancestor.LocalName);
                    if(auth)Add(node.LocalName,node.Value);
                    var attributes=node.Clone();if(attributes.MoveToFirstAttribute())do{if(auth||Interpreter.Sensitive(attributes.LocalName))Add(attributes.LocalName,attributes.Value);}while(attributes.MoveToNextAttribute());
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
