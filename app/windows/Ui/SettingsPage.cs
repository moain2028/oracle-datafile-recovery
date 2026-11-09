// SettingsPage.cs - defaults, device id, licence state, versions, support.
using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Windows.Forms;
using OdrApp.Remote;

namespace OdrApp.Ui
{
    public sealed class SettingsPage : UserControl
    {
        readonly Label lblLic = new Label(), lblVer = new Label();
        readonly TextBox txtDevice = new TextBox(), txtOut = new TextBox();
        public Func<LicenseService> License;

        public SettingsPage(Settings prefs)
        {
            Dock = DockStyle.Fill; BackColor = Theme.Bg; RightToLeft = RightToLeft.Yes; Font = Theme.Base; Padding = new Padding(12);
            var head = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top, Padding = new Padding(0, 0, 0, 6) };
            var info = Theme.Info(); info.Click += (s, e) => Help.Show(FindForm(), "الإعدادات", Help.Settings);
            head.Controls.Add(Theme.Title("⚙️ الإعدادات وحول البرنامج")); head.Controls.Add(info);

            var grid = new TableLayoutPanel { Dock = DockStyle.Top, AutoSize = true, ColumnCount = 2, Padding = new Padding(10) };
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 220)); grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            int r = 0;
            void Row(string label, Control c) { grid.Controls.Add(new Label { Text = label, AutoSize = true, Font = Theme.Bold, ForeColor = Theme.Navy, Margin = new Padding(6, 12, 6, 6) }, 0, r); grid.Controls.Add(c, 1, r); r++; }

            txtOut.Width = 520; txtOut.RightToLeft = RightToLeft.No; txtOut.Font = Theme.Mono; txtOut.Margin = new Padding(4, 8, 4, 4);
            txtOut.Text = prefs.Get("out_dir", Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "ODR"));
            var outRow = new FlowLayoutPanel { AutoSize = true }; var bOut = Theme.Secondary("تغيير…"); bOut.Click += (s, e) => { using (var d = new FolderBrowserDialog { SelectedPath = txtOut.Text }) if (d.ShowDialog(this) == DialogResult.OK) { txtOut.Text = d.SelectedPath; prefs.Set("out_dir", d.SelectedPath); prefs.Save(); } };
            outRow.Controls.AddRange(new Control[] { txtOut, bOut });
            Row("مجلد النتائج الافتراضي", outRow);

            txtDevice.ReadOnly = true; txtDevice.Width = 160; txtDevice.RightToLeft = RightToLeft.No; txtDevice.Font = new Font("Consolas", 13f, FontStyle.Bold); txtDevice.Margin = new Padding(4, 8, 4, 4);
            txtDevice.Text = DeviceIdentity.Pretty(DeviceIdentity.Id());
            var devRow = new FlowLayoutPanel { AutoSize = true };
            var bCopy = Theme.Secondary("📋 نسخ"); bCopy.Click += (s, e) => { try { Clipboard.SetText(txtDevice.Text); } catch { } };
            var bWa = Theme.Secondary("💬 إرسال للمطوّر عبر واتساب"); bWa.Click += (s, e) => { try { Process.Start(new ProcessStartInfo("https://wa.me/967770941666?text=" + Uri.EscapeDataString("ODR Device ID: " + txtDevice.Text)) { UseShellExecute = true }); } catch { } };
            devRow.Controls.AddRange(new Control[] { txtDevice, bCopy, bWa, Theme.MutedLabel("رقم ثابت لهذا الجهاز، للترخيص فقط، لا يُرسل تلقائياً لأي جهة.") });
            Row("معرّف هذا الجهاز", devRow);

            lblLic.AutoSize = true; lblLic.Margin = new Padding(4, 12, 4, 4);
            var licRow = new FlowLayoutPanel { AutoSize = true };
            var bRecheck = Theme.Secondary("↻ إعادة التحقق الآن"); bRecheck.Click += async (s, e) => { var l = License?.Invoke(); if (l != null) { bRecheck.Enabled = false; await l.RefreshAsync(); bRecheck.Enabled = true; RefreshLicense(); } };
            licRow.Controls.AddRange(new Control[] { lblLic, bRecheck });
            Row("حالة الترخيص", licRow);

            lblVer.AutoSize = true; lblVer.Margin = new Padding(4, 12, 4, 4); lblVer.Font = Theme.Mono; lblVer.RightToLeft = RightToLeft.No;
            lblVer.Text = $"ODR.exe v{Application.ProductVersion}\nodr.exe: {(OdrTool.Odr ?? "غير موجود!")}\nodr-corrupt.exe: {(OdrTool.Corrupt ?? "—")}\nالتطبيق: {OdrTool.AppDir}";
            Row("الإصدار والمحرك", lblVer);

            var links = new FlowLayoutPanel { AutoSize = true };
            var bDocs = Theme.Secondary("📁 مجلد التوثيق"); bDocs.Click += (s, e) => { var d = Path.Combine(OdrTool.AppDir, "docs"); if (Directory.Exists(d)) Process.Start("explorer.exe", OdrTool.Quote(d)); };
            var bSite = Theme.Secondary("🌐 صفحة التحميل والتحديثات"); bSite.Click += (s, e) => { try { Process.Start(new ProcessStartInfo("https://github.com/moain2028/odr-apps/releases/latest") { UseShellExecute = true }); } catch { } };
            var bSupport = Theme.Primary("💬 الدعم: +967770941666"); bSupport.Click += (s, e) => { try { Process.Start(new ProcessStartInfo("https://wa.me/967770941666") { UseShellExecute = true }); } catch { } };
            links.Controls.AddRange(new Control[] { bDocs, bSite, bSupport });
            Row("روابط", links);

            var about = new Label
            {
                AutoSize = true, Margin = new Padding(4, 16, 4, 4), ForeColor = Theme.Muted, MaximumSize = new Size(800, 0),
                Text = "ODR — Oracle Datafile Recovery. يقرأ ملفات أوراكل .dbf مباشرة بلا أوراكل، للقراءة فقط: لا يكتب في ملفاتك أبداً، ويثبت ذلك ببصمة SHA-256 قبل وبعد كل عملية. " +
                       "كل صف يُصنَّف موثَّقاً أو مستنتَجاً أو مرفوضاً — لا تخمين. تطوير المهندس معين العباسي © 2026."
            };
            Row("حول", about);

            Controls.Add(Theme.Card(grid, 4)); Controls.Add(head);
            RefreshLicense();
        }

        public void RefreshLicense()
        {
            var l = License?.Invoke();
            if (l == null || l.State == null) { lblLic.Text = "لم يُتحقق بعد."; lblLic.ForeColor = Theme.Muted; return; }
            var s = l.State;
            lblLic.Text = s.Allowed ? "✔ مفعَّل" + (s.Offline ? " (آخر حالة معروفة — بلا إنترنت)" : "") : "🔒 موقوف: " + s.Message;
            lblLic.ForeColor = s.Allowed ? Theme.Ok : Theme.Bad;
        }
    }
}
