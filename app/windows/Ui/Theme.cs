// Theme.cs - the ODR look: deep navy + gold, Segoe UI, flat controls, RTL. One place for every colour.
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Windows.Forms;

namespace OdrApp.Ui
{
    public static class Theme
    {
        public static readonly Color Bg = Color.FromArgb(243, 245, 250);        // page background
        public static readonly Color CardBg = Color.White;
        public static readonly Color CardBorder = Color.FromArgb(222, 228, 240);
        public static readonly Color Navy = Color.FromArgb(14, 30, 64);          // header / sidebar
        public static readonly Color NavyLight = Color.FromArgb(24, 46, 92);
        public static readonly Color Gold = Color.FromArgb(214, 164, 46);
        public static readonly Color GoldLight = Color.FromArgb(245, 197, 66);
        public static readonly Color Text = Color.FromArgb(28, 34, 48);
        public static readonly Color Muted = Color.FromArgb(104, 116, 140);
        public static readonly Color Ok = Color.FromArgb(34, 140, 80);
        public static readonly Color Warn = Color.FromArgb(214, 134, 20);
        public static readonly Color Bad = Color.FromArgb(196, 48, 48);
        public static readonly Color OkBg = Color.FromArgb(232, 247, 236);
        public static readonly Color WarnBg = Color.FromArgb(255, 246, 225);
        public static readonly Color BadBg = Color.FromArgb(253, 232, 232);
        public static readonly Color InfoBg = Color.FromArgb(232, 240, 255);

        public static readonly Font Base = new Font("Segoe UI", 10f);
        public static readonly Font Bold = new Font("Segoe UI", 10f, FontStyle.Bold);
        public static readonly Font H1 = new Font("Segoe UI", 18f, FontStyle.Bold);
        public static readonly Font H2 = new Font("Segoe UI", 13f, FontStyle.Bold);
        public static readonly Font Small = new Font("Segoe UI", 9f);
        public static readonly Font Mono = new Font("Consolas", 10f);
        public static readonly Font Big = new Font("Segoe UI", 34f, FontStyle.Bold);

        public static Color BandColor(string band)
        {
            switch ((band ?? "").ToLowerInvariant())
            {
                case "excellent": return Ok;
                case "good": return Color.FromArgb(70, 150, 60);
                case "partial": return Warn;
                case "poor": return Color.FromArgb(214, 90, 20);
                default: return Bad;
            }
        }

        public static Color SeverityBg(string sev)
        {
            switch ((sev ?? "").ToLowerInvariant())
            {
                case "critical": return BadBg;
                case "high": return Color.FromArgb(255, 236, 224);
                case "medium": return WarnBg;
                case "low": return Color.FromArgb(250, 250, 232);
                default: return InfoBg;
            }
        }

        public static string SeverityAr(string sev)
        {
            switch ((sev ?? "").ToLowerInvariant())
            {
                case "critical": return "حرج";
                case "high": return "عالٍ";
                case "medium": return "متوسط";
                case "low": return "منخفض";
                default: return "معلومة";
            }
        }

        public static string BandAr(string band)
        {
            switch ((band ?? "").ToLowerInvariant())
            {
                case "excellent": return "ممتاز";
                case "good": return "جيد";
                case "partial": return "جزئي";
                case "poor": return "ضعيف";
                case "lost": return "ضائع";
                default: return band ?? "";
            }
        }

        // ------------------------------------------------------------------ control factories
        public static Button Primary(string text, int w = 0)
        {
            var b = Flat(text, w); b.BackColor = Gold; b.ForeColor = Color.FromArgb(24, 24, 24); b.Font = Bold;
            b.FlatAppearance.MouseOverBackColor = GoldLight; return b;
        }

        public static Button Secondary(string text, int w = 0)
        {
            var b = Flat(text, w); b.BackColor = Color.White; b.ForeColor = Navy; b.FlatAppearance.BorderSize = 1; b.FlatAppearance.BorderColor = CardBorder;
            b.FlatAppearance.MouseOverBackColor = Color.FromArgb(236, 240, 250); return b;
        }

        public static Button Danger(string text, int w = 0)
        {
            var b = Flat(text, w); b.BackColor = Color.White; b.ForeColor = Bad; b.FlatAppearance.BorderSize = 1; b.FlatAppearance.BorderColor = Color.FromArgb(240, 200, 200); return b;
        }

        static Button Flat(string text, int w)
        {
            var b = new Button { Text = text, Height = 36, FlatStyle = FlatStyle.Flat, Font = Base, Cursor = Cursors.Hand, Margin = new Padding(4), Padding = new Padding(10, 0, 10, 0) };
            if (w > 0) b.Width = w; else b.AutoSize = true;
            b.FlatAppearance.BorderSize = 0;
            return b;
        }

        /// <summary>The ℹ️ button that opens the page explanation.</summary>
        public static Button Info(string tip = "ما هذه الصفحة؟")
        {
            var b = new Button
            {
                Text = "ℹ", Width = 32, Height = 32, FlatStyle = FlatStyle.Flat, Font = new Font("Segoe UI", 13f, FontStyle.Bold),
                BackColor = Color.White, ForeColor = Navy, Cursor = Cursors.Hand, Margin = new Padding(4), TextAlign = ContentAlignment.MiddleCenter
            };
            b.FlatAppearance.BorderSize = 1; b.FlatAppearance.BorderColor = Gold;
            new ToolTip().SetToolTip(b, tip);
            return b;
        }

        public static Panel Card(Control content, int pad = 12)
        {
            var p = new BorderPanel { Dock = DockStyle.Fill, Padding = new Padding(pad), BackColor = CardBg };
            content.Dock = DockStyle.Fill;
            p.Controls.Add(content);
            return p;
        }

        public static Label Title(string text) => new Label { Text = text, Font = H2, ForeColor = Navy, AutoSize = true, Margin = new Padding(4, 6, 4, 6) };
        public static Label MutedLabel(string text) => new Label { Text = text, Font = Small, ForeColor = Muted, AutoSize = true, Margin = new Padding(4) };

        public static ListView List(params Tuple<string, int>[] columns)
        {
            var lv = new ListView
            {
                View = View.Details, FullRowSelect = true, GridLines = false, Dock = DockStyle.Fill, RightToLeftLayout = true,
                BorderStyle = BorderStyle.None, Font = Base, BackColor = Color.White, HideSelection = false
            };
            foreach (var c in columns) lv.Columns.Add(c.Item1, c.Item2);
            return lv;
        }

        public static Tuple<string, int> Col(string name, int w) => Tuple.Create(name, w);
    }

    /// <summary>White card with a 1px border and rounded look.</summary>
    public sealed class BorderPanel : Panel
    {
        public BorderPanel() { DoubleBuffered = true; }
        protected override void OnPaint(PaintEventArgs e)
        {
            base.OnPaint(e);
            using (var pen = new Pen(Theme.CardBorder))
                e.Graphics.DrawRectangle(pen, 0, 0, Width - 1, Height - 1);
        }
    }

    /// <summary>Score ring 0–100 with band colour.</summary>
    public sealed class ScoreGauge : Control
    {
        int score = -1; string band = "";
        public ScoreGauge() { DoubleBuffered = true; Size = new Size(150, 150); }
        public void Set(int s, string b) { score = s; band = b; Invalidate(); }
        protected override void OnPaint(PaintEventArgs e)
        {
            var g = e.Graphics; g.SmoothingMode = SmoothingMode.AntiAlias; g.Clear(BackColor);
            var r = new Rectangle(10, 10, Width - 20, Height - 20);
            using (var back = new Pen(Color.FromArgb(230, 234, 242), 12)) g.DrawArc(back, r, 0, 360);
            if (score >= 0)
            {
                using (var p = new Pen(Theme.BandColor(band), 12) { StartCap = LineCap.Round, EndCap = LineCap.Round })
                    g.DrawArc(p, r, -90, 360f * score / 100f);
                TextRenderer.DrawText(g, score.ToString(), Theme.Big, new Rectangle(0, 0, Width, Height - 22), Theme.Text, TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter);
                TextRenderer.DrawText(g, Theme.BandAr(band), Theme.Bold, new Rectangle(0, Height - 48, Width, 30), Theme.BandColor(band), TextFormatFlags.HorizontalCenter);
            }
            else TextRenderer.DrawText(g, "—", Theme.Big, new Rectangle(0, 0, Width, Height), Theme.Muted, TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter);
        }
    }
}
