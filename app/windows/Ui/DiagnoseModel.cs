// DiagnoseModel.cs - the parts of `odr diagnose --json` the UI shows. Tolerant: any missing key is just empty.
using System;
using System.Collections.Generic;
using System.Linq;
using System.Web.Script.Serialization;

namespace OdrApp.Ui
{
    public sealed class Finding
    {
        public string Code = "", Severity = "", CauseAr = "", CauseEn = "", RecoveryAr = "", RecoveryEn = "";
        public long Blocks; public int Confidence;
        public List<string> Evidence = new List<string>(), OracleErrors = new List<string>();
        public List<KeyValuePair<string, long>> Parameters = new List<KeyValuePair<string, long>>();

        public string TitleAr
        {
            get
            {
                switch (Code)
                {
                    case "HEALTHY": return "الملف سليم";
                    case "HEADER_DESTROYED": return "رأس الملف مدمَّر";
                    case "TRUNCATED": return "الملف مقطوع";
                    case "RANSOMWARE_HEAD": return "فدية: تشفير بداية الملف";
                    case "RANSOMWARE_PERIODIC": return "فدية: تشفير متقطّع";
                    case "ENCRYPTED_SCATTERED": return "كتل مشفَّرة متفرقة";
                    case "PAYLOAD_SCRAMBLED": return "تشفير يحافظ على الرأس (مخفي عن dbv)";
                    case "ZEROED_REGIONS": return "مناطق أصفار";
                    case "BIT_ROT": return "تلف مادي (checksum)";
                    case "TORN_WRITES": return "كتابات مقطوعة (انقطاع كهرباء)";
                    case "MISDIRECTED_WRITES": return "كتل في غير مكانها";
                    case "LOGICAL_CORRUPTION": return "تلف منطقي";
                    case "MARKED_CORRUPT": return "كتل وسمها أوراكل تالفة";
                    case "GARBAGE_OVERWRITE": return "كتابة غريبة فوق الملف";
                    case "SIZE_MISMATCH": return "حجم مختلف عن الرأس";
                    case "STALE_BLOCKS": return "كتل قديمة (كتابات ضائعة)";
                    case "DUPLICATE_BLOCKS": return "كتل مكرَّرة";
                    case "HEADER_BLOCKSIZE_MISMATCH": return "حجم الكتلة مختلف بين الرأس والجسم";
                    case "CHECKSUM_COVERAGE_LOW": return "تغطية checksum ناقصة";
                    case "CONSISTENCY_PASS_SKIPPED": return "فحص الاتساق لم يُنفَّذ";
                    case "TAIL_FOOTER": return "بيانات غريبة في ذيل الملف";
                    default: return Code;
                }
            }
        }
    }

    public sealed class NextStep { public string PurposeAr = "", Command = ""; }

    public sealed class Diagnosis
    {
        public string File = "", Sha256 = "", LayoutSource = "", Band = "";
        public int BlockSize, Score = -1, ExpectedLossMilli, CleanUsedMilli, ChecksumCoverageMilli = -1;
        public long FileLen, BlocksPresent, Examined, Data, Index, Lob, Other, Empty, DamagedTotal, BytesAffected;
        public long? LiveRows;
        public bool IdentityKnown, Unchanged;
        public string VerdictAr = "";
        public Dictionary<string, long> Damaged = new Dictionary<string, long>();
        public List<Finding> Findings = new List<Finding>();
        public List<NextStep> NextSteps = new List<NextStep>();
        public List<Tuple<long, long, string>> Regions = new List<Tuple<long, long, string>>();
        public int ObjectCount;
        public string Raw = "";

        public static Diagnosis Parse(string json)
        {
            var d = new Diagnosis { Raw = json };
            var js = new JavaScriptSerializer { MaxJsonLength = int.MaxValue, RecursionLimit = 512 };
            var root = js.DeserializeObject(json) as Dictionary<string, object>;
            if (root == null) throw new FormatException("not a diagnose report");
            d.File = S(root, "file"); d.Sha256 = S(root, "sha256_scanned"); d.FileLen = L(root, "file_len"); d.BlocksPresent = L(root, "blocks_present");
            d.Unchanged = root.ContainsKey("unchanged") && root["unchanged"] is bool u && u;
            var layout = root["layout"] as Dictionary<string, object>;
            if (layout != null) { d.BlockSize = (int)L(layout, "block_size"); d.LayoutSource = S(layout, "source"); }
            var c = root["counters"] as Dictionary<string, object>;
            if (c != null)
            {
                d.Examined = L(c, "examined"); d.Data = L(c, "data"); d.Index = L(c, "index"); d.Lob = L(c, "lob"); d.Other = L(c, "other"); d.Empty = L(c, "empty");
                if (c["damaged"] is Dictionary<string, object> dm) foreach (var kv in dm) d.Damaged[kv.Key] = Convert.ToInt64(kv.Value);
                d.DamagedTotal = d.Damaged.Values.Sum();
            }
            if (root["objects"] is Dictionary<string, object> objs) d.ObjectCount = objs.Count;
            if (root["regions"] is object[] regs)
                foreach (Dictionary<string, object> r in regs) d.Regions.Add(Tuple.Create(L(r, "start"), L(r, "len"), S(r, "code")));
            if (root["findings"] is object[] fs)
                foreach (Dictionary<string, object> f in fs)
                {
                    var x = new Finding
                    {
                        Code = S(f, "code"), Severity = S(f, "severity"), Blocks = L(f, "blocks"), Confidence = (int)L(f, "confidence_milli"),
                        CauseAr = S(f, "cause_ar"), CauseEn = S(f, "cause_en"), RecoveryAr = S(f, "recovery_ar"), RecoveryEn = S(f, "recovery_en")
                    };
                    if (f["evidence"] is object[] ev) x.Evidence.AddRange(ev.Select(e => Convert.ToString(e)));
                    if (f["oracle_errors"] is object[] oe) x.OracleErrors.AddRange(oe.Select(e => Convert.ToString(e)));
                    if (f.ContainsKey("parameters") && f["parameters"] is object[] ps)
                        foreach (object[] p in ps) if (p.Length == 2) x.Parameters.Add(new KeyValuePair<string, long>(Convert.ToString(p[0]), Convert.ToInt64(p[1])));
                    d.Findings.Add(x);
                }
            if (root["triage"] is Dictionary<string, object> t)
            {
                d.Score = (int)L(t, "score"); d.Band = S(t, "band"); d.ExpectedLossMilli = (int)L(t, "expected_loss_milli"); d.CleanUsedMilli = (int)L(t, "clean_used_milli");
                d.BytesAffected = L(t, "bytes_affected"); d.IdentityKnown = t.ContainsKey("identity_known") && t["identity_known"] is bool ik && ik;
                d.VerdictAr = S(t, "verdict_ar");
                if (t.ContainsKey("live_rows") && t["live_rows"] != null) d.LiveRows = Convert.ToInt64(t["live_rows"]);
                if (t.ContainsKey("checksum_coverage_milli") && t["checksum_coverage_milli"] != null) d.ChecksumCoverageMilli = Convert.ToInt32(t["checksum_coverage_milli"]);
                if (t["next_steps"] is object[] ns)
                    foreach (Dictionary<string, object> n in ns) d.NextSteps.Add(new NextStep { PurposeAr = S(n, "purpose_ar"), Command = S(n, "command") });
            }
            return d;
        }

        static string S(Dictionary<string, object> d, string k) { object v; return d.TryGetValue(k, out v) && v != null ? Convert.ToString(v) : ""; }
        static long L(Dictionary<string, object> d, string k) { object v; return d.TryGetValue(k, out v) && v != null ? Convert.ToInt64(v) : 0; }

        public static string DamageAr(string code)
        {
            switch (code)
            {
                case "zeroed": return "أصفار";
                case "header_zeroed": return "رأس مصفَّر";
                case "encrypted": return "مشفَّرة";
                case "payload_scrambled": return "حمولة مشفَّرة";
                case "garbage": return "بيانات غريبة";
                case "marked_corrupt": return "موسومة تالفة";
                case "checksum_fail": return "checksum خاطئ";
                case "torn": return "مقطوعة";
                case "misplaced": return "في غير مكانها";
                case "logical": return "تلف منطقي";
                case "truncated": return "مبتورة";
                case "stale_scn": return "قديمة";
                case "duplicate_rdba": return "مكرَّرة";
                default: return code;
            }
        }

        public static string Bytes(long b)
        {
            if (b >= 1L << 30) return (b / (double)(1L << 30)).ToString("0.##") + " GB";
            if (b >= 1L << 20) return (b / (double)(1L << 20)).ToString("0.#") + " MB";
            if (b >= 1L << 10) return (b / 1024.0).ToString("0") + " KB";
            return b + " B";
        }
    }
}
