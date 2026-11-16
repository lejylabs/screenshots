using System.Text;
using System.Text.RegularExpressions;
using Newtonsoft.Json.Linq;
using Json = System.Text.Json.JsonSerializer;

namespace Screenshots.ShareXRuntime;

internal static class Program
{
    static int Main(string[] args)
    {
        Encoding.RegisterProvider(CodePagesEncodingProvider.Instance);
        if (args.SequenceEqual(new[] { "--self-test" })) return SelfTest.Run();
        if (args.Length != 0) return 2;
        using var watchdog = new System.Threading.Timer(_ => Environment.Exit(3), null, TimeSpan.FromSeconds(5), System.Threading.Timeout.InfiniteTimeSpan);
        object result;
        try
        {
            using var input = Console.OpenStandardInput(); using var memory = new MemoryStream(); byte[] buffer = new byte[65536]; int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0) { Limits.Require(memory.Length + read <= Limits.Input, "request_limit"); memory.Write(buffer, 0, read); }
            string text = new UTF8Encoding(false, true).GetString(memory.ToArray()); JObject request = Limits.Object(text);
            Limits.Require(request["version"]?.Value<int>() == 1, "protocol_version");
            string stage = Limits.String(request["stage"], 16); var profile = Profile.Read(Limits.String(request["profile_json"], 16384)); var context = Context.Read(request);
            var interpreter = new Interpreter(profile, context); interpreter.Compile();
            object value = stage switch { "inspect" => interpreter.Inspect(), "prepare" => interpreter.Prepare(), "response" => interpreter.Response(), _ => throw new Failure("stage") };
            result = new { ok = true, result = value };
        }
        catch (Failure error) { result = new { ok = false, code = error.Code }; }
        catch (RegexMatchTimeoutException) { result = new { ok = false, code = "regex_timeout" }; }
        catch (OutOfMemoryException) { result = new { ok = false, code = "memory_limit" }; }
        catch (Exception) { result = new { ok = false, code = "invalid_expression_or_request" }; }
        byte[] output = Json.SerializeToUtf8Bytes(result);
        if (output.Length > Limits.Output) output = Json.SerializeToUtf8Bytes(new { ok = false, code = "output_limit" });
        using var stream = Console.OpenStandardOutput(); stream.Write(output); stream.Flush(); return 0;
    }
}
