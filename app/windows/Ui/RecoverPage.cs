// RecoverPage.cs - the recovery flow (the whole window before v0.7). Three steps, no typing:
//   1. add the .dbf files   2. read the tables   3. extract the chosen tables -> results + preview.
// Public members (ReadTablesAsync, ExtractAsync, CheckedTables, Results, DictIssues, VisibleTables,
// ShowPreview, AddDemoFiles) are what MainForm.AutoTest drives for the CI proof.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace OdrApp.Ui
{
    public sealed class TableInfo
    {
        public string Owner = "", Name = "", Tablespace = "";
        public int Columns;
        public string Full => Owner + "." + Name;
    }

    public sealed class ExtractResult
    {
        public string Table = "", Folder = "", Status = "", Message = "";
        public Layers Layers = new Layers();
        public int Code;
    }

    public sealed class RecoverPage : UserControl
    {
        static readonly string[] SysOwnerList = {
            "SYS", "SYSTEM", "AUDSYS", "CTXSYS", "DBSFWUSER", "DBSNMP", "DVSYS", "DVF", "GSMADMIN_INTERNAL",
            "GSMCATUSER", "GSMUSER", "LBACSYS", "MDSYS", "MDDATA", "OJVMSYS", "OLAPSYS", "OUTLN", "VECSYS",
            "WMSYS", "XDB", "APPQOSSYS", "ORDSYS", "ORDDATA", "ORDPLUGINS", "EXFSYS", "WKSYS", "WK_TEST",
            "ANONYMOUS", "SI_INFORMTN_SCHEMA", "ORACLE_OCM", "XS$NULL", "DIP", "REMOTE_SCHEDULER_AGENT",
            "SYSBACKUP", "SYSDG", "SYSKM", "SYSRAC", "DGPDB_INT", "GGSYS", "SYS$UMF", "SPATIAL_CSW_ADMIN_USR",
            "SPATIAL_WFS_ADMIN_USR", "MGMT_VIEW", "SYSMAN", "OWBSYS", "OWBSYS_AUDIT", "PUBLIC", "FLOWS_FILES" };
        static readonly HashSet<string> SysOwners = new HashSet<string>(SysOwnerList);
        static bool IsSystemOwner(string o) => SysOwners.Contains(o) || o.StartsWith("APEX_") || o.StartsWith("FLOWS_");

        readonly bool autotest;
        public event Action<string> Status;
        public event Action<string> OpenDiagnosis;
        public string DefaultOutRoot = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "ODR");

        readonly ListBox lstFiles = new ListBox();
        readonly Button btnAdd = new Button(), btnRemove = new Button(), btnDemo = new Button(), btnRead = new Button();
        readonly Button btnExtract = new Button(), btnScavenge = new Button(), btnOpenOut = new Button();
        readonly Button btnBrowseOut = new Button(), btnCancel = new Button(), btnAll = new Button(), btnNone = new Button(), btnDiag = new Button();
        readonly CheckBox chkHideSys = new CheckBox();
        readonly TextBox txtFilter = new TextBox(), txtOut = new TextBox();
        readonly ListView lvTables = new ListView(), lvResults = new ListView();
        readonly ProgressBar bar = new ProgressBar();
        readonly Label lblStatus = new Label(), lblDb = new Label();

        readonly List<TableInfo> tables = new List<TableInfo>();
        public int DictIssues = -1;
        public readonly List<ExtractResult> Results = new List<ExtractResult>();
        bool busy;

        public RecoverPage(bool autotest)
        {
            this.autotest = autotest;
            Dock = DockStyle.Fill; BackColor = Theme.Bg; Font = Theme.Base;
            RightToLeft = RightToLeft.Yes;
            BuildUi();
            txtOut.Text = Path.Combine(DefaultOutRoot, DateTime.Now.ToString("yyyy-MM-dd_HH-mm"));
            SetStatus(OdrTool.Odr == null
                ? "تنبيه: محرك odr.exe غير موجود — أعد تثبيت البرنامج."
                : "ابدأ بإضافة ملفات قاعدة البيانات (.dbf) — يجب أن يكون بينها ملف SYSTEM.");
        }

        // ------------------------------------------------------------------ layout
        static Button Btn(Button b, string text, int w = 150)
        {
            b.Text = text; b.Width = w; b.Height = 34; b.Margin = new Padding(4); b.UseVisualStyleBackColor = true;
            return b;
        }

        static GroupBox Group(string title, Control content)
        {
            var g = new GroupBox { Text = title, Dock = DockStyle.Fill, Padding = new Padding(8), Font = new Font("Segoe UI", 10.5f, FontStyle.Bold), ForeColor = Theme.Navy };
            content.Font = new Font("Segoe UI", 10f);
            content.Dock = DockStyle.Fill;
            g.Controls.Add(content);
            return g;
        }

        void BuildUi()
        {
            // step 1: files
            var p1 = new TableLayoutPanel { ColumnCount = 1, RowCount = 2 };
            p1.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); p1.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            lstFiles.HorizontalScrollbar = true; lstFiles.Dock = DockStyle.Fill; lstFiles.RightToLeft = RightToLeft.No;
            lstFiles.SelectionMode = SelectionMode.MultiExtended;
            var f1 = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill, WrapContents = true };
            f1.Controls.AddRange(new Control[] { Btn(btnAdd, "➕ إضافة ملفات .dbf", 170), Btn(btnRemove, "إزالة المحدد", 120), Btn(btnDemo, "تجربة بملفات العرض", 170), Btn(btnDiag, "🩺 تشخيص الملف المحدد", 180) });
            p1.Controls.Add(lstFiles, 0, 0); p1.Controls.Add(f1, 0, 1);

            // step 2: tables
            var p2 = new TableLayoutPanel { ColumnCount = 1, RowCount = 3 };
            p2.RowStyles.Add(new RowStyle(SizeType.AutoSize)); p2.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); p2.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            var f2 = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill };
            chkHideSys.Text = "إخفاء جداول النظام"; chkHideSys.Checked = true; chkHideSys.AutoSize = true; chkHideSys.Margin = new Padding(8, 10, 8, 4);
            var lf = new Label { Text = "بحث:", AutoSize = true, Margin = new Padding(8, 10, 0, 4) };
            txtFilter.Width = 200; txtFilter.Margin = new Padding(4, 6, 4, 4);
            f2.Controls.AddRange(new Control[] { Btn(btnRead, "📖 قراءة الجداول", 160), chkHideSys, lf, txtFilter, Btn(btnAll, "تحديد الكل", 100), Btn(btnNone, "إلغاء التحديد", 110) });
            lvTables.View = View.Details; lvTables.CheckBoxes = true; lvTables.FullRowSelect = true; lvTables.Dock = DockStyle.Fill;
            lvTables.RightToLeftLayout = true; lvTables.GridLines = true;
            lvTables.Columns.Add("الجدول", 320); lvTables.Columns.Add("مساحة الجداول", 160); lvTables.Columns.Add("الأعمدة", 80);
            lblDb.AutoSize = true; lblDb.Margin = new Padding(6); lblDb.ForeColor = Color.FromArgb(40, 40, 40);
            p2.Controls.Add(f2, 0, 0); p2.Controls.Add(lvTables, 0, 1); p2.Controls.Add(lblDb, 0, 2);

            // step 3: extract + results
            var p3 = new TableLayoutPanel { ColumnCount = 1, RowCount = 3 };
            p3.RowStyles.Add(new RowStyle(SizeType.AutoSize)); p3.RowStyles.Add(new RowStyle(SizeType.AutoSize)); p3.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            var f3a = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill };
            var lo = new Label { Text = "حفظ النتائج في:", AutoSize = true, Margin = new Padding(4, 10, 0, 4) };
            txtOut.Width = 420; txtOut.Margin = new Padding(4, 6, 4, 4); txtOut.RightToLeft = RightToLeft.No;
            f3a.Controls.AddRange(new Control[] { lo, txtOut, Btn(btnBrowseOut, "تغيير...", 90), Btn(btnOpenOut, "📂 فتح مجلد النتائج", 170) });
            var f3b = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill };
            btnExtract.BackColor = Theme.Gold; btnExtract.ForeColor = Color.FromArgb(24, 24, 24); btnExtract.FlatStyle = FlatStyle.Flat; btnExtract.FlatAppearance.BorderSize = 0; btnExtract.Font = new Font("Segoe UI", 10.5f, FontStyle.Bold);
            bar.Width = 260; bar.Height = 22; bar.Margin = new Padding(8, 10, 8, 4);
            f3b.Controls.AddRange(new Control[] { Btn(btnExtract, "▶ استخراج الجداول المحددة", 230), Btn(btnScavenge, "استخراج بدون ملف SYSTEM", 210), Btn(btnCancel, "إيقاف", 80), bar });
            lvResults.View = View.Details; lvResults.FullRowSelect = true; lvResults.GridLines = true; lvResults.Dock = DockStyle.Fill; lvResults.RightToLeftLayout = true;
            lvResults.Columns.Add("الجدول", 260); lvResults.Columns.Add("موثَّق ✔", 100); lvResults.Columns.Add("مستنتج", 90);
            lvResults.Columns.Add("مرفوض", 90); lvResults.Columns.Add("الحالة", 420);
            p3.Controls.Add(f3a, 0, 0); p3.Controls.Add(f3b, 0, 1); p3.Controls.Add(lvResults, 0, 2);

            var grid = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 2, RowCount = 2, Padding = new Padding(8) };
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 42)); grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 58));
            grid.RowStyles.Add(new RowStyle(SizeType.Percent, 42)); grid.RowStyles.Add(new RowStyle(SizeType.Percent, 58));
            grid.Controls.Add(Group("١) ملفات قاعدة البيانات", p1), 0, 0);
            grid.Controls.Add(Group("٢) الجداول", p2), 1, 0);
            grid.Controls.Add(Group("٣) الاستخراج والنتائج — انقر مرتين على جدول لعرض بياناته", p3), 0, 1);
            grid.SetColumnSpan(grid.GetControlFromPosition(0, 1), 2);

            lblStatus.Dock = DockStyle.Bottom; lblStatus.Height = 26; lblStatus.ForeColor = Theme.Muted; lblStatus.TextAlign = ContentAlignment.MiddleRight;
            var head = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top, Padding = new Padding(8, 8, 8, 0) };
            var info = Theme.Info(); info.Click += (s, e) => Help.Show(FindForm(), "الاسترجاع", Help.Recover);
            head.Controls.Add(Theme.Title("📦 الاسترجاع — استخراج الجداول من ملفات .dbf")); head.Controls.Add(info);
            head.Controls.Add(Theme.MutedLabel("ثلاث خطوات: الملفات ← الجداول ← الاستخراج. كل صف موثَّق أو مستنتج أو مرفوض."));
            btnDiag.Click += (s, e) => { if (lstFiles.SelectedItem is string p) OpenDiagnosis?.Invoke(p); else if (lstFiles.Items.Count > 0) OpenDiagnosis?.Invoke((string)lstFiles.Items[0]); };
            Controls.Add(grid); Controls.Add(lblStatus); Controls.Add(head);

            btnAdd.Click += (s, e) => AddFiles();
            btnRemove.Click += (s, e) => { foreach (var i in lstFiles.SelectedItems.Cast<object>().ToList()) lstFiles.Items.Remove(i); };
            btnDemo.Click += (s, e) => AddDemo();
            btnRead.Click += async (s, e) => await ReadTablesAsync();
            chkHideSys.CheckedChanged += (s, e) => FillTables();
            txtFilter.TextChanged += (s, e) => FillTables();
            btnAll.Click += (s, e) => { foreach (ListViewItem i in lvTables.Items) i.Checked = true; };
            btnNone.Click += (s, e) => { foreach (ListViewItem i in lvTables.Items) i.Checked = false; };
            btnBrowseOut.Click += (s, e) => { using (var d = new FolderBrowserDialog { SelectedPath = txtOut.Text }) if (d.ShowDialog(FindForm()) == DialogResult.OK) txtOut.Text = d.SelectedPath; };
            btnOpenOut.Click += (s, e) => { Directory.CreateDirectory(txtOut.Text); Process.Start("explorer.exe", OdrTool.Quote(txtOut.Text)); };
            btnExtract.Click += async (s, e) => await ExtractAsync(CheckedTables(), txtOut.Text);
            btnScavenge.Click += async (s, e) => await ScavengeAsync();
            btnCancel.Click += (s, e) => OdrTool.Cancel();
            lvResults.DoubleClick += (s, e) => { if (lvResults.SelectedItems.Count == 1) ShowPreview((ExtractResult)lvResults.SelectedItems[0].Tag); };
            btnDemo.Visible = OdrTool.HasDemo;
            SetBusy(false);
        }

        void SetStatus(string s) { lblStatus.Text = s; Status?.Invoke(s); }

        void SetBusy(bool b)
        {
            busy = b;
            foreach (var c in new Control[] { btnAdd, btnRemove, btnDemo, btnRead, btnExtract, btnScavenge, btnAll, btnNone, btnBrowseOut })
                c.Enabled = !b;
            btnCancel.Enabled = b;
            bar.Style = b ? ProgressBarStyle.Marquee : ProgressBarStyle.Blocks;
            UseWaitCursor = b;
        }

        // ------------------------------------------------------------------ step 1
        void AddFiles()
        {
            using (var d = new OpenFileDialog
            {
                Title = "اختر ملفات قاعدة البيانات (SYSTEM + ملفات الجداول)", Multiselect = true,
                Filter = "ملفات أوراكل (*.dbf;*.ora;*.dat)|*.dbf;*.ora;*.dat|كل الملفات (*.*)|*.*"
            })
            {
                if (d.ShowDialog(FindForm()) != DialogResult.OK) return;
                AddPaths(d.FileNames);
            }
        }

        void AddPaths(IEnumerable<string> paths)
        {
            foreach (var p in paths)
                if (!lstFiles.Items.Contains(p)) lstFiles.Items.Add(p);
            SetStatus(lstFiles.Items.Count + " ملف مختار. الخطوة التالية: «قراءة الجداول».");
        }

        void AddDemo()
        {
            lstFiles.Items.Clear();
            AddPaths(Directory.GetFiles(OdrTool.DemoDir, "*.dbf").OrderBy(x => x));
            txtOut.Text = Path.Combine(DefaultOutRoot, "demo_" + DateTime.Now.ToString("yyyy-MM-dd_HH-mm"));
        }

        List<string> Files() => lstFiles.Items.Cast<string>().ToList();

        // ------------------------------------------------------------------ step 2
        public async Task<bool> ReadTablesAsync()
        {
            if (lstFiles.Items.Count == 0) { Warn("أضف ملفات .dbf أولاً (ومنها ملف SYSTEM)."); return false; }
            SetBusy(true); SetStatus("جارٍ قراءة القاموس من ملف SYSTEM...");
            var args = new List<string> { "dict", "--json" }; args.AddRange(Files());
            var r = await OdrTool.RunAsync(OdrTool.Odr, args);
            SetBusy(false);
            if (r.Code != 0)
            {
                var why = OdrTool.Explain(r.All);
                SetStatus("تعذّرت قراءة الجداول.");
                Warn((why == "" ? "تعذّرت قراءة القاموس." : why) + "\n\nالتفاصيل:\n" + Last(r.All, 600));
                return false;
            }
            try
            {
                var js = new JavaScriptSerializer { MaxJsonLength = int.MaxValue, RecursionLimit = 256 };
                var root = (Dictionary<string, object>)js.DeserializeObject(r.Out);
                tables.Clear();
                foreach (Dictionary<string, object> t in (object[])root["tables"])
                {
                    tables.Add(new TableInfo
                    {
                        Owner = (string)t["owner"], Name = (string)t["name"], Tablespace = t["tablespace"] as string ?? "",
                        Columns = t["columns"] is object[] c ? c.Length : 0
                    });
                }
                DictIssues = root["issues"] is object[] iss ? iss.Length : -1;
                var nls = root["nls"] as Dictionary<string, object>;
                string cs = nls != null && nls.ContainsKey("NLS_CHARACTERSET") ? (string)nls["NLS_CHARACTERSET"] : "?";
                string ver = nls != null && nls.ContainsKey("NLS_RDBMS_VERSION") ? (string)nls["NLS_RDBMS_VERSION"] : "?";
                int user = tables.Count(x => !IsSystemOwner(x.Owner));
                lblDb.Text = $"قاعدة البيانات: إصدار {ver} · الترميز {cs} · {tables.Count:N0} جدولاً ({user:N0} للمستخدمين) · مشاكل القاموس: {DictIssues}";
            }
            catch (Exception ex) { Warn("تعذّر فهم مخرجات القاموس: " + ex.Message); return false; }
            FillTables();
            foreach (ListViewItem i in lvTables.Items) i.Checked = true;
            SetStatus($"تمت قراءة {tables.Count:N0} جدولاً. حدّد ما تريد ثم اضغط «استخراج الجداول المحددة».");
            return true;
        }

        public int VisibleTables => lvTables.Items.Count;

        void FillTables()
        {
            var checkedNow = new HashSet<string>(CheckedTables());
            var f = txtFilter.Text.Trim().ToUpperInvariant();
            lvTables.BeginUpdate();
            lvTables.Items.Clear();
            foreach (var t in tables.Where(t => (!chkHideSys.Checked || !IsSystemOwner(t.Owner)) && (f == "" || t.Full.Contains(f)))
                                     .OrderBy(t => t.Owner).ThenBy(t => t.Name))
            {
                var it = new ListViewItem(t.Full) { Tag = t, Checked = checkedNow.Contains(t.Full) };
                it.SubItems.Add(t.Tablespace); it.SubItems.Add(t.Columns.ToString());
                lvTables.Items.Add(it);
            }
            lvTables.EndUpdate();
        }

        public List<string> CheckedTablesPublic() => CheckedTables();
        List<string> CheckedTables() => lvTables.Items.Cast<ListViewItem>().Where(i => i.Checked).Select(i => ((TableInfo)i.Tag).Full).ToList();

        // ------------------------------------------------------------------ step 3
        public async Task ExtractAsync(List<string> which, string outDir)
        {
            if (which.Count == 0) { Warn("لا يوجد جدول محدد. اقرأ الجداول ثم حدّد واحداً على الأقل."); return; }
            try { Directory.CreateDirectory(outDir); } catch (Exception ex) { Warn("لا يمكن إنشاء مجلد النتائج: " + ex.Message); return; }
            SetBusy(true);
            Results.Clear(); lvResults.Items.Clear();
            int n = 0;
            foreach (var t in which)
            {
                n++;
                SetStatus($"جارٍ استخراج {t}  ({n} من {which.Count})...");
                var folder = Path.Combine(outDir, Safe(t));
                if (Directory.Exists(folder)) try { Directory.Delete(folder, true); } catch { }
                var args = new List<string> { "extract" }; args.AddRange(Files()); args.AddRange(new[] { "--table", t, "--out", folder });
                var r = await OdrTool.RunAsync(OdrTool.Odr, args);
                if (r.Cancelled) { SetStatus("أُوقف الاستخراج."); break; }
                var res = new ExtractResult { Table = t, Folder = folder, Code = r.Code, Layers = Layers.Parse(r.All) };
                if (r.Code == 0) res.Status = "✔ تم — كل الصفوف مقروءة";
                else if (r.Code == 2) res.Status = $"⚠ تم جزئياً — {res.Layers.Rejected:N0} صف لم يمكن إثباته (السبب في rejected.csv)";
                else { var w = OdrTool.Explain(r.All); res.Status = "✖ رُفض: " + (w != "" ? w : Last(r.All, 160)); }
                res.Message = r.All;
                AddResult(res);
            }
            SetBusy(false);
            int v = Results.Where(x => x.Layers.Ok).Sum(x => x.Layers.Verified), j = Results.Where(x => x.Layers.Ok).Sum(x => x.Layers.Rejected);
            SetStatus($"انتهى: {Results.Count} جدولاً · {v:N0} صف موثَّق · {j:N0} مرفوض. النتائج في {outDir}  — انقر مرتين على جدول لعرضه.");
        }

        void AddResult(ExtractResult res)
        {
            Results.Add(res);
            var it = new ListViewItem(res.Table) { Tag = res };
            it.SubItems.Add(res.Layers.Ok ? res.Layers.Verified.ToString("N0") : "-");
            it.SubItems.Add(res.Layers.Ok ? res.Layers.Inferred.ToString("N0") : "-");
            it.SubItems.Add(res.Layers.Ok ? res.Layers.Rejected.ToString("N0") : "-");
            it.SubItems.Add(res.Status);
            it.BackColor = res.Code == 0 ? Color.FromArgb(232, 247, 236) : res.Code == 2 ? Color.FromArgb(255, 246, 225) : Color.FromArgb(253, 232, 232);
            lvResults.Items.Add(it);
            it.EnsureVisible();
        }

        async Task ScavengeAsync()
        {
            if (lstFiles.Items.Count == 0) { Warn("أضف ملفات .dbf أولاً."); return; }
            var outDir = Path.Combine(txtOut.Text, "بدون_SYSTEM");
            if (Directory.Exists(outDir)) try { Directory.Delete(outDir, true); } catch { }
            SetBusy(true); SetStatus("جارٍ الاستخراج بدون قاموس (قد يستغرق وقتاً)...");
            var args = new List<string> { "scavenge" }; args.AddRange(Files()); args.AddRange(new[] { "--out", outDir });
            var r = await OdrTool.RunAsync(OdrTool.Odr, args);
            SetBusy(false);
            if (r.Code != 0 && r.Code != 2) { Warn("تعذّر الاستخراج بدون قاموس.\n\n" + Last(r.All, 600)); return; }
            Results.Clear(); lvResults.Items.Clear();
            foreach (var d in Directory.GetDirectories(outDir).OrderBy(x => x))
            {
                var csv = Path.Combine(d, "inferred.csv");
                int rows = 0;
                if (File.Exists(csv)) Csv.Read(csv, 0, out rows);
                AddResult(new ExtractResult
                {
                    Table = Path.GetFileName(d), Folder = d, Code = 0,
                    Layers = new Layers { Verified = 0, Inferred = rows, Rejected = 0 },
                    Status = "مستنتج فقط — لا قاموس، أسماء الأعمدة COL_n ونوعها مستنتج"
                });
            }
            SetStatus($"انتهى: {Results.Count} كائن في {outDir}. كل القيم «مستنتجة» لأن ملف SYSTEM غير موجود.");
        }

        // ------------------------------------------------------------------ preview
        public Form ShowPreview(ExtractResult res, bool modal = true)
        {
            var f = new Form
            {
                Text = "بيانات " + res.Table, Size = new Size(1100, 680), StartPosition = FormStartPosition.CenterParent,
                RightToLeft = RightToLeft.Yes, RightToLeftLayout = true, Font = Font, Icon = FindForm()?.Icon
            };
            var tabs = new TabControl { Dock = DockStyle.Fill, RightToLeftLayout = true };
            foreach (var (file, label) in new[] { ("verified.csv", "موثَّق ✔"), ("inferred.csv", "مستنتج"), ("rejected.csv", "مرفوض") })
            {
                var path = Path.Combine(res.Folder, file);
                if (!File.Exists(path)) continue;
                var rows = Csv.Read(path, 2000, out int total);
                var page = new TabPage($"{label}  ({total:N0})");
                var g = new DataGridView
                {
                    Dock = DockStyle.Fill, ReadOnly = true, AllowUserToAddRows = false, AllowUserToDeleteRows = false,
                    RowHeadersVisible = false, AutoSizeColumnsMode = DataGridViewAutoSizeColumnsMode.DisplayedCells,
                    BackgroundColor = Color.White
                };
                if (rows.Count > 0)
                {
                    foreach (var h in rows[0]) g.Columns.Add(h, h);
                    foreach (var c in g.Columns.Cast<DataGridViewColumn>()) { c.SortMode = DataGridViewColumnSortMode.Automatic; }
                    foreach (var row in rows.Skip(1)) g.Rows.Add(row.Take(g.Columns.Count).Select(x => x.Length > 300 ? x.Substring(0, 300) + "…" : x).ToArray<object>());
                }
                var bottom = new FlowLayoutPanel { Dock = DockStyle.Bottom, Height = 44 };
                var info = new Label { AutoSize = true, Margin = new Padding(8, 12, 8, 4), Text = total > 2000 ? $"يعرض أول 2,000 صف من {total:N0}" : $"{total:N0} صف" };
                var excel = Btn(new Button(), "فتح في Excel", 130);
                excel.Click += (s, e) => { try { Process.Start(new ProcessStartInfo(Csv.ExcelCopy(path)) { UseShellExecute = true }); } catch (Exception ex) { Warn(ex.Message); } };
                var folder = Btn(new Button(), "فتح المجلد", 110);
                folder.Click += (s, e) => Process.Start("explorer.exe", OdrTool.Quote(res.Folder));
                bottom.Controls.AddRange(new Control[] { excel, folder, info });
                page.Controls.Add(g); page.Controls.Add(bottom);
                tabs.TabPages.Add(page);
            }
            f.Controls.Add(tabs);
            if (modal) { f.ShowDialog(FindForm()); f.Dispose(); return null; }
            f.Show(FindForm());
            return f;
        }

        // ------------------------------------------------------------------ helpers
        void Warn(string msg)
        {
            if (autotest) { Log("WARN " + msg.Replace("\n", " ")); return; }
            MessageBox.Show(FindForm(), msg, "ODR", MessageBoxButtons.OK, MessageBoxIcon.Warning, MessageBoxDefaultButton.Button1,
                MessageBoxOptions.RtlReading | MessageBoxOptions.RightAlign);
        }

        static string Last(string s, int n) { s = (s ?? "").Trim(); return s.Length <= n ? s : "…" + s.Substring(s.Length - n); }
        static string Safe(string s) { foreach (var c in Path.GetInvalidFileNameChars()) s = s.Replace(c, '_'); return s; }

        // ------------------------------------------------------------------ hooks for MainForm.AutoTest
        readonly StringBuilder log = new StringBuilder();
        public string LogText => log.ToString();
        void Log(string s) { log.AppendLine(s); }
        public int FileCount => lstFiles.Items.Count;
        public void AddDemoFiles() => AddDemo();
        public string OutDir { get => txtOut.Text; set => txtOut.Text = value; }
    }
}
