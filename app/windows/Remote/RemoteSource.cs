// RemoteSource.cs - reads the control files from the PUBLIC repository moain2028/odr-apps, cache-free.
//
//   odr/license.json  <- remote kill switch (lock / unlock / per-device block)
//   odr/update.json   <- latest installer announcement
//
// Primary: GitHub Contents API (no CDN cache, 60 req/h per IP). Fallback: raw.githubusercontent
// (CDN, may lag up to 5 min). Every request carries a unique nonce because the API answers with
// `cache-control: max-age=60` and its edge may return a stale copy right after an edit (measured on
// the Flutter twin of this module, Qurat Al-Mahalat v1.2.1).
//
// NEVER change Owner / Repo / Folder after shipping: installed copies read this path literally.
// NEVER put secrets here: this file ships inside a public download.
using System;
using System.IO;
using System.Net;
using System.Net.Http;
using System.Text;
using System.Threading.Tasks;
using System.Web.Script.Serialization;

namespace OdrApp.Remote
{
    public enum RemoteKind { Data, NotFound, Offline }

    public sealed class RemoteResult
    {
        public RemoteKind Kind;
        public System.Collections.Generic.Dictionary<string, object> Data;
        public bool IsData => Kind == RemoteKind.Data && Data != null;
        public bool IsNotFound => Kind == RemoteKind.NotFound;
        public bool IsOffline => Kind == RemoteKind.Offline;
        public static readonly RemoteResult OfflineResult = new RemoteResult { Kind = RemoteKind.Offline };
        public static readonly RemoteResult NotFoundResult = new RemoteResult { Kind = RemoteKind.NotFound };
    }

    public static class RemoteSource
    {
        public const string Owner = "moain2028";
        public const string Repo = "odr-apps";
        public const string Branch = "main";
        public const string Folder = "odr";

        static readonly TimeSpan Timeout = TimeSpan.FromSeconds(8);
        static HttpClient _http;

        /// <summary>Replaceable in tests: given a URL returns (status, body) or throws.</summary>
        public static Func<string, Task<Tuple<int, string>>> Fetcher;

        static HttpClient Http()
        {
            if (_http != null) return _http;
            ServicePointManager.SecurityProtocol |= SecurityProtocolType.Tls12;
            var h = new HttpClient { Timeout = Timeout };
            h.DefaultRequestHeaders.UserAgent.ParseAdd("ODR-Windows/" + System.Windows.Forms.Application.ProductVersion);
            h.DefaultRequestHeaders.CacheControl = new System.Net.Http.Headers.CacheControlHeaderValue { NoCache = true };
            return _http = h;
        }

        public static string ApiUrl(string file, long nonce) =>
            $"https://api.github.com/repos/{Owner}/{Repo}/contents/{Folder}/{file}?ref={Branch}&t={nonce}";

        public static string RawUrl(string file, long nonce) =>
            $"https://raw.githubusercontent.com/{Owner}/{Repo}/{Branch}/{Folder}/{file}?nocache={nonce}";

        /// <summary>Fetch a JSON object. Never throws.</summary>
        public static async Task<RemoteResult> FetchAsync(string file)
        {
            var api = await ViaApiAsync(file);
            if (api != null) return api;
            return await ViaRawAsync(file) ?? RemoteResult.OfflineResult;
        }

        static async Task<Tuple<int, string>> GetAsync(string url)
        {
            if (Fetcher != null) return await Fetcher(url);
            using (var req = new HttpRequestMessage(HttpMethod.Get, url))
            {
                req.Headers.Accept.ParseAdd("application/vnd.github+json");
                using (var resp = await Http().SendAsync(req))
                {
                    var body = await resp.Content.ReadAsStringAsync();
                    return Tuple.Create((int)resp.StatusCode, body);
                }
            }
        }

        static async Task<RemoteResult> ViaApiAsync(string file)
        {
            try
            {
                var r = await GetAsync(ApiUrl(file, DateTime.UtcNow.Ticks));
                if (r.Item1 == 404) return RemoteResult.NotFoundResult;
                if (r.Item1 != 200) return null; // 403 rate limit etc. -> try raw
                var js = new JavaScriptSerializer();
                var env = js.DeserializeObject(r.Item2) as System.Collections.Generic.Dictionary<string, object>;
                if (env == null || !env.ContainsKey("content")) return null;
                var b64 = ((string)env["content"]).Replace("\n", "").Replace("\r", "");
                var json = Encoding.UTF8.GetString(Convert.FromBase64String(b64));
                return Parse(json);
            }
            catch { return null; }
        }

        static async Task<RemoteResult> ViaRawAsync(string file)
        {
            try
            {
                var r = await GetAsync(RawUrl(file, DateTime.UtcNow.Ticks));
                if (r.Item1 == 404) return RemoteResult.NotFoundResult;
                if (r.Item1 != 200) return null;
                return Parse(r.Item2);
            }
            catch { return null; }
        }

        /// <summary>Parses a JSON object; a corrupt file counts as *offline* (never a false lock).</summary>
        public static RemoteResult Parse(string json)
        {
            try
            {
                var js = new JavaScriptSerializer();
                var d = js.DeserializeObject(json) as System.Collections.Generic.Dictionary<string, object>;
                return d == null ? null : new RemoteResult { Kind = RemoteKind.Data, Data = d };
            }
            catch { return null; }
        }
    }
}
