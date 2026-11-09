// LockForm.cs - full-window lock shown when license.json says "active": false (or this device is blocked).
// The main window stays alive behind it (no state lost); the user cannot reach it until unlocked.
using System;
using System.Drawing;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace OdrApp.Remote
{
    public sealed class LockForm : Form
    {
        readonly LicenseService lic;
        readonly Label lblTitle = new Label(), lblMsg = new Label(), lblDevice = new Label(), lblErr = new Label(), lblWait = new Label();
        readonly TextBox txtCode = new TextBox();
        readonly Button btnUnlock = new Button(), btnRecheck = new Button(), btnCopy = new Button(), btnContact = new Button();
        readonly Timer tick = new Timer { Interval = 1000 };

        public LockForm(LicenseService lic, Form owner)
        {
            this.lic = lic;
            Text = "ODR — الترخيص";
            RightToLeft = RightToLeft.Yes; RightToLeftLayout = true;
            FormBorderStyle = FormBorderStyle.None; ShowInTaskbar = false;
            StartPosition = FormStartPosition.Manual;
            BackColor = Color.FromArgb(14, 22, 40);
            Font = new Font("Segoe UI", 11f);
            Bounds = owner.Bounds;
            owner.LocationChanged += (s, e) => Bounds = owner.Bounds;
            owner.SizeChanged += (s, e) => Bounds = owner.Bounds;

            var card = new TableLayoutPanel
            {
                ColumnCount = 1, AutoSize = true, Padding = new Padding(36), BackColor = Color.FromArgb(22, 34, 60),
                Anchor = AnchorStyles.None, MinimumSize = new Size(560, 0)
            };
            lblTitle.Text = "🔒 النسخة موقوفة"; lblTitle.Font = new Font("Segoe UI", 20f, FontStyle.Bold); lblTitle.ForeColor = Color.FromArgb(245, 197, 66); lblTitle.AutoSize = true;
            lblMsg.ForeColor = Color.FromArgb(220, 228, 240); lblMsg.AutoSize = true; lblMsg.MaximumSize = new Size(520, 0); lblMsg.Margin = new Padding(0, 10, 0, 16);
            lblDevice.ForeColor = Color.FromArgb(160, 175, 200); lblDevice.AutoSize = true; lblDevice.Font = new Font("Consolas", 11f); lblDevice.RightToLeft = RightToLeft.No;
            lblDevice.Text = "Device ID: " + DeviceIdentity.Pretty(lic.DeviceId);
            txtCode.Width = 340; txtCode.Font = new Font("Consolas", 14f); txtCode.RightToLeft = RightToLeft.No; txtCode.CharacterCasing = CharacterCasing.Upper;
            txtCode.Margin = new Padding(0, 10, 0, 4);
            var hint = new Label { Text = "كود التفعيل (ODR-XXXX-XXXX-XXXX)", ForeColor = Color.FromArgb(160, 175, 200), AutoSize = true };
            lblErr.ForeColor = Color.FromArgb(255, 120, 120); lblErr.AutoSize = true; lblErr.Margin = new Padding(0, 4, 0, 4);
            lblWait.ForeColor = Color.FromArgb(255, 200, 120); lblWait.AutoSize = true;
            Style(btnUnlock, "🔓 تفعيل", true); Style(btnRecheck, "↻ إعادة التحقق", false); Style(btnCopy, "📋 نسخ معرّف الجهاز", false); Style(btnContact, "💬 تواصل مع المطوّر", false);
            var row = new FlowLayoutPanel { AutoSize = true, FlowDirection = FlowDirection.RightToLeft, Margin = new Padding(0, 12, 0, 0) };
            row.Controls.AddRange(new Control[] { btnUnlock, btnRecheck, btnCopy, btnContact });

            card.Controls.Add(lblTitle); card.Controls.Add(lblMsg); card.Controls.Add(lblDevice); card.Controls.Add(hint);
            card.Controls.Add(txtCode); card.Controls.Add(lblErr); card.Controls.Add(lblWait); card.Controls.Add(row);
            Controls.Add(card);
            Resize += (s, e) => card.Location = new Point((ClientSize.Width - card.Width) / 2, (ClientSize.Height - card.Height) / 2);
            card.SizeChanged += (s, e) => card.Location = new Point((ClientSize.Width - card.Width) / 2, (ClientSize.Height - card.Height) / 2);

            btnUnlock.Click += async (s, e) => await Unlock();
            txtCode.KeyDown += async (s, e) => { if (e.KeyCode == Keys.Enter) { e.SuppressKeyPress = true; await Unlock(); } };
            btnRecheck.Click += async (s, e) => { btnRecheck.Enabled = false; await lic.RefreshAsync(); btnRecheck.Enabled = true; Render(); };
            btnCopy.Click += (s, e) => { try { Clipboard.SetText(DeviceIdentity.Pretty(lic.DeviceId)); } catch { } };
            btnContact.Click += (s, e) =>
            {
                var c = (lic.State?.Contact ?? "").Replace("+", "").Replace(" ", "");
                if (c == "") c = "967770941666";
                try { System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo("https://wa.me/" + c + "?text=" + Uri.EscapeDataString("ODR Device ID: " + DeviceIdentity.Pretty(lic.DeviceId))) { UseShellExecute = true }); } catch { }
            };
            tick.Tick += (s, e) => RenderWait();
            tick.Start();
            lic.Changed += () => { if (IsHandleCreated) BeginInvoke(new Action(Render)); };
            Render();
        }

        static void Style(Button b, string text, bool primary)
        {
            b.Text = text; b.AutoSize = true; b.Height = 38; b.Padding = new Padding(12, 4, 12, 4); b.FlatStyle = FlatStyle.Flat; b.Margin = new Padding(4);
            b.FlatAppearance.BorderSize = primary ? 0 : 1; b.FlatAppearance.BorderColor = Color.FromArgb(90, 110, 150);
            b.BackColor = primary ? Color.FromArgb(245, 197, 66) : Color.FromArgb(30, 45, 78);
            b.ForeColor = primary ? Color.FromArgb(20, 20, 20) : Color.White;
            if (primary) b.Font = new Font("Segoe UI", 11f, FontStyle.Bold);
        }

        void Render()
        {
            var st = lic.State ?? LicenseState.Open();
            string title, msg;
            switch (st.Reason)
            {
                case LockReason.DeviceBlocked: title = "🚫 موقوف على هذا الجهاز"; msg = st.Message; break;
                case LockReason.Revoked: title = "🔒 انتهى الترخيص"; msg = st.Message; break;
                case LockReason.OfflineExpired: title = "📡 يلزم الاتصال بالإنترنت"; msg = st.Message; break;
                default: title = "🔒 النسخة موقوفة"; msg = st.Message != "" ? st.Message : "هذه النسخة موقوفة من المطوّر. أدخل كود التفعيل أو تواصل معنا."; break;
            }
            lblTitle.Text = title; lblMsg.Text = msg;
            bool code = st.AcceptsCode;
            txtCode.Visible = code; btnUnlock.Visible = code;
            foreach (Control c in Controls[0].Controls) if (c is Label l && l.Text.StartsWith("كود التفعيل")) l.Visible = code;
            lblErr.Text = "";
            RenderWait();
        }

        void RenderWait()
        {
            int w = lic.WaitSeconds();
            lblWait.Text = w > 0 ? $"محاولات كثيرة. انتظر {w} ثانية." : "";
            btnUnlock.Enabled = w == 0;
        }

        async Task Unlock()
        {
            if (txtCode.Text.Trim() == "") { lblErr.Text = "أدخل الكود."; return; }
            btnUnlock.Enabled = false; lblErr.Text = "";
            bool ok = await lic.UnlockAsync(txtCode.Text);
            if (!ok) { lblErr.Text = "الكود غير صحيح أو لا يمكن التحقق منه الآن."; txtCode.SelectAll(); txtCode.Focus(); }
            RenderWait();
        }

        protected override void OnFormClosing(FormClosingEventArgs e)
        {
            // Only the service may dismiss us (on unlock). Alt+F4 / X are ignored.
            if (e.CloseReason == CloseReason.UserClosing && !lic.Allowed) { e.Cancel = true; return; }
            tick.Stop();
            base.OnFormClosing(e);
        }
    }
}
