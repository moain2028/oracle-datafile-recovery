// MainForm.cs - the ODR shell (v0.7): navy sidebar with five pages, gold accents, Arabic RTL.
//   الرئيسية · الاسترجاع · التشخيص · الفرز · البصمة · الإعدادات
// Every page carries an ℹ️ button that explains what it is for. The remote-lock gate (odr-apps/odr/license.json)
// covers the whole window. `--autotest DIR` drives the recovery page + diagnose page unattended for the CI proof.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using System.Windows.Forms;
using OdrApp.Remote;
using OdrApp.Ui;

namespace OdrApp
{
    public sealed class MainForm : Form
    {
        readonly string autotestDir;
        public int ExitCode;

        readonly Panel side = new Panel { Dock = DockStyle.Right, Width = 220, BackColor = Theme.Navy };
        readonly Panel host = new Panel { Dock = DockStyle.Fill, BackColor = Theme.Bg };
        readonly Label lblStatus = new Label(), lblLic = new Label();
        readonly Dictionary<string, Control> pages = new Dictionary<string, Control>();
        readonly Dictionary<string, Button> navs = new Dictionary<string, Button>();
        readonly Settings prefs = new Settings(Settings.DefaultPath);

        public readonly RecoverPage Recover;
        public readonly DiagnosePage Diagnose = new DiagnosePage();
        public readonly TriagePage Triage = new TriagePage();
        public readonly FingerprintPage Fingerprint = new FingerprintPage();
        readonly SettingsPage settings;
        readonly HomePage home = new HomePage();

        LicenseService lic;
        LockForm lockForm;
        readonly Timer licTimer = new Timer { Interval = 60_000 };

        public MainForm(string autotestDir)
        {
            this.autotestDir = autotestDir;
            Text = "ODR — استرجاع بيانات أوراكل";
            Font = Theme.Base;
            RightToLeft = RightToLeft.Yes; RightToLeftLayout = true;
            StartPosition = FormStartPosition.CenterScreen;
            Size = new Size(1280, 820); MinimumSize = new Size(1024, 700);
            BackColor = Theme.Bg;
            try { Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); } catch { }

            Recover = new RecoverPage(autotestDir != null) { DefaultOutRoot = prefs.Get("out_dir", Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "ODR")) };
            settings = new SettingsPage(prefs) { License = () => lic };
            BuildShell();

            home.Go += Show;
            home.DropFile += p => { Diagnose.SetFile(p); Show("diagnose"); var _ = Diagnose.RunAsync(false); };
            Recover.OpenDiagnosis += p => { Diagnose.SetFile(p); Show("diagnose"); var _ = Diagnose.RunAsync(false); };
            Triage.OpenDiagnosis += p => { Diagnose.SetFile(p); Show("diagnose"); var _ = Diagnose.RunAsync(false); };
            foreach (var pg in new Control[] { Recover, Diagnose, Triage, Fingerprint })
            {
                if (pg is RecoverPage r) r.Status += SetStatus;
                if (pg is DiagnosePage d) d.Status += SetStatus;
                if (pg is TriagePage t) t.Status += SetStatus;
                if (pg is FingerprintPage f) f.Status += SetStatus;
            }
            Show("home");
            SetStatus(OdrTool.Odr == null ? "تنبيه: محرك odr.exe غير موجود — أعد تثبيت البرنامج." : "جاهز.");
            Shown += async (s, e) =>
            {
                if (autotestDir != null) { await AutoTest(); return; }
                StartLicense();
            };
        }

        // ------------------------------------------------------------------ shell
        void BuildShell()
        {
            // sidebar: brand + nav + footer
            var brand = new Panel { Dock = DockStyle.Top, Height = 96, BackColor = Theme.Navy };
            brand.Controls.Add(new Label { Text = "ODR", Font = new Font("Segoe UI", 26f, FontStyle.Bold), ForeColor = Theme.GoldLight, AutoSize = true, Location = new Point(18, 14) });
            brand.Controls.Add(new Label { Text = "استرجاع بيانات أوراكل", Font = Theme.Small, ForeColor = Color.FromArgb(190, 200, 225), AutoSize = true, Location = new Point(20, 60) });
            side.Controls.Add(brand);

            var nav = new FlowLayoutPanel { Dock = DockStyle.Top, FlowDirection = FlowDirection.TopDown, AutoSize = true, WrapContents = false, Padding = new Padding(0, 10, 0, 0) };
            Nav(nav, "home", "🏠", "الرئيسية");
            Nav(nav, "recover", "📦", "الاسترجاع");
            Nav(nav, "diagnose", "🩺", "التشخيص");
            Nav(nav, "triage", "⚖️", "الفرز");
            Nav(nav, "fingerprint", "🔍", "البصمة");
            Nav(nav, "settings", "⚙️", "الإعدادات");
            side.Controls.Add(nav); nav.BringToFront();

            var foot = new Panel { Dock = DockStyle.Bottom, Height = 92, BackColor = Theme.Navy };
            lblLic.AutoSize = true; lblLic.ForeColor = Color.FromArgb(160, 200, 160); lblLic.Font = Theme.Small; lblLic.Location = new Point(18, 8); lblLic.Text = "";
            var help = Theme.Secondary("ℹ️ ما هذا البرنامج؟", 180); help.Location = new Point(18, 30); help.BackColor = Theme.NavyLight; help.ForeColor = Color.White; help.FlatAppearance.BorderColor = Theme.Gold;
            help.Click += (s, e) => Ui.Help.Show(this, "الرئيسية", Ui.Help.Home);
            var ver = new Label { Text = "v" + Application.ProductVersion + " · للقراءة فقط", Font = Theme.Small, ForeColor = Color.FromArgb(140, 155, 185), AutoSize = true, Location = new Point(20, 70) };
            foot.Controls.AddRange(new Control[] { lblLic, help, ver });
            side.Controls.Add(foot);

            // pages
            pages["home"] = home; pages["recover"] = Recover; pages["diagnose"] = Diagnose; pages["triage"] = Triage; pages["fingerprint"] = Fingerprint; pages["settings"] = settings;
            foreach (var p in pages.Values) { p.Dock = DockStyle.Fill; p.Visible = false; host.Controls.Add(p); }

            // status bar
            var status = new Panel { Dock = DockStyle.Bottom, Height = 30, BackColor = Color.White };
            lblStatus.Dock = DockStyle.Fill; lblStatus.TextAlign = ContentAlignment.MiddleRight; lblStatus.Padding = new Padding(12, 0, 12, 0); lblStatus.ForeColor = Theme.Text;
            status.Controls.Add(lblStatus);
            status.Paint += (s, e) => e.Graphics.DrawLine(new Pen(Theme.CardBorder), 0, 0, status.Width, 0);

            Controls.Add(host); Controls.Add(status); Controls.Add(side);
        }

        void Nav(FlowLayoutPanel nav, string key, string icon, string text)
        {
            var b = new Button
            {
                Text = "  " + icon + "   " + text, Width = 220, Height = 48, FlatStyle = FlatStyle.Flat, TextAlign = ContentAlignment.MiddleRight,
                Font = new Font("Segoe UI", 11.5f), ForeColor = Color.FromArgb(210, 220, 240), BackColor = Theme.Navy, Cursor = Cursors.Hand, Margin = new Padding(0),
                Padding = new Padding(0, 0, 14, 0), RightToLeft = RightToLeft.Yes
            };
            b.FlatAppearance.BorderSize = 0; b.FlatAppearance.MouseOverBackColor = Theme.NavyLight;
            b.Click += (s, e) => Show(key);
            navs[key] = b; nav.Controls.Add(b);
        }

        public void Show(string key)
        {
            if (!pages.ContainsKey(key)) return;
            foreach (var kv in pages) kv.Value.Visible = kv.Key == key;
            foreach (var kv in navs)
            {
                bool on = kv.Key == key;
                kv.Value.BackColor = on ? Theme.NavyLight : Theme.Navy;
                kv.Value.ForeColor = on ? Theme.GoldLight : Color.FromArgb(210, 220, 240);
                kv.Value.Font = new Font("Segoe UI", 11.5f, on ? FontStyle.Bold : FontStyle.Regular);
            }
            if (key == "settings") settings.RefreshLicense();
        }

        void SetStatus(string s) { lblStatus.Text = s; }

        // ------------------------------------------------------------------ remote license gate
        void StartLicense()
        {
            lic = new LicenseService(prefs);
            lic.Changed += () => { if (IsHandleCreated) BeginInvoke(new Action(ApplyLicense)); };
            ApplyLicense();
            var _ = lic.RefreshAsync();
            licTimer.Tick += async (s, e) => await lic.OnResumedAsync();
            licTimer.Start();
            Activated += async (s, e) => { if (lic != null) await lic.OnResumedAsync(); };
        }

        void ApplyLicense()
        {
            var st = lic.State;
            lblLic.Text = st == null ? "" : st.Allowed ? "● مفعَّل" + (st.Offline ? " (بلا إنترنت)" : "") : "● موقوف";
            lblLic.ForeColor = st != null && st.Allowed ? Color.FromArgb(120, 210, 140) : Color.FromArgb(255, 140, 140);
            settings.RefreshLicense();
            if (lic.Allowed)
            {
                if (lockForm != null) { var f = lockForm; lockForm = null; f.Close(); f.Dispose(); Enabled = true; Activate(); }
                return;
            }
            Enabled = false;
            if (lockForm == null) { lockForm = new LockForm(lic, this); lockForm.Show(this); }
            lockForm.Activate();
        }

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
                Shot(this, "01_home");

                Show("recover");
                Recover.AddDemoFiles();
                Check(Recover.FileCount == 4, "4 demo datafiles added (" + Recover.FileCount + ")");
                Check(await Recover.ReadTablesAsync(), "tables read from SYSTEM");
                Check(Recover.DictIssues == 0, "dictionary issues = 0 (" + Recover.DictIssues + ")");
                Check(Recover.VisibleTables == 18, "18 user tables shown, system tables hidden (" + Recover.VisibleTables + ")");
                Shot(this, "02_tables");

                await Recover.ExtractAsync(Recover.CheckedTablesPublic(), Path.Combine(autotestDir, "out"));
                Check(Recover.Results.Count == 18, "18 tables extracted (" + Recover.Results.Count + ")");
                Check(Recover.Results.All(r => r.Code == 0), "all 18 extracted cleanly (exit 0)");
                ExtractResult R(string t) => Recover.Results.FirstOrDefault(x => x.Table == "ODR." + t) ?? new ExtractResult();
                void L(string t, int v) { var r = R(t).Layers; Check(r.Verified == v && r.Inferred == 0 && r.Rejected == 0, $"{t}: verified {r.Verified} · inferred {r.Inferred} · rejected {r.Rejected} (expected {v} · 0 · 0)"); }
                L("T_JOURNAL", 20000); L("T_LOB_BASIC", 6); L("T_LOB_SECURE", 6); L("T_COMPRESS", 2000);
                var jv = Path.Combine(R("T_JOURNAL").Folder, "verified.csv");
                int hits = File.Exists(jv) ? File.ReadAllLines(jv, Encoding.UTF8).Count(x => x.Contains("قيد يومية رقم 7684")) : -1;
                Check(hits == 1, "Arabic text 'قيد يومية رقم 7684' present exactly once in T_JOURNAL (" + hits + ")");
                var lb = Path.Combine(R("T_LOB_BASIC").Folder, "verified.csv");
                var lrows = File.Exists(lb) ? Csv.Read(lb, 10, out _) : new List<string[]>();
                Check(lrows.Any(r => r.Length > 4 && r[4] == "فاتورة رقم 1 - مرفق"), "T_LOB_BASIC inline Arabic CLOB exact");
                Shot(this, "03_results");

                var pv = Recover.ShowPreview(R("T_JOURNAL"), modal: false);
                if (pv != null) { pv.TopMost = true; Shot(pv, "04_preview_arabic"); pv.Close(); }

                // diagnose page on a healthy demo file: score must be high and the HEALTHY finding present
                Show("diagnose");
                var demo = Directory.GetFiles(OdrTool.DemoDir, "*.dbf").OrderBy(x => x).FirstOrDefault(x => !x.ToLowerInvariant().Contains("system")) ?? Directory.GetFiles(OdrTool.DemoDir, "*.dbf").First();
                Diagnose.SetFile(demo);
                Check(await Diagnose.RunAsync(true), "diagnose ran on " + Path.GetFileName(demo));
                var d = Diagnose.Current;
                Check(d != null && d.Score >= 90, "healthy demo file scores >= 90 (" + (d?.Score ?? -1) + ")");
                Check(d != null && d.DamagedTotal == 0, "healthy demo file has 0 damaged blocks (" + (d?.DamagedTotal ?? -1) + ")");
                Check(d != null && d.Findings.Any(f => f.Code == "HEALTHY"), "HEALTHY finding present");
                Shot(this, "05_diagnose");

                Show("triage"); Shot(this, "06_triage");
                Show("fingerprint"); Shot(this, "07_fingerprint");
                Show("settings"); Shot(this, "08_settings");
            }
            catch (Exception ex) { Check(false, "exception: " + ex); }
            Log(Recover.LogText);
            Log($"RESULT: {pass} passed, {fail} failed");
            File.WriteAllText(Path.Combine(autotestDir, "autotest.txt"), log.ToString(), new UTF8Encoding(true));
            ExitCode = fail == 0 && pass > 0 ? 0 : 1;
            Close();
        }
    }
}
