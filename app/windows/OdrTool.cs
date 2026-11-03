// OdrTool.cs - runs the odr.exe / odr-corrupt.exe engines (installed in .\tool) and parses their output.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading.Tasks;

namespace OdrApp
{
    public sealed class RunResult
    {
        public int Code;
        public string Out = "";
        public string Err = "";
        public bool Cancelled;
        public string All => (Out + "\n" + Err).Trim();
    }

    /// <summary>Layer counts printed by `odr extract` ("verified N · inferred N · rejected N").</summary>
    public sealed class Layers
    {
        public int Verified = -1, Inferred = -1, Rejected = -1;
        public bool Ok => Verified >= 0;
        public static Layers Parse(string text)
        {
            var l = new Layers();
            var ms = Regex.Matches(text ?? "", @"verified (\d+)\D+inferred (\d+)\D+rejected (\d+)");
            if (ms.Count > 0)
            {
                var m = ms[ms.Count - 1];
                l.Verified = int.Parse(m.Groups[1].Value);
                l.Inferred = int.Parse(m.Groups[2].Value);
                l.Rejected = int.Parse(m.Groups[3].Value);
            }
            return l;
        }
    }

    public static class OdrTool
    {
        public static string AppDir => AppDomain.CurrentDomain.BaseDirectory.TrimEnd('\\', '/');

        public static string Find(string exe)
        {
            foreach (var d in new[] { Path.Combine(AppDir, "tool"), AppDir })
            {
                var p = Path.Combine(d, exe);
                if (File.Exists(p)) return p;
            }
            return null;
        }

        public static string Odr => Find("odr.exe");
        public static string Corrupt => Find("odr-corrupt.exe");
        public static string DemoDir => Path.Combine(AppDir, "demo");
        public static bool HasDemo => File.Exists(Path.Combine(DemoDir, "FREE_FREEPDB1_system01.dbf"));

        static Process current;
        static bool cancelRequested;
        static readonly object gate = new object();

        public static void Cancel()
        {
            lock (gate)
            {
                cancelRequested = true;
                try { if (current != null && !current.HasExited) current.Kill(); } catch { }
            }
        }

        /// <summary>Windows command-line quoting (CommandLineToArgvW rules).</summary>
        public static string Quote(string a)
        {
            if (a.Length > 0 && a.IndexOfAny(new[] { ' ', '\t', '"' }) < 0) return a;
            var sb = new StringBuilder("\"");
            int bs = 0;
            foreach (var c in a)
            {
                if (c == '\\') { bs++; continue; }
                if (c == '"') { sb.Append('\\', bs * 2 + 1); sb.Append('"'); bs = 0; continue; }
                sb.Append('\\', bs); bs = 0; sb.Append(c);
            }
            sb.Append('\\', bs * 2);
            sb.Append('"');
            return sb.ToString();
        }

        public static Task<RunResult> RunAsync(string exe, IEnumerable<string> args)
        {
            var argLine = string.Join(" ", args.Select(Quote));
            return Task.Run(() =>
            {
                var r = new RunResult();
                if (exe == null || !File.Exists(exe))
                {
                    r.Code = -1;
                    r.Err = "odr.exe غير موجود بجانب البرنامج (مجلد tool). أعد تثبيت البرنامج.";
                    return r;
                }
                var psi = new ProcessStartInfo(exe, argLine)
                {
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    CreateNoWindow = true,
                    StandardOutputEncoding = Encoding.UTF8,
                    StandardErrorEncoding = Encoding.UTF8,
                    WorkingDirectory = Path.GetTempPath(),
                };
                using (var p = new Process { StartInfo = psi })
                {
                    lock (gate) { cancelRequested = false; }
                    p.Start();
                    lock (gate) { current = p; }
                    var o = p.StandardOutput.ReadToEndAsync();
                    var e = p.StandardError.ReadToEndAsync();
                    p.WaitForExit();
                    r.Out = o.Result;
                    r.Err = e.Result;
                    r.Code = p.ExitCode;
                    lock (gate) { current = null; r.Cancelled = cancelRequested; }
                }
                return r;
            });
        }

        /// <summary>Plain-Arabic explanation of an odr error line, plus the original text.</summary>
        public static string Explain(string text)
        {
            var t = text ?? "";
            if (t.Contains("no SYSTEM datafile"))
                return "لم يُعثر على ملف SYSTEM بين الملفات المختارة. أضف ملف SYSTEM (مثل SYSTEM01.DBF)، أو استخدم «استخراج بدون ملف SYSTEM».";
            if (t.Contains("not found in the dictionary"))
                return "الجدول غير موجود في القاموس.";
            if (t.Contains("compressed"))
                return "الجدول مضغوط بطريقة غير مدعومة؛ رُفض بدلاً من التخمين.";
            if (t.Contains("Permission denied") || t.Contains("Access is denied") || t.Contains("os error 5"))
                return "لا توجد صلاحية لقراءة الملف. انسخ الملف إلى مجلد آخر، أو أوقف خدمة أوراكل التي تستخدمه.";
            if (t.Contains("os error 32") || t.Contains("being used by another process"))
                return "الملف مستخدم من برنامج آخر (غالباً أوراكل). أوقف القاعدة أو انسخ الملف أولاً.";
            return "";
        }
    }
}
