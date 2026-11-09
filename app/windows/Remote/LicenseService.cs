// LicenseService.cs - remote kill switch for the Windows app (port of Qurat Al-Mahalat's license_service.dart).
//
// Remote file: github.com/moain2028/odr-apps/odr/license.json
//   { "active": true, "codeSha256": "<sha256 of the activation code, upper-case, no spaces>",
//     "message": "...", "blockedDevices": ["ABCD1234"], "blockedMessage": "...",
//     "offlineGraceDays": 0, "contact": "+967..." }
//
//   active=true                 -> open (clears any earlier lock)
//   active=false + code hash    -> locked, asks for the code; a correct code is remembered while the hash is unchanged
//   code hash changed           -> everybody who activated with the old code is locked again
//   device in blockedDevices    -> locked on that device only, code does not help
//   file deleted (404)          -> permanent lock, but ONLY if this install has seen the file before (first-run safety)
//   offline / corrupt / 403     -> last known state. NEVER a false lock.
//   offlineGraceDays = N        -> lock after N days without one successful check (reopens on contact)
//
// User data is never touched: the lock only covers the window.
using System;
using System.Collections.Generic;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Threading.Tasks;

namespace OdrApp.Remote
{
    public enum LockReason { None, Locked, DeviceBlocked, Revoked, OfflineExpired }

    public sealed class LicenseState
    {
        public bool Allowed;
        public LockReason Reason = LockReason.None;
        public string Message = "";
        public string Contact = "";
        public bool Offline;
        public static LicenseState Open() => new LicenseState { Allowed = true };
        /// <summary>Can the user type a code? Not after revocation, device block or offline expiry.</summary>
        public bool AcceptsCode => !Allowed && Reason == LockReason.Locked;
    }

    public sealed class LicenseService
    {
        public const string File = "license.json";
        public static readonly TimeSpan RecheckEvery = TimeSpan.FromMinutes(10);

        // settings keys
        const string KSeen = "lic_seen", KBlocked = "lic_blocked", KReason = "lic_reason", KMessage = "lic_message",
            KContact = "lic_contact", KCodeHash = "lic_code_hash", KActivated = "lic_activated_hash",
            KLastOk = "lic_last_ok", KGraceDays = "lic_grace_days", KFails = "lic_fails", KNextTry = "lic_next_try";

        /// <summary>Clock, replaceable in tests.</summary>
        public static Func<DateTime> Now = () => DateTime.UtcNow;

        readonly Settings prefs;
        LicenseState state;
        DateTime? lastCheck;
        Task<LicenseState> inflight;

        public string DeviceId { get; }
        public LicenseState State => state;
        public bool Allowed => state == null || state.Allowed;
        public event Action Changed;

        public LicenseService(Settings prefs)
        {
            this.prefs = prefs;
            DeviceId = DeviceIdentity.Id();
            state = FromCache(prefs);
        }

        /// <summary>Non-blocking: cached state is visible immediately, then refreshed.</summary>
        public Task<LicenseState> RefreshAsync()
        {
            if (inflight != null) return inflight;
            inflight = RefreshCore().ContinueWith(t => { inflight = null; return t.Result; });
            return inflight;
        }

        async Task<LicenseState> RefreshCore()
        {
            try
            {
                var remote = await RemoteSource.FetchAsync(File);
                var st = Apply(prefs, remote, DeviceId);
                lastCheck = Now();
                state = st;
                Changed?.Invoke();
                return st;
            }
            catch { return state ?? LicenseState.Open(); }
        }

        public Task OnResumedAsync()
        {
            if (lastCheck.HasValue && Now() - lastCheck.Value < RecheckEvery) return Task.CompletedTask;
            return RefreshAsync();
        }

        /// <summary>Seconds the user must wait before another attempt (brute-force slowdown), 0 if none.</summary>
        public int WaitSeconds()
        {
            var next = prefs.GetLong(KNextTry, 0);
            var now = Now().Ticks;
            return next > now ? (int)Math.Ceiling(TimeSpan.FromTicks(next - now).TotalSeconds) : 0;
        }

        public async Task<bool> UnlockAsync(string code)
        {
            if (WaitSeconds() > 0) return false;
            var ok = await ActivateAsync(prefs, code, DeviceId);
            if (ok)
            {
                prefs.Set(KFails, 0); prefs.Set(KNextTry, 0L);
                state = LicenseState.Open();
                Changed?.Invoke();
            }
            else
            {
                // after 5 failures: 30 s, then 60, 120 ... (doubling)
                int fails = prefs.GetInt(KFails, 0) + 1;
                prefs.Set(KFails, fails);
                if (fails >= 5)
                {
                    var wait = TimeSpan.FromSeconds(30 * Math.Pow(2, Math.Min(fails - 5, 8)));
                    prefs.Set(KNextTry, (Now() + wait).Ticks);
                }
            }
            prefs.Save();
            return ok;
        }

        // ------------------------------------------------------------------ pure logic (testable)

        public static string NormalizeCode(string s)
        {
            const string ar = "٠١٢٣٤٥٦٧٨٩";
            var b = new StringBuilder();
            foreach (var c in s ?? "")
            {
                int i = ar.IndexOf(c);
                if (i >= 0) b.Append((char)('0' + i));
                else if (!char.IsWhiteSpace(c)) b.Append(char.ToUpperInvariant(c));
            }
            return b.ToString();
        }

        public static string HashCode256(string code)
        {
            using (var sha = SHA256.Create())
                return BitConverter.ToString(sha.ComputeHash(Encoding.UTF8.GetBytes(NormalizeCode(code)))).Replace("-", "").ToLowerInvariant();
        }

        /// <summary>Reference hash from the file; "" when no code is accepted.</summary>
        public static string CodeHashOf(Dictionary<string, object> d)
        {
            var h = Str(d, "codeSha256").Trim().ToLowerInvariant();
            if (h.Length == 64 && h.All(c => "0123456789abcdef".IndexOf(c) >= 0)) return h;
            var plain = Str(d, "code").Trim();
            return plain == "" ? "" : HashCode256(plain);
        }

        public static bool IsDeviceBlocked(Dictionary<string, object> d, string deviceId)
        {
            var id = DeviceIdentity.Normalize(deviceId);
            if (id == "") return false;
            object list;
            if (!d.TryGetValue("blockedDevices", out list) || !(list is object[] arr)) return false;
            return arr.Any(e => DeviceIdentity.Normalize(Convert.ToString(e)) == id);
        }

        public static LicenseState Apply(Settings prefs, RemoteResult remote, string deviceId)
        {
            if (remote.IsOffline) return OfflineState(prefs);

            if (remote.IsNotFound)
            {
                if (!prefs.GetBool(KSeen, false)) return OfflineState(prefs);
                const string msg = "انتهى ترخيص هذه النسخة. يرجى التواصل مع المطوّر.";
                SaveBlocked(prefs, LockReason.Revoked, msg);
                prefs.Set(KCodeHash, ""); prefs.Save();
                return new LicenseState { Allowed = false, Reason = LockReason.Revoked, Message = msg, Contact = prefs.Get(KContact, "") };
            }

            var d = remote.Data;
            var message = Str(d, "message").Trim();
            var contact = Str(d, "contact").Trim();
            var codeHash = CodeHashOf(d);
            int grace = 0;
            object g;
            if (d.TryGetValue("offlineGraceDays", out g)) int.TryParse(Convert.ToString(g), out grace);

            prefs.Set(KSeen, true);
            prefs.Set(KCodeHash, codeHash);
            prefs.Set(KContact, contact);
            prefs.Set(KLastOk, Now().Ticks);
            prefs.Set(KGraceDays, grace < 0 ? 0 : grace);

            LicenseState result;
            if (IsDeviceBlocked(d, deviceId))
            {
                var m = Str(d, "blockedMessage").Trim();
                var text = m != "" ? m : (message != "" ? message : "تم إيقاف التطبيق على هذا الجهاز.");
                SaveBlocked(prefs, LockReason.DeviceBlocked, text);
                result = new LicenseState { Allowed = false, Reason = LockReason.DeviceBlocked, Message = text, Contact = contact };
            }
            else if (Bool(d, "active"))
            {
                prefs.Set(KBlocked, false); prefs.Set(KReason, LockReason.None.ToString()); prefs.Set(KMessage, message);
                result = new LicenseState { Allowed = true, Message = message, Contact = contact };
            }
            else if (codeHash != "" && prefs.Get(KActivated, "") == codeHash)
            {
                prefs.Set(KBlocked, false); prefs.Set(KReason, LockReason.None.ToString()); prefs.Set(KMessage, message);
                result = new LicenseState { Allowed = true, Message = message, Contact = contact };
            }
            else
            {
                SaveBlocked(prefs, LockReason.Locked, message);
                result = new LicenseState
                {
                    Allowed = false,
                    Reason = codeHash == "" ? LockReason.Revoked : LockReason.Locked, // no code in the file = no way to unlock by code
                    Message = message, Contact = contact
                };
            }
            prefs.Save();
            return result;
        }

        static void SaveBlocked(Settings p, LockReason r, string msg)
        {
            p.Set(KBlocked, true); p.Set(KReason, r.ToString()); p.Set(KMessage, msg);
        }

        static LicenseState OfflineState(Settings prefs)
        {
            var cached = FromCache(prefs, offline: true);
            if (!cached.Allowed) return cached;
            int grace = prefs.GetInt(KGraceDays, 0);
            long lastOk = prefs.GetLong(KLastOk, 0);
            if (grace > 0 && lastOk > 0 && Now() - new DateTime(lastOk, DateTimeKind.Utc) > TimeSpan.FromDays(grace))
                return new LicenseState
                {
                    Allowed = false, Reason = LockReason.OfflineExpired, Offline = true,
                    Message = $"مضى أكثر من {grace} يوم دون تحقق. اتصل بالإنترنت ثم اضغط «إعادة التحقق».",
                    Contact = prefs.Get(KContact, "")
                };
            return cached;
        }

        public static LicenseState FromCache(Settings prefs, bool offline = false)
        {
            bool blocked = prefs.GetBool(KBlocked, false);
            LockReason reason;
            if (!Enum.TryParse(prefs.Get(KReason, ""), out reason)) reason = blocked ? LockReason.Locked : LockReason.None;
            var r = blocked ? reason : LockReason.None;
            if (r == LockReason.Locked && prefs.Get(KCodeHash, "") == "") r = LockReason.Revoked;
            return new LicenseState { Allowed = !blocked, Reason = r, Message = prefs.Get(KMessage, ""), Contact = prefs.Get(KContact, ""), Offline = offline };
        }

        /// <summary>Checks the code against the file (or the last cached hash when offline).</summary>
        public static async Task<bool> ActivateAsync(Settings prefs, string input, string deviceId)
        {
            if (NormalizeCode(input) == "") return false;
            var remote = await RemoteSource.FetchAsync(File);
            if (remote.IsNotFound) return false;
            if (remote.IsData)
            {
                if (IsDeviceBlocked(remote.Data, deviceId)) return false;
                prefs.Set(KCodeHash, CodeHashOf(remote.Data));
            }
            else if (prefs.Get(KReason, "") == LockReason.DeviceBlocked.ToString()) return false;
            var codeHash = prefs.Get(KCodeHash, "");
            if (codeHash == "" || HashCode256(input) != codeHash) return false;
            prefs.Set(KActivated, codeHash); prefs.Set(KBlocked, false); prefs.Set(KReason, LockReason.None.ToString());
            prefs.Save();
            return true;
        }

        static string Str(Dictionary<string, object> d, string k) { object v; return d.TryGetValue(k, out v) && v != null ? Convert.ToString(v) : ""; }
        static bool Bool(Dictionary<string, object> d, string k) { object v; return d.TryGetValue(k, out v) && v is bool b && b; }
    }
}
