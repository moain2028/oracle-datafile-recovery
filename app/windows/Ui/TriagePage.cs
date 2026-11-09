// TriagePage.cs - many files in, one worst-first table out. Double-click opens the full diagnosis.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace OdrApp.Ui
{
    public sealed class TriagePage : UserControl
    {
        readonly TextBox txtDir = new TextBox();
        readonly Button btnDir = Theme.Secondary("📂 اختيار مجلد"), btnFiles = Theme.Secondary("➕ اختيار ملفات"), btnRun = Theme.Primary("⚖️ فرز", 140), btnCancel = Theme.Danger("إيقاف"), btnCsv = Theme.Secondary("💾 حفظ CSV");
        readonly CheckBox chkFast = new CheckBox { Text = "سريع (مادي فقط)", AutoSize = true, Margin = new Padding(8, 10, 8, 4) };
        readonly ListView lv = Theme.List(Theme.Col("الدرجة", 70), Theme.Col("الحالة", 90), Theme.Col("الملف", 300), Theme.Col("الحجم", 90), Theme.Col("كتل", 80), Theme.Col("تالفة", 90), Theme.Col("تالف %", 80), Theme.Col("أبرز نتيجة", 260), Theme.Col("ملاحظة", 300));
        readonly Label lblStatus = new Label();
        readonly ProgressBar bar = new ProgressBar { Style = ProgressBarStyle.Marquee, Visible = false, Width = 220, Height = 10 };
        readonly List<string> picked = new List<string>();
        public event Action<string> OpenDiagnosis;
        public event Action<string> Status;

        public TriagePage()
        {
            Dock = DockStyle.Fill; BackColor = Theme.Bg; RightToLeft = RightToLeft.Yes; Font = Theme.Base; Padding = new Padding(12);
            var top = new TableLayoutPanel { Dock = DockStyle.Top, AutoSize = true, ColumnCount = 1 };
            var head = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top, Padding = new Padding(0, 0, 0, 6) };
            var info = Theme.Info(); info.Click += (s, e) => Help.Show(FindForm(), "الفرز", Help.Triage);
            head.Controls.Add(Theme.Title("⚖️ الفرز — رتّب ملفات كثيرة من الأسوأ إلى الأفضل")); head.Controls.Add(info);
            head.Controls.Add(Theme.MutedLabel("بعد حادثة: مجلد كامل ← قائمة أولويات خلال دقائق. انقر مرتين على صف لفتح تشخيصه."));
            var row = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top };
            txtDir.Width = 480; txtDir.RightToLeft = RightToLeft.No; txtDir.Margin = new Padding(4, 7, 4, 4); txtDir.Font = Theme.Mono; txtDir.ReadOnly = true;
            row.Controls.AddRange(new Control[] { txtDir, btnDir, btnFiles, chkFast, btnRun, btnCancel, bar, btnCsv });
            btnCancel.Enabled = false; btnCsv.Enabled = false;
            top.Controls.Add(head); top.Controls.Add(row);
            lblStatus.Dock = DockStyle.Bottom; lblStatus.Height = 26; lblStatus.ForeColor = Theme.Muted; lblStatus.TextAlign = ContentAlignment.MiddleRight;
            Controls.Add(Theme.Card(lv, 2)); Controls.Add(lblStatus); Controls.Add(top);

            btnDir.Click += (s, e) => { using (var d = new FolderBrowserDialog { Description = "اختر مجلداً فيه ملفات .dbf" }) if (d.ShowDialog(this) == DialogResult.OK) { picked.Clear(); picked.Add(d.SelectedPath); txtDir.Text = d.SelectedPath; } };
            btnFiles.Click += (s, e) => { using (var d = new OpenFileDialog { Multiselect = true, Filter = "ملفات أوراكل (*.dbf;*.ora;*.dat)|*.dbf;*.ora;*.dat|كل الملفات|*.*" }) if (d.ShowDialog(this) == DialogResult.OK) { picked.Clear(); picked.AddRange(d.FileNames); txtDir.Text = d.FileNames.Length + " ملف"; } };
            btnRun.Click += async (s, e) => await RunAsync();
            btnCancel.Click += (s, e) => OdrTool.Cancel();
            lv.DoubleClick += (s, e) => { if (lv.SelectedItems.Count == 1 && lv.SelectedItems[0].Tag is string p && File.Exists(p)) OpenDiagnosis?.Invoke(p); };
            btnCsv.Click += (s, e) => SaveCsv();
        }

        void Say(string s) { lblStatus.Text = s; Status?.Invoke(s); }

        public async Task RunAsync()
        {
            if (picked.Count == 0) { Say("اختر مجلداً أو ملفات أولاً."); return; }
            foreach (var c in new Control[] { btnDir, btnFiles, btnRun }) c.Enabled = false;
            btnCancel.Enabled = true; bar.Visible = true; UseWaitCursor = true;
            Say("جارٍ فرز الملفات…");
            var args = new List<string> { "triage" }; args.AddRange(picked); args.Add("--json"); if (chkFast.Checked) args.Add("--fast");
            var r = await OdrTool.RunAsync(OdrTool.Odr, args);
            foreach (var c in new Control[] { btnDir, btnFiles, btnRun }) c.Enabled = true;
            btnCancel.Enabled = false; bar.Visible = false; UseWaitCursor = false;
            if (r.Cancelled) { Say("أُوقف."); return; }
            lv.Items.Clear();
            try
            {
                var js = new JavaScriptSerializer { MaxJsonLength = int.MaxValue, RecursionLimit = 512 };
                var rows = js.DeserializeObject(r.Out) as object[];
                if (rows == null) throw new FormatException("unexpected triage output");
                int n = 0;
                // odr triage --json: [{file, bytes, score, band, damaged, examined, live_rows, top_finding, layout_source, error}] worst-first
                foreach (Dictionary<string, object> e in rows)
                {
                    string path = S(e, "file");
                    var it = new ListViewItem("") { Tag = path };
                    string err = S(e, "error");
                    if (err != "" || e["score"] == null)
                    {
                        it.Text = "—"; it.SubItems.Add("ليس أوراكل"); it.SubItems.Add(Path.GetFileName(path)); it.SubItems.Add(Diagnosis.Bytes(L(e, "bytes")));
                        it.SubItems.Add(""); it.SubItems.Add(""); it.SubItems.Add(""); it.SubItems.Add(""); it.SubItems.Add(err);
                        it.BackColor = Color.FromArgb(240, 240, 240);
                    }
                    else
                    {
                        int score = (int)L(e, "score"); string band = S(e, "band");
                        long dmg = L(e, "damaged"), ex = L(e, "examined");
                        it.Text = score.ToString(); it.SubItems.Add(Theme.BandAr(band)); it.SubItems.Add(Path.GetFileName(path));
                        it.SubItems.Add(Diagnosis.Bytes(L(e, "bytes"))); it.SubItems.Add(ex.ToString("N0"));
                        it.SubItems.Add(dmg.ToString("N0"));
                        it.SubItems.Add(ex > 0 ? (dmg * 100.0 / ex).ToString("0.#") : "");
                        it.SubItems.Add(FindingAr(S(e, "top_finding")));
                        it.SubItems.Add(S(e, "layout_source") == "Inferred" ? "رأس الملف ضائع — التخطيط مستنتج" : "");
                        it.BackColor = score >= 70 ? Theme.OkBg : score >= 40 ? Theme.WarnBg : Theme.BadBg;
                    }
                    lv.Items.Add(it); n++;
                }
                btnCsv.Enabled = n > 0;
                Say($"تم فرز {n} ملف — الأسوأ أولاً.");
            }
            catch (Exception ex) { Say("تعذّر فهم مخرجات الفرز: " + ex.Message + " — " + Tail(r.All, 200)); }
        }

        void SaveCsv()
        {
            using (var d = new SaveFileDialog { Filter = "CSV|*.csv", FileName = "odr_triage.csv" })
            {
                if (d.ShowDialog(this) != DialogResult.OK) return;
                var sb = new System.Text.StringBuilder();
                sb.AppendLine(string.Join(",", lv.Columns.Cast<ColumnHeader>().Select(c => Q(c.Text))));
                foreach (ListViewItem it in lv.Items) sb.AppendLine(string.Join(",", it.SubItems.Cast<ListViewItem.ListViewSubItem>().Select(s => Q(s.Text))));
                File.WriteAllText(d.FileName, sb.ToString(), new System.Text.UTF8Encoding(true));
                Say("حُفظ: " + d.FileName);
            }
        }

        static string FindingAr(string code)
        {
            // PascalCase from triage -> same titles as the diagnose page
            var f = new Finding { Code = System.Text.RegularExpressions.Regex.Replace(code ?? "", "(?<=[a-z])([A-Z])", "_$1").ToUpperInvariant() };
            return f.TitleAr;
        }
        static string Q(string s) => "\"" + (s ?? "").Replace("\"", "\"\"") + "\"";
        static string S(Dictionary<string, object> d, string k) { object v; return d != null && d.TryGetValue(k, out v) && v != null ? Convert.ToString(v) : ""; }
        static long L(Dictionary<string, object> d, params string[] keys) { foreach (var k in keys) { object v; if (d != null && d.TryGetValue(k, out v) && v != null && !(v is Dictionary<string, object>)) { try { return Convert.ToInt64(v); } catch { } } } return 0; }
        static string Tail(string s, int n) { s = (s ?? "").Trim(); return s.Length <= n ? s : "…" + s.Substring(s.Length - n); }
    }
}
