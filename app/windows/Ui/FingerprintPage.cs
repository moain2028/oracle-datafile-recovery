// FingerprintPage.cs - name OBJ_12345 when SYSTEM is gone: learn a book from a healthy DB, match a scavenge report.
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
    public sealed class FingerprintPage : UserControl
    {
        // learn
        readonly ListBox lstHealthy = new ListBox();
        readonly TextBox txtProduct = new TextBox { Text = "yemensoft", Width = 160 }, txtOwners = new TextBox { Width = 220 }, txtBook = new TextBox { Width = 420 };
        readonly Button btnAddHealthy = Theme.Secondary("➕ ملفات قاعدة سليمة (مع SYSTEM)"), btnBook = Theme.Secondary("📂 مكان الكتاب"), btnLearn = Theme.Primary("📚 تعلُّم", 130);
        // match
        readonly TextBox txtReport = new TextBox { Width = 420 }, txtBook2 = new TextBox { Width = 420 };
        readonly Button btnReport = Theme.Secondary("📂 report.json من «بدون SYSTEM»"), btnBook2 = Theme.Secondary("📂 كتاب البصمات"), btnMatch = Theme.Primary("🔍 مطابقة", 130);
        readonly ListView lv = Theme.List(Theme.Col("الكائن", 120), Theme.Col("الاسم المقترح", 280), Theme.Col("الثقة %", 80), Theme.Col("أعمدة", 70), Theme.Col("بدائل", 420));
        readonly Label lblStatus = new Label();
        readonly ProgressBar bar = new ProgressBar { Style = ProgressBarStyle.Marquee, Visible = false, Width = 200, Height = 10 };
        public event Action<string> Status;

        public FingerprintPage()
        {
            Dock = DockStyle.Fill; BackColor = Theme.Bg; RightToLeft = RightToLeft.Yes; Font = Theme.Base; Padding = new Padding(12);
            var head = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top, Padding = new Padding(0, 0, 0, 6) };
            var info = Theme.Info(); info.Click += (s, e) => Help.Show(FindForm(), "البصمة", Help.Fingerprint);
            head.Controls.Add(Theme.Title("🔍 البصمة — تسمية الجداول حين يضيع القاموس")); head.Controls.Add(info);
            head.Controls.Add(Theme.MutedLabel("١) تعلّم من قاعدة سليمة لنفس المنتج  ←  ٢) طابق تقرير «استخراج بدون SYSTEM». اقتراحات لا حقائق."));

            foreach (var t in new[] { txtProduct, txtOwners, txtBook, txtReport, txtBook2 }) { t.RightToLeft = RightToLeft.No; t.Font = Theme.Mono; t.Margin = new Padding(4, 7, 4, 4); }
            txtBook.Text = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "ODR", "fingerprints", "yemensoft.book.json");
            txtBook2.Text = txtBook.Text;

            // learn card
            var learn = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 3 };
            learn.RowStyles.Add(new RowStyle(SizeType.AutoSize)); learn.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); learn.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            learn.Controls.Add(new Label { Text = "١) التعلُّم من قاعدة سليمة", Font = Theme.Bold, ForeColor = Theme.Navy, AutoSize = true, Margin = new Padding(6) }, 0, 0);
            lstHealthy.Dock = DockStyle.Fill; lstHealthy.RightToLeft = RightToLeft.No; lstHealthy.Font = Theme.Mono; lstHealthy.BorderStyle = BorderStyle.FixedSingle;
            learn.Controls.Add(lstHealthy, 0, 1);
            var lrow = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill };
            lrow.Controls.AddRange(new Control[] { btnAddHealthy, new Label { Text = "المنتج:", AutoSize = true, Margin = new Padding(6, 10, 0, 0) }, txtProduct,
                new Label { Text = "المالكون (اختياري، بفواصل):", AutoSize = true, Margin = new Padding(6, 10, 0, 0) }, txtOwners, txtBook, btnBook, btnLearn });
            learn.Controls.Add(lrow, 0, 2);

            // match card
            var match = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 3 };
            match.RowStyles.Add(new RowStyle(SizeType.AutoSize)); match.RowStyles.Add(new RowStyle(SizeType.AutoSize)); match.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            match.Controls.Add(new Label { Text = "٢) المطابقة", Font = Theme.Bold, ForeColor = Theme.Navy, AutoSize = true, Margin = new Padding(6) }, 0, 0);
            var mrow = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill };
            mrow.Controls.AddRange(new Control[] { txtReport, btnReport, txtBook2, btnBook2, btnMatch, bar });
            match.Controls.Add(mrow, 0, 1); match.Controls.Add(lv, 0, 2);

            var body = new TableLayoutPanel { Dock = DockStyle.Fill, RowCount = 2, ColumnCount = 1 };
            body.RowStyles.Add(new RowStyle(SizeType.Percent, 38)); body.RowStyles.Add(new RowStyle(SizeType.Percent, 62));
            body.Controls.Add(Theme.Card(learn, 6), 0, 0); body.Controls.Add(Theme.Card(match, 6), 0, 1);
            lblStatus.Dock = DockStyle.Bottom; lblStatus.Height = 26; lblStatus.ForeColor = Theme.Muted; lblStatus.TextAlign = ContentAlignment.MiddleRight;
            Controls.Add(body); Controls.Add(lblStatus); Controls.Add(head);

            btnAddHealthy.Click += (s, e) => { using (var d = new OpenFileDialog { Multiselect = true, Filter = "ملفات أوراكل (*.dbf)|*.dbf|كل الملفات|*.*" }) if (d.ShowDialog(this) == DialogResult.OK) foreach (var f in d.FileNames) if (!lstHealthy.Items.Contains(f)) lstHealthy.Items.Add(f); };
            btnBook.Click += (s, e) => { using (var d = new SaveFileDialog { Filter = "كتاب بصمات|*.book.json", FileName = Path.GetFileName(txtBook.Text) }) if (d.ShowDialog(this) == DialogResult.OK) { txtBook.Text = d.FileName; txtBook2.Text = d.FileName; } };
            btnBook2.Click += (s, e) => { using (var d = new OpenFileDialog { Filter = "كتاب بصمات|*.json" }) if (d.ShowDialog(this) == DialogResult.OK) txtBook2.Text = d.FileName; };
            btnReport.Click += (s, e) => { using (var d = new OpenFileDialog { Filter = "report.json|report.json;*.json" }) if (d.ShowDialog(this) == DialogResult.OK) txtReport.Text = d.FileName; };
            btnLearn.Click += async (s, e) => await LearnAsync();
            btnMatch.Click += async (s, e) => await MatchAsync();
        }

        void Say(string s) { lblStatus.Text = s; Status?.Invoke(s); }
        void Busy(bool b) { foreach (var c in new Control[] { btnLearn, btnMatch, btnAddHealthy }) c.Enabled = !b; bar.Visible = b; UseWaitCursor = b; }

        async Task LearnAsync()
        {
            if (lstHealthy.Items.Count == 0) { Say("أضف ملفات القاعدة السليمة أولاً (يجب أن يكون بينها SYSTEM)."); return; }
            Busy(true); Say("جارٍ قراءة قاموس القاعدة السليمة…");
            var dictArgs = new List<string> { "dict", "--json" }; dictArgs.AddRange(lstHealthy.Items.Cast<string>());
            var r = await OdrTool.RunAsync(OdrTool.Odr, dictArgs);
            if (r.Code != 0) { Busy(false); Say("تعذّرت قراءة القاموس: " + (OdrTool.Explain(r.All) != "" ? OdrTool.Explain(r.All) : Tail(r.All, 200))); return; }
            var tmp = Path.Combine(Path.GetTempPath(), "odr_dict_" + Guid.NewGuid().ToString("N") + ".json");
            File.WriteAllText(tmp, r.Out);
            Directory.CreateDirectory(Path.GetDirectoryName(txtBook.Text));
            var args = new List<string> { "fingerprint", "learn", tmp, "--out", txtBook.Text, "--product", txtProduct.Text.Trim() == "" ? "unnamed" : txtProduct.Text.Trim() };
            foreach (var o in txtOwners.Text.Split(new[] { ',', ' ', ';' }, StringSplitOptions.RemoveEmptyEntries)) { args.Add("--owner"); args.Add(o.ToUpperInvariant()); }
            var r2 = await OdrTool.RunAsync(OdrTool.Odr, args);
            try { File.Delete(tmp); } catch { }
            Busy(false);
            if (r2.Code != 0) { Say("تعذّر بناء الكتاب: " + Tail(r2.All, 200)); return; }
            txtBook2.Text = txtBook.Text;
            Say("تم إنشاء كتاب البصمات: " + txtBook.Text + "   " + Tail(r2.All, 120));
        }

        async Task MatchAsync()
        {
            if (!File.Exists(txtReport.Text)) { Say("اختر report.json الناتج عن «استخراج بدون SYSTEM»."); return; }
            if (!File.Exists(txtBook2.Text)) { Say("اختر كتاب بصمات موجوداً (أو أنشئه في الخطوة ١)."); return; }
            Busy(true); Say("جارٍ المطابقة…");
            var r = await OdrTool.RunAsync(OdrTool.Odr, new[] { "fingerprint", "match", txtReport.Text, "--book", txtBook2.Text, "--json" });
            Busy(false);
            if (r.Code != 0) { Say("تعذّرت المطابقة: " + Tail(r.All, 200)); return; }
            lv.Items.Clear();
            try
            {
                var js = new JavaScriptSerializer { MaxJsonLength = int.MaxValue };
                var root = js.DeserializeObject(r.Out);
                var rows = root as object[] ?? ((root as Dictionary<string, object>)?.Values.FirstOrDefault(v => v is object[]) as object[]) ?? new object[0];
                foreach (Dictionary<string, object> m in rows)
                {
                    string obj = Str(m, "objd", "object", "objd_name"); if (obj == "") obj = Str(m, "name");
                    var cands = (m.ContainsKey("candidates") ? m["candidates"] : m.ContainsKey("matches") ? m["matches"] : null) as object[] ?? new object[0];
                    string best = Str(m, "best", "table", "suggested"); long conf = Lng(m, "confidence_milli", "confidence", "score_milli");
                    if (best == "" && cands.Length > 0 && cands[0] is Dictionary<string, object> c0) { best = Str(c0, "table", "name"); conf = Lng(c0, "confidence_milli", "score_milli", "confidence"); }
                    var it = new ListViewItem(obj.StartsWith("OBJ_") ? obj : "OBJ_" + obj);
                    it.SubItems.Add(best == "" ? "— لا اقتراح —" : best); it.SubItems.Add(conf > 0 ? (conf / 10.0).ToString("0") : "");
                    it.SubItems.Add(Lng(m, "columns", "ncols").ToString());
                    it.SubItems.Add(string.Join(" · ", cands.Skip(1).Take(3).OfType<Dictionary<string, object>>().Select(c => Str(c, "table", "name") + " " + (Lng(c, "confidence_milli", "score_milli", "confidence") / 10.0).ToString("0") + "%")));
                    it.BackColor = conf >= 900 ? Theme.OkBg : conf >= 600 ? Theme.WarnBg : Color.White;
                    lv.Items.Add(it);
                }
                Say($"{lv.Items.Count} كائن — الأخضر ثقة ≥ 90%، الأصفر ≥ 60%. اقتراحات تحتاج تأكيدك.");
            }
            catch (Exception ex) { Say("تعذّر فهم المخرجات: " + ex.Message); }
        }

        static string Str(Dictionary<string, object> d, params string[] keys) { foreach (var k in keys) { object v; if (d.TryGetValue(k, out v) && v != null && !(v is object[]) && !(v is Dictionary<string, object>)) return Convert.ToString(v); } return ""; }
        static long Lng(Dictionary<string, object> d, params string[] keys) { foreach (var k in keys) { object v; if (d.TryGetValue(k, out v) && v != null) { try { var x = Convert.ToDouble(v); return x <= 1.0 && k.Contains("confidence") && !k.Contains("milli") ? (long)(x * 1000) : (long)x; } catch { } } } return 0; }
        static string Tail(string s, int n) { s = (s ?? "").Trim(); return s.Length <= n ? s : "…" + s.Substring(s.Length - n); }
    }
}
