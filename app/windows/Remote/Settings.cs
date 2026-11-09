// Settings.cs - tiny persistent key/value store (JSON in %LOCALAPPDATA%\ODR\settings.json).
// Used by the remote license state; nothing here is secret (the activation code is never stored, only its hash).
using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Web.Script.Serialization;

namespace OdrApp.Remote
{
    public sealed class Settings
    {
        readonly string path;
        readonly Dictionary<string, object> d = new Dictionary<string, object>();

        public static string DefaultPath =>
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ODR", "settings.json");

        /// <summary>In-memory store (tests).</summary>
        public Settings() { path = null; }

        public Settings(string file)
        {
            path = file;
            try
            {
                if (File.Exists(file))
                {
                    var js = new JavaScriptSerializer();
                    var m = js.DeserializeObject(File.ReadAllText(file, Encoding.UTF8)) as Dictionary<string, object>;
                    if (m != null) foreach (var kv in m) d[kv.Key] = kv.Value;
                }
            }
            catch { /* unreadable settings must never block the app */ }
        }

        public string Get(string k, string def) { object v; return d.TryGetValue(k, out v) && v != null ? Convert.ToString(v) : def; }
        public bool GetBool(string k, bool def) { object v; return d.TryGetValue(k, out v) && v is bool b ? b : def; }
        public int GetInt(string k, int def) { object v; int r; return d.TryGetValue(k, out v) && int.TryParse(Convert.ToString(v), out r) ? r : def; }
        public long GetLong(string k, long def) { object v; long r; return d.TryGetValue(k, out v) && long.TryParse(Convert.ToString(v), out r) ? r : def; }
        public void Set(string k, object v) { d[k] = v; }

        public void Save()
        {
            if (path == null) return;
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(path));
                var tmp = path + ".tmp";
                File.WriteAllText(tmp, new JavaScriptSerializer().Serialize(d), Encoding.UTF8);
                if (File.Exists(path)) File.Replace(tmp, path, null); else File.Move(tmp, path);
            }
            catch { }
        }
    }
}
