// HomePage.cs - four big doors + what the program promises. Drop a file anywhere here to diagnose it.
using System;
using System.Drawing;
using System.Windows.Forms;

namespace OdrApp.Ui
{
    public sealed class HomePage : UserControl
    {
        public event Action<string> Go;        // page key
        public event Action<string> DropFile;  // path dropped

        public HomePage()
        {
            Dock = DockStyle.Fill; BackColor = Theme.Bg; RightToLeft = RightToLeft.Yes; Font = Theme.Base; Padding = new Padding(16); AllowDrop = true;
            var head = new FlowLayoutPanel { AutoSize = true, Dock = DockStyle.Top, Padding = new Padding(0, 0, 0, 10) };
            var info = Theme.Info(); info.Click += (s, e) => Help.Show(FindForm(), "الرئيسية", Help.Home);
            head.Controls.Add(Theme.Title("مرحباً — ماذا تريد أن تفعل؟")); head.Controls.Add(info);
            head.Controls.Add(Theme.MutedLabel("يمكنك أيضاً سحب ملف .dbf وإفلاته هنا لتشخيصه فوراً."));

            var grid = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 2, RowCount = 2, Padding = new Padding(6) };
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 50)); grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 50));
            grid.RowStyles.Add(new RowStyle(SizeType.Percent, 50)); grid.RowStyles.Add(new RowStyle(SizeType.Percent, 50));
            grid.Controls.Add(Door("📦", "الاسترجاع", "عندك ملفات .dbf وتريد الجداول؟ اقرأ القاموس من SYSTEM، حدّد الجداول، واستخرجها إلى موثَّق / مستنتج / مرفوض.", "recover"), 0, 0);
            grid.Controls.Add(Door("🩺", "التشخيص", "ملف مشكوك فيه؟ درجة 0–100، ماذا حدث له ولماذا (فدية؟ قرص؟ كهرباء؟)، وما الذي يمكن إنقاذه، مع تقرير HTML للعميل.", "diagnose"), 1, 0);
            grid.Controls.Add(Door("⚖️", "الفرز", "مجلد كامل بعد حادثة؟ رتّب كل الملفات من الأسوأ إلى الأفضل خلال دقائق وابدأ بالأهم.", "triage"), 0, 1);
            grid.Controls.Add(Door("🔍", "البصمة", "ضاع ملف SYSTEM وظهرت الجداول كأرقام؟ اقترح أسماءها من قاعدة سليمة لنفس المنتج.", "fingerprint"), 1, 1);

            var promise = new Label
            {
                Dock = DockStyle.Bottom, Height = 54, TextAlign = ContentAlignment.MiddleCenter, ForeColor = Theme.Muted, Font = Theme.Small,
                Text = "للقراءة فقط — لا يكتب في ملفاتك أبداً ويثبت ذلك ببصمة SHA-256 قبل وبعد.   ·   لا تخمين: كل صف موثَّق أو مستنتج أو مرفوض بسبب.   ·   يعمل بلا أوراكل وبلا إنترنت."
            };
            Controls.Add(grid); Controls.Add(promise); Controls.Add(head);

            DragEnter += (s, e) => { if (e.Data.GetDataPresent(DataFormats.FileDrop)) e.Effect = DragDropEffects.Copy; };
            DragDrop += (s, e) => { var f = (string[])e.Data.GetData(DataFormats.FileDrop); if (f.Length > 0) DropFile?.Invoke(f[0]); };
        }

        Control Door(string icon, string title, string text, string key)
        {
            var p = new BorderPanel { Dock = DockStyle.Fill, BackColor = Color.White, Margin = new Padding(10), Padding = new Padding(18), Cursor = Cursors.Hand };
            var ic = new Label { Text = icon, Font = new Font("Segoe UI Emoji", 30f), AutoSize = true, Location = new Point(18, 14) };
            var t = new Label { Text = title, Font = Theme.H1, ForeColor = Theme.Navy, AutoSize = true, Location = new Point(18, 70) };
            var d = new Label { Text = text, Font = new Font("Segoe UI", 11f), ForeColor = Theme.Text, AutoSize = false, Location = new Point(18, 116), Size = new Size(380, 90) };
            var go = Theme.Primary("ابدأ ←", 110); go.Location = new Point(18, 210);
            p.Controls.AddRange(new Control[] { ic, t, d, go });
            p.Resize += (s, e) => { d.Width = p.ClientSize.Width - 40; go.Top = p.ClientSize.Height - go.Height - 18; };
            EventHandler click = (s, e) => Go?.Invoke(key);
            p.Click += click; ic.Click += click; t.Click += click; d.Click += click; go.Click += click;
            p.MouseEnter += (s, e) => p.BackColor = Color.FromArgb(250, 251, 255); p.MouseLeave += (s, e) => p.BackColor = Color.White;
            return p;
        }
    }
}
