// DiagnosePage.cs - "What happened to this file?" One .dbf in, a score + explained findings out, HTML/DB3F reports.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace OdrApp.Ui
{
    public sealed class DiagnosePage : UserControl
    {
        readonly TextBox txtFile = new TextBox();
        readonly Button btnBrowse = Theme.Secondary("📂 اختيار ملف"), btnRun = Theme.Primary("🩺 تشخيص", 150), btnFast = Theme.Secondary("⚡ سريع (مادي فقط)");
        readonly Button btnHtml = Theme.Secondary("📄 حفظ تقرير HTML"), btnDb3f = Theme.Secondary("🧾 حفظ DB3F"), btnJson = Theme.Secondary("{ } حفظ JSON"), btnCopy = Theme.Secondary("📋 نسخ الملخص"), btnCancel = Theme.Danger("إيقاف");
        readonly ScoreGauge gauge = new ScoreGauge();
        readonly Label lblVerdict = new Label(), lblFacts = new Label(), lblStatus = new Label();
        readonly FlowLayoutPanel findings = new FlowLayoutPanel();
        readonly ListView lvDamage = Theme.List(Theme.Col("نوع التلف", 200), Theme.Col("كتل", 90), Theme.Col("الحجم", 100));
        readonly ListView lvSteps = Theme.List(Theme.Col("الخطوة التالية", 420), Theme.Col("الأمر", 500));
        readonly ListView lvRegions = Theme.List(Theme.Col("من كتلة", 100), Theme.Col("عدد", 80), Theme.Col("النوع", 160));
        readonly ProgressBar bar = new ProgressBar { Style = ProgressBarStyle.Marquee, Visible = false, Width = 220, Height = 10, MarqueeAnimationSpeed = 30 };
        public Diagnosis Current;
        public event Action<string> Status;

        public DiagnosePage()
        {
            Dock = DockStyle.Fill; BackColor = Theme.Bg; RightToLeft = RightToLeft.Yes; Font = Theme.Base; Padding = new Padding(12);

            var top = new TableLayoutPanel { Dock = DockStyle.Top, AutoSize = true, ColumnCount = 1 };
            var head = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top, Padding = new Padding(0, 0, 0, 6) };
            var info = Theme.Info(); info.Click += (s, e) => Help.Show(FindForm(), "التشخيص", Help.Diagnose);
            head.Controls.Add(Theme.Title("🩺 التشخيص — ماذا حدث لهذا الملف؟")); head.Controls.Add(info);
            head.Controls.Add(Theme.MutedLabel("يفحص كل كتلة، يحسب درجة الاسترجاع، ويشرح السبب والعلاج. للقراءة فقط."));

            var row = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top };
            txtFile.Width = 520; txtFile.RightToLeft = RightToLeft.No; txtFile.Margin = new Padding(4, 7, 4, 4); txtFile.Font = Theme.Mono;
            txtFile.AllowDrop = true; txtFile.DragEnter += (s, e) => { if (e.Data.GetDataPresent(DataFormats.FileDrop)) e.Effect = DragDropEffects.Copy; };
            txtFile.DragDrop += (s, e) => { var f = (string[])e.Data.GetData(DataFormats.FileDrop); if (f.Length > 0) txtFile.Text = f[0]; };
            row.Controls.AddRange(new Control[] { txtFile, btnBrowse, btnRun, btnFast, btnCancel, bar });
            btnCancel.Enabled = false;
            var row2 = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top };
            row2.Controls.AddRange(new Control[] { btnHtml, btnDb3f, btnJson, btnCopy });
            foreach (var b in new[] { btnHtml, btnDb3f, btnJson, btnCopy }) b.Enabled = false;
            top.Controls.Add(head); top.Controls.Add(row); top.Controls.Add(row2);

            // summary card: gauge + verdict + facts
            var sum = new TableLayoutPanel { ColumnCount = 3, Dock = DockStyle.Fill, Padding = new Padding(6) };
            sum.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 170)); sum.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 55)); sum.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 45));
            gauge.Size = new Size(150, 150); gauge.BackColor = Color.White; gauge.Margin = new Padding(6);
            lblVerdict.AutoSize = true; lblVerdict.MaximumSize = new Size(520, 0); lblVerdict.Font = new Font("Segoe UI", 11.5f); lblVerdict.ForeColor = Theme.Text; lblVerdict.Margin = new Padding(8);
            lblVerdict.Text = "اختر ملف .dbf ثم اضغط «تشخيص».";
            lblFacts.AutoSize = true; lblFacts.Font = Theme.Base; lblFacts.ForeColor = Theme.Muted; lblFacts.Margin = new Padding(8);
            sum.Controls.Add(gauge, 0, 0); sum.Controls.Add(lblVerdict, 1, 0); sum.Controls.Add(lblFacts, 2, 0);

            // findings
            findings.Dock = DockStyle.Fill; findings.AutoScroll = true; findings.FlowDirection = FlowDirection.TopDown; findings.WrapContents = false; findings.BackColor = Color.White;
            findings.Resize += (s, e) => { foreach (Control c in findings.Controls) c.Width = findings.ClientSize.Width - 24; };

            // right column: damage classes + regions + steps
            var right = new TableLayoutPanel { Dock = DockStyle.Fill, RowCount = 3, ColumnCount = 1 };
            right.RowStyles.Add(new RowStyle(SizeType.Percent, 34)); right.RowStyles.Add(new RowStyle(SizeType.Percent, 33)); right.RowStyles.Add(new RowStyle(SizeType.Percent, 33));
            right.Controls.Add(Section("أنواع التلف", lvDamage), 0, 0);
            right.Controls.Add(Section("مناطق التلف (متتالية)", lvRegions), 0, 1);
            right.Controls.Add(Section("الخطوات التالية — انقر مرتين لنسخ الأمر", lvSteps), 0, 2);
            lvSteps.DoubleClick += (s, e) => { if (lvSteps.SelectedItems.Count == 1 && lvSteps.SelectedItems[0].SubItems[1].Text != "") { Clipboard.SetText(lvSteps.SelectedItems[0].SubItems[1].Text); Say("نُسخ الأمر."); } };

            var body = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 2, RowCount = 2 };
            body.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 58)); body.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 42));
            body.RowStyles.Add(new RowStyle(SizeType.Absolute, 180)); body.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            body.Controls.Add(Theme.Card(sum, 4), 0, 0); body.SetColumnSpan(body.GetControlFromPosition(0, 0), 2);
            body.Controls.Add(Section("النتائج — ما حدث، الدليل، السبب، وما يمكن إنقاذه", findings), 0, 1);
            body.Controls.Add(right, 1, 1);

            lblStatus.Dock = DockStyle.Bottom; lblStatus.Height = 26; lblStatus.ForeColor = Theme.Muted; lblStatus.TextAlign = ContentAlignment.MiddleRight;
            Controls.Add(body); Controls.Add(lblStatus); Controls.Add(top);

            btnBrowse.Click += (s, e) => { using (var d = new OpenFileDialog { Title = "اختر ملف قاعدة بيانات", Filter = "ملفات أوراكل (*.dbf;*.ora;*.dat)|*.dbf;*.ora;*.dat|كل الملفات|*.*" }) if (d.ShowDialog(this) == DialogResult.OK) txtFile.Text = d.FileName; };
            btnRun.Click += async (s, e) => await RunAsync(false);
            btnFast.Click += async (s, e) => await RunAsync(true);
            btnCancel.Click += (s, e) => OdrTool.Cancel();
            btnHtml.Click += async (s, e) => await ExportAsync("html", "تقرير HTML|*.html");
            btnDb3f.Click += async (s, e) => await ExportAsync("db3f", "DB3F|*.db3f");
            btnJson.Click += (s, e) => { if (Current == null) return; using (var d = new SaveFileDialog { Filter = "JSON|*.json", FileName = Path.GetFileNameWithoutExtension(Current.File) + "_diagnose.json" }) if (d.ShowDialog(this) == DialogResult.OK) { File.WriteAllText(d.FileName, Current.Raw); Say("حُفظ: " + d.FileName); } };
            btnCopy.Click += (s, e) => { if (Current != null) { Clipboard.SetText(Summary(Current)); Say("نُسخ الملخص."); } };
        }

        static Control Section(string title, Control content)
        {
            var p = new TableLayoutPanel { Dock = DockStyle.Fill, RowCount = 2, ColumnCount = 1, Margin = new Padding(4) };
            p.RowStyles.Add(new RowStyle(SizeType.AutoSize)); p.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            p.Controls.Add(new Label { Text = title, Font = Theme.Bold, ForeColor = Theme.Navy, AutoSize = true, Margin = new Padding(6, 6, 6, 2) }, 0, 0);
            p.Controls.Add(content, 0, 1);
            return Theme.Card(p, 2);
        }

        void Say(string s) { lblStatus.Text = s; Status?.Invoke(s); }

        public void SetFile(string path) { txtFile.Text = path; }

        void Busy(bool b)
        {
            foreach (var c in new Control[] { btnBrowse, btnRun, btnFast }) c.Enabled = !b;
            btnCancel.Enabled = b; bar.Visible = b; UseWaitCursor = b;
        }

        public async Task<bool> RunAsync(bool fast)
        {
            var f = txtFile.Text.Trim().Trim('"');
            if (f == "" || !File.Exists(f)) { Say("اختر ملفاً موجوداً أولاً."); return false; }
            Busy(true); Say("جارٍ فحص كل كتلة في " + Path.GetFileName(f) + " …");
            var args = new List<string> { "diagnose", f, "--json", "--lang", "ar" };
            if (fast) args.Add("--fast");
            var r = await OdrTool.RunAsync(OdrTool.Odr, args);
            Busy(false);
            if (r.Cancelled) { Say("أُوقف."); return false; }
            if (r.Code != 0 && r.Code != 2) { Say("تعذّر التشخيص: " + Tail(r.All, 300)); return false; }
            try { Current = Diagnosis.Parse(r.Out); }
            catch (Exception ex) { Say("تعذّر فهم المخرجات: " + ex.Message); return false; }
            Render(Current);
            foreach (var b in new[] { btnHtml, btnDb3f, btnJson, btnCopy }) b.Enabled = true;
            Say($"انتهى: الدرجة {Current.Score}/100 ({Theme.BandAr(Current.Band)}) · {Current.DamagedTotal:N0} كتلة تالفة من {Current.Examined:N0}.");
            return true;
        }

        void Render(Diagnosis d)
        {
            gauge.Set(d.Score, d.Band);
            lblVerdict.Text = d.VerdictAr;
            lblFacts.Text =
                $"الملف: {Path.GetFileName(d.File)}\n" +
                $"الحجم: {Diagnosis.Bytes(d.FileLen)} · الكتلة: {d.BlockSize:N0} بايت · المصدر: {LayoutAr(d.LayoutSource)}\n" +
                $"كتل مفحوصة: {d.Examined:N0} · بيانات {d.Data:N0} · فهارس {d.Index:N0} · LOB {d.Lob:N0} · فارغة {d.Empty:N0}\n" +
                $"تالفة: {d.DamagedTotal:N0} ({(d.Examined > 0 ? d.DamagedTotal * 100.0 / d.Examined : 0):0.#}%) · الخسارة المتوقعة: {d.ExpectedLossMilli / 10.0:0.#}% ≈ {Diagnosis.Bytes(d.BytesAffected)}\n" +
                (d.LiveRows.HasValue ? $"صفوف حيّة مقروءة: {d.LiveRows:N0} · " : "") + $"كائنات: {d.ObjectCount:N0}" +
                (d.ChecksumCoverageMilli >= 0 && d.ChecksumCoverageMilli < 1000 ? $"\n⚠ تغطية checksum {d.ChecksumCoverageMilli / 10.0:0.#}% فقط" : "") +
                $"\nSHA-256: {d.Sha256.Substring(0, Math.Min(16, d.Sha256.Length))}… {(d.Unchanged ? "(لم يتغيّر الملف أثناء الفحص ✔)" : "")}";

            findings.SuspendLayout(); findings.Controls.Clear();
            foreach (var f in d.Findings) findings.Controls.Add(FindingCard(f, findings.ClientSize.Width - 24));
            findings.ResumeLayout();

            lvDamage.Items.Clear();
            foreach (var kv in d.Damaged.OrderByDescending(x => x.Value))
            { var it = new ListViewItem(Diagnosis.DamageAr(kv.Key)); it.SubItems.Add(kv.Value.ToString("N0")); it.SubItems.Add(Diagnosis.Bytes(kv.Value * d.BlockSize)); lvDamage.Items.Add(it); }
            if (d.Damaged.Count == 0) lvDamage.Items.Add(new ListViewItem("لا تلف ✔") { ForeColor = Theme.Ok });

            lvRegions.Items.Clear();
            foreach (var r in d.Regions.OrderByDescending(x => x.Item2).Take(500))
            { var it = new ListViewItem(r.Item1.ToString("N0")); it.SubItems.Add(r.Item2.ToString("N0")); it.SubItems.Add(Diagnosis.DamageAr(r.Item3)); lvRegions.Items.Add(it); }

            lvSteps.Items.Clear();
            foreach (var s in d.NextSteps) { var it = new ListViewItem(s.PurposeAr); it.SubItems.Add(s.Command); lvSteps.Items.Add(it); }
        }

        static Control FindingCard(Finding f, int width)
        {
            var p = new BorderPanel { Width = Math.Max(300, width), AutoSize = true, BackColor = Theme.SeverityBg(f.Severity), Padding = new Padding(10), Margin = new Padding(6) };
            var t = new TableLayoutPanel { ColumnCount = 1, AutoSize = true, Dock = DockStyle.Top };
            var title = new Label { Text = $"{f.TitleAr}   ·   {Theme.SeverityAr(f.Severity)}   ·   {f.Blocks:N0} كتلة   ·   ثقة {f.Confidence / 10}%", Font = Theme.Bold, ForeColor = Theme.Navy, AutoSize = true };
            t.Controls.Add(title);
            t.Controls.Add(Para("ما حدث:", f.CauseAr, width - 40));
            if (f.Evidence.Count > 0) t.Controls.Add(Para("الدليل:", string.Join("\n", f.Evidence.Select(e => "• " + e)), width - 40, mono: true));
            if (f.Parameters.Count > 0) t.Controls.Add(Para("المعاملات:", string.Join(" · ", f.Parameters.Select(kv => kv.Key + " = " + kv.Value.ToString("N0"))), width - 40, mono: true));
            t.Controls.Add(Para("الاسترجاع:", f.RecoveryAr, width - 40));
            if (f.OracleErrors.Count > 0) t.Controls.Add(Para("أخطاء أوراكل المتوقعة:", string.Join("  ", f.OracleErrors), width - 40, mono: true));
            p.Controls.Add(t);
            return p;
        }

        static Control Para(string head, string text, int width, bool mono = false)
        {
            var fl = new FlowLayoutPanel { AutoSize = true, FlowDirection = FlowDirection.TopDown, Margin = new Padding(0, 4, 0, 0), WrapContents = false };
            fl.Controls.Add(new Label { Text = head, Font = Theme.Small, ForeColor = Theme.Muted, AutoSize = true });
            fl.Controls.Add(new Label { Text = text, Font = mono ? Theme.Mono : Theme.Base, ForeColor = Theme.Text, AutoSize = true, MaximumSize = new Size(Math.Max(200, width), 0), RightToLeft = mono ? RightToLeft.No : RightToLeft.Yes });
            return fl;
        }

        async Task ExportAsync(string kind, string filter)
        {
            if (Current == null) return;
            using (var d = new SaveFileDialog { Filter = filter, FileName = Path.GetFileNameWithoutExtension(Current.File) + "_report." + kind })
            {
                if (d.ShowDialog(this) != DialogResult.OK) return;
                Busy(true); Say("جارٍ إنشاء التقرير…");
                var args = new List<string> { "diagnose", Current.File, "--lang", "ar", "--" + kind, d.FileName };
                var r = await OdrTool.RunAsync(OdrTool.Odr, args);
                Busy(false);
                if (!File.Exists(d.FileName)) { Say("تعذّر إنشاء التقرير: " + Tail(r.All, 200)); return; }
                Say("حُفظ: " + d.FileName);
                if (kind == "html") try { Process.Start(new ProcessStartInfo(d.FileName) { UseShellExecute = true }); } catch { }
            }
        }

        public static string Summary(Diagnosis d)
        {
            var sb = new System.Text.StringBuilder();
            sb.AppendLine($"ODR — تشخيص {Path.GetFileName(d.File)}");
            sb.AppendLine($"الدرجة: {d.Score}/100 ({Theme.BandAr(d.Band)}) · الخسارة المتوقعة {d.ExpectedLossMilli / 10.0:0.#}%");
            sb.AppendLine(d.VerdictAr);
            sb.AppendLine($"كتل: {d.Examined:N0} مفحوصة · {d.DamagedTotal:N0} تالفة · حجم الكتلة {d.BlockSize}");
            foreach (var f in d.Findings) sb.AppendLine($"- {f.TitleAr} [{Theme.SeverityAr(f.Severity)}، {f.Blocks:N0} كتلة]: {f.CauseAr}");
            sb.AppendLine("SHA-256: " + d.Sha256);
            return sb.ToString();
        }

        static string LayoutAr(string s)
        {
            switch (s) { case "file_header": return "رأس الملف"; case "os_block": return "الكتلة 0"; case "inferred": return "مستنتج من الكتل"; case "forced": return "مفروض"; default: return s; }
        }
        static string Tail(string s, int n) { s = (s ?? "").Trim(); return s.Length <= n ? s : "…" + s.Substring(s.Length - n); }
    }
}
