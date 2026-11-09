// MainForm.cs - the ODR window (Arabic, right-to-left). Three steps, no typing:
//   1. add the .dbf files   2. read the tables   3. extract the chosen tables -> results + preview.
using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using OdrApp.Remote;

namespace OdrApp
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

    public sealed class MainForm : Form
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

        readonly string autotestDir;
        public int ExitCode;

        readonly ListBox lstFiles = new ListBox();
        readonly Button btnAdd = new Button(), btnRemove = new Button(), btnDemo = new Button(), btnRead = new Button();
        readonly Button btnExtract = new Button(), btnScavenge = new Button(), btnOpenOut = new Button();
        readonly Button btnBrowseOut = new Button(), btnCancel = new Button(), btnAll = new Button(), btnNone = new Button();
        readonly CheckBox chkHideSys = new CheckBox();
        readonly TextBox txtFilter = new TextBox(), txtOut = new TextBox();
        readonly ListView lvTables = new ListView(), lvResults = new ListView();
        readonly ProgressBar bar = new ProgressBar();
        readonly Label lblStatus = new Label(), lblDb = new Label();

        readonly List<TableInfo> tables = new List<TableInfo>();
        public int DictIssues = -1;
        public readonly List<ExtractResult> Results = new List<ExtractResult>();
        bool busy;

        // Remote kill switch (odr-apps/odr/license.json). Disabled under --autotest so CI never
        // depends on the network; the logic itself is covered by LicenseService's unit tests.
        LicenseService lic;
        LockForm lockForm;
        readonly Timer licTimer = new Timer { Interval = 60_000 };

        public MainForm(string autotestDir)
        {
            this.autotestDir = autotestDir;
            Text = "ODR — استرجاع بيانات أوراكل من ملفات .dbf";
            Font = new Font("Segoe UI", 10f);
            RightToLeft = RightToLeft.Yes;
            RightToLeftLayout = true;
            StartPosition = FormStartPosition.CenterScreen;
            Size = new Size(1180, 800);
            MinimumSize = new Size(980, 680);
            try { Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); } catch { }
            BuildUi();
            txtOut.Text = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "ODR",
                DateTime.Now.ToString("yyyy-MM-dd_HH-mm"));
            SetStatus(OdrTool.Odr == null
                ? "تنبيه: محرك odr.exe غير موجود — أعد تثبيت البرنامج."
                : "ابدأ بإضافة ملفات قاعدة البيانات (.dbf) — يجب أن يكون بينها ملف SYSTEM.");
            Shown += async (s, e) =>
            {
                if (autotestDir != null) { await AutoTest(); return; }
                StartLicense();
            };
        }

        // ------------------------------------------------------------------ remote license gate
        void StartLicense()
        {
            lic = new LicenseService(new Settings(Settings.DefaultPath));
            lic.Changed += () => { if (IsHandleCreated) BeginInvoke(new Action(ApplyLicense)); };
            ApplyLicense();                       // cached state, instantly
            var _ = lic.RefreshAsync();           // network, in the background
            licTimer.Tick += async (s, e) => await lic.OnResumedAsync();
            licTimer.Start();
            Activated += async (s, e) => { if (lic != null) await lic.OnResumedAsync(); };
        }

        void ApplyLicense()
        {
            if (lic.Allowed)
            {
                if (lockForm != null) { var f = lockForm; lockForm = null; f.Close(); f.Dispose(); Enabled = true; Activate(); }
                return;
            }
            Enabled = false;
            if (lockForm == null) { lockForm = new LockForm(lic, this); lockForm.Show(this); }
            lockForm.Activate();
        }

        /// <summary>Device id for the About/ℹ️ dialogs.</summary>
        public string DeviceIdPretty => DeviceIdentity.Pretty(DeviceIdentity.Id());

        // ------------------------------------------------------------------ layout
        static Button Btn(Button b, string text, int w = 150)
        {
            b.Text = text; b.Width = w; b.Height = 34; b.Margin = new Padding(4); b.UseVisualStyleBackColor = true;
            return b;
        }

        static GroupBox Group(string title, Control content)
        {
            var g = new GroupBox { Text = title, Dock = DockStyle.Fill, Padding = new Padding(8), Font = new Font("Segoe UI", 10.5f, FontStyle.Bold) };
            content.Font = new Font("Segoe UI", 10f);
            content.Dock = DockStyle.Fill;
            g.Controls.Add(content);
            return g;
        }

        void BuildUi()
        {
            var header = new Panel { Dock = DockStyle.Top, Height = 64, BackColor = Color.FromArgb(18, 52, 110) };
            var title = new Label
            {
                Text = "ODR — استرجاع بيانات أوراكل", ForeColor = Color.White, Font = new Font("Segoe UI", 16f, FontStyle.Bold),
                AutoSize = true, Location = new Point(16, 8)
            };
            var sub = new Label
            {
                Text = "يقرأ ملفات .dbf مباشرة بدون أوراكل — للقراءة فقط، لا يغيّر ملفاتك أبداً   ·   v" + Application.ProductVersion,
                ForeColor = Color.FromArgb(200, 215, 240), AutoSize = true, Location = new Point(18, 40)
            };
            header.Controls.Add(title); header.Controls.Add(sub);

            // step 1: files
            var p1 = new TableLayoutPanel { ColumnCount = 1, RowCount = 2 };
            p1.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); p1.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            lstFiles.HorizontalScrollbar = true; lstFiles.Dock = DockStyle.Fill; lstFiles.RightToLeft = RightToLeft.No;
            lstFiles.SelectionMode = SelectionMode.MultiExtended;
            var f1 = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Fill, WrapContents = true };
            f1.Controls.AddRange(new Control[] { Btn(btnAdd, "➕ إضافة ملفات .dbf", 170), Btn(btnRemove, "إزالة المحدد", 120), Btn(btnDemo, "تجربة بملفات العرض", 170) });
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
            btnExtract.BackColor = Color.FromArgb(34, 140, 80); btnExtract.ForeColor = Color.White; btnExtract.Font = new Font("Segoe UI", 10.5f, FontStyle.Bold);
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

            var statusPanel = new Panel { Dock = DockStyle.Bottom, Height = 34, BackColor = Color.FromArgb(235, 240, 248) };
            lblStatus.Dock = DockStyle.Fill; lblStatus.TextAlign = ContentAlignment.MiddleRight; lblStatus.Padding = new Padding(10, 0, 10, 0);
            statusPanel.Controls.Add(lblStatus);

            Controls.Add(grid); Controls.Add(statusPanel); Controls.Add(header);

            btnAdd.Click += (s, e) => AddFiles();
            btnRemove.Click += (s, e) => { foreach (var i in lstFiles.SelectedItems.Cast<object>().ToList()) lstFiles.Items.Remove(i); };
            btnDemo.Click += (s, e) => AddDemo();
            btnRead.Click += async (s, e) => await ReadTablesAsync();
            chkHideSys.CheckedChanged += (s, e) => FillTables();
            txtFilter.TextChanged += (s, e) => FillTables();
            btnAll.Click += (s, e) => { foreach (ListViewItem i in lvTables.Items) i.Checked = true; };
            btnNone.Click += (s, e) => { foreach (ListViewItem i in lvTables.Items) i.Checked = false; };
            btnBrowseOut.Click += (s, e) => { using (var d = new FolderBrowserDialog { SelectedPath = txtOut.Text }) if (d.ShowDialog(this) == DialogResult.OK) txtOut.Text = d.SelectedPath; };
            btnOpenOut.Click += (s, e) => { Directory.CreateDirectory(txtOut.Text); Process.Start("explorer.exe", OdrTool.Quote(txtOut.Text)); };
            btnExtract.Click += async (s, e) => await ExtractAsync(CheckedTables(), txtOut.Text);
            btnScavenge.Click += async (s, e) => await ScavengeAsync();
            btnCancel.Click += (s, e) => OdrTool.Cancel();
            lvResults.DoubleClick += (s, e) => { if (lvResults.SelectedItems.Count == 1) ShowPreview((ExtractResult)lvResults.SelectedItems[0].Tag); };
            btnDemo.Visible = OdrTool.HasDemo;
            SetBusy(false);
        }

        void SetStatus(string s) { lblStatus.Text = s; }

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
                if (d.ShowDialog(this) != DialogResult.OK) return;
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
            txtOut.Text = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "ODR", "demo_" + DateTime.Now.ToString("yyyy-MM-dd_HH-mm"));
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
                RightToLeft = RightToLeft.Yes, RightToLeftLayout = true, Font = Font, Icon = Icon
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
            if (modal) { f.ShowDialog(this); f.Dispose(); return null; }
            f.Show(this);
            return f;
        }

        // ------------------------------------------------------------------ helpers
        void Warn(string msg)
        {
            if (autotestDir != null) { Log("WARN " + msg.Replace("\n", " ")); return; }
            MessageBox.Show(this, msg, "ODR", MessageBoxButtons.OK, MessageBoxIcon.Warning, MessageBoxDefaultButton.Button1,
                MessageBoxOptions.RtlReading | MessageBoxOptions.RightAlign);
        }

        static string Last(string s, int n) { s = (s ?? "").Trim(); return s.Length <= n ? s : "…" + s.Substring(s.Length - n); }
        static string Safe(string s) { foreach (var c in Path.GetInvalidFileNameChars()) s = s.Replace(c, '_'); return s; }

        // ------------------------------------------------------------------ unattended proof (--autotest DIR)
        readonly StringBuilder log = new StringBuilder();
        int pass, fail;
        void Log(string s) { log.AppendLine(s); }
        void Check(bool ok, string what) { if (ok) pass++; else fail++; Log((ok ? "PASS  " : "FAIL  ") + what); }

        void Shot(Form f, string name)
        {
            f.Activate(); f.Refresh(); Application.DoEvents(); System.Threading.Thread.Sleep(500); Application.DoEvents();
            var b = f.Bounds;
            using (var bmp = new Bitmap(b.Width, b.Height))
            {
                f.DrawToBitmap(bmp, new Rectangle(Point.Empty, b.Size));
                bmp.Save(Path.Combine(autotestDir, name + ".png"), ImageFormat.Png);
            }
            try
            {
                using (var bmp = new Bitmap(b.Width, b.Height))
                {
                    using (var g = Graphics.FromImage(bmp)) g.CopyFromScreen(b.Location, Point.Empty, b.Size);
                    bmp.Save(Path.Combine(autotestDir, name + "_screen.png"), ImageFormat.Png);
                }
            }
            catch (Exception ex) { Log("screen capture unavailable: " + ex.Message); }
        }

        async Task AutoTest()
        {
            Directory.CreateDirectory(autotestDir);
            TopMost = true;
            var wa = Screen.PrimaryScreen.WorkingArea;
            Bounds = new Rectangle(wa.X, wa.Y, Math.Min(Width, wa.Width), Math.Min(Height, wa.Height));
            try
            {
                Log("ODR desktop app autotest · " + Application.ExecutablePath + " · v" + Application.ProductVersion);
                Check(OdrTool.Odr != null, "engine odr.exe installed next to the app: " + OdrTool.Odr);
                Check(OdrTool.HasDemo, "demo datafiles installed: " + OdrTool.DemoDir);
                Shot(this, "01_start");

                AddDemo();
                Check(lstFiles.Items.Count == 4, "4 demo datafiles added (" + lstFiles.Items.Count + ")");
                Check(await ReadTablesAsync(), "tables read from SYSTEM");
                Check(DictIssues == 0, "dictionary issues = 0 (" + DictIssues + ")");
                Check(VisibleTables == 18, "18 user tables shown, system tables hidden (" + VisibleTables + ")");
                Shot(this, "02_tables");

                await ExtractAsync(CheckedTables(), Path.Combine(autotestDir, "out"));
                Check(Results.Count == 18, "18 tables extracted (" + Results.Count + ")");
                Check(Results.All(r => r.Code == 0), "all 18 extracted cleanly (exit 0)");
                ExtractResult R(string t) => Results.FirstOrDefault(x => x.Table == "ODR." + t) ?? new ExtractResult();
                void L(string t, int v) { var r = R(t).Layers; Check(r.Verified == v && r.Inferred == 0 && r.Rejected == 0, $"{t}: verified {r.Verified} · inferred {r.Inferred} · rejected {r.Rejected} (expected {v} · 0 · 0)"); }
                L("T_JOURNAL", 20000); L("T_LOB_BASIC", 6); L("T_LOB_SECURE", 6); L("T_COMPRESS", 2000);
                var jv = Path.Combine(R("T_JOURNAL").Folder, "verified.csv");
                int hits = File.Exists(jv) ? File.ReadAllLines(jv, Encoding.UTF8).Count(x => x.Contains("قيد يومية رقم 7684")) : -1;
                Check(hits == 1, "Arabic text 'قيد يومية رقم 7684' present exactly once in T_JOURNAL (" + hits + ")");
                var lb = Path.Combine(R("T_LOB_BASIC").Folder, "verified.csv");
                var lrows = File.Exists(lb) ? Csv.Read(lb, 10, out _) : new List<string[]>();
                Check(lrows.Any(r => r.Length > 4 && r[4] == "فاتورة رقم 1 - مرفق"), "T_LOB_BASIC inline Arabic CLOB exact");
                Shot(this, "03_results");

                var pv = ShowPreview(R("T_JOURNAL"), modal: false);
                if (pv != null) { pv.TopMost = true; Shot(pv, "04_preview_arabic"); pv.Close(); }
            }
            catch (Exception ex) { Check(false, "exception: " + ex); }
            Log($"RESULT: {pass} passed, {fail} failed");
            File.WriteAllText(Path.Combine(autotestDir, "autotest.txt"), log.ToString(), new UTF8Encoding(true));
            ExitCode = fail == 0 && pass > 0 ? 0 : 1;
            Close();
        }
    }
}
