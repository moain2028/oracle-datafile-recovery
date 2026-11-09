// LicenseTests.cs - the remote-lock state table, exercised without network (fake RemoteResult).
// Run by ODR.exe --selftest-license (CI) — no test framework needed on .NET Framework 4.8.
using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using OdrApp.Remote;

namespace OdrApp.Tests
{
    public static class LicenseTests
    {
        static int pass, fail;
        static readonly List<string> log = new List<string>();
        static void Check(bool ok, string what) { if (ok) pass++; else fail++; log.Add((ok ? "PASS  " : "FAIL  ") + what); }

        static RemoteResult Data(string json) => RemoteSource.Parse(json);

        public static int Run(out string report)
        {
            pass = fail = 0; log.Clear();
            var t0 = new DateTime(2026, 11, 9, 12, 0, 0, DateTimeKind.Utc);
            LicenseService.Now = () => t0;
            DeviceIdentity.MachineSeed = () => "test-machine";
            var dev = DeviceIdentity.Id();
            var hash = LicenseService.HashCode256("odr-abcd-efgh-2345");

            Check(LicenseService.NormalizeCode(" odr-abcd ٢٣٤٥ ") == "ODR-ABCD2345", "normalize: spaces, case, Arabic digits (dashes are part of the code)");
            Check(LicenseService.HashCode256("ODR-ABCD-EFGH-2345") == hash, "hash is case/space independent");
            Check(dev.Length == 8 && DeviceIdentity.Normalize(DeviceIdentity.Pretty(dev)) == dev, "device id 8 hex, pretty round-trips");

            // 1. offline on first run: open (never a false lock)
            var p = new Settings();
            var s = LicenseService.Apply(p, RemoteResult.OfflineResult, dev);
            Check(s.Allowed && s.Offline, "offline first run -> open");

            // 2. 404 on first run: open; 404 after having seen the file: revoked
            s = LicenseService.Apply(p, RemoteResult.NotFoundResult, dev);
            Check(s.Allowed, "404 before ever seeing the file -> open");
            s = LicenseService.Apply(p, Data("{\"active\":true}"), dev);
            Check(s.Allowed && !s.Offline, "active:true -> open");
            s = LicenseService.Apply(p, RemoteResult.NotFoundResult, dev);
            Check(!s.Allowed && s.Reason == LockReason.Revoked && !s.AcceptsCode, "404 after seen -> revoked, no code box");
            s = LicenseService.Apply(p, RemoteResult.OfflineResult, dev);
            Check(!s.Allowed && s.Reason == LockReason.Revoked, "offline keeps revoked");

            // 3. lock with code; wrong/right code; code rotation
            p = new Settings();
            var locked = "{\"active\":false,\"codeSha256\":\"" + hash + "\",\"message\":\"stop\",\"contact\":\"+967\"}";
            s = LicenseService.Apply(p, Data(locked), dev);
            Check(!s.Allowed && s.Reason == LockReason.Locked && s.AcceptsCode && s.Message == "stop" && s.Contact == "+967", "active:false + code -> locked, accepts code");
            RemoteSource.Fetcher = url => Task.FromResult(Tuple.Create(200, locked)); // raw path returns the json
            Check(!LicenseService.ActivateAsync(p, "ODR-WRONG", dev).Result, "wrong code rejected");
            Check(LicenseService.ActivateAsync(p, " odr-abcd - efgh-٢٣٤٥ ", dev).Result, "right code (lower-case, spaces, Arabic digits) accepted");
            Check(!LicenseService.ActivateAsync(p, "odr abcd efgh 2345", dev).Result, "code without its dashes is a different code");
            s = LicenseService.Apply(p, Data(locked), dev);
            Check(s.Allowed, "activated code keeps it open while the hash is unchanged");
            var rotated = "{\"active\":false,\"codeSha256\":\"" + LicenseService.HashCode256("ODR-NEW1-NEW2-NEW3") + "\"}";
            s = LicenseService.Apply(p, Data(rotated), dev);
            Check(!s.Allowed && s.AcceptsCode, "rotated code locks again");
            s = LicenseService.Apply(p, RemoteResult.OfflineResult, dev);
            Check(!s.Allowed && s.Reason == LockReason.Locked, "offline keeps the lock");

            // 4. active:false without any code -> revoked (no code box)
            s = LicenseService.Apply(new Settings(), Data("{\"active\":false}"), dev);
            Check(!s.Allowed && s.Reason == LockReason.Revoked && !s.AcceptsCode, "active:false without code -> no code box");

            // 5. device block beats active:true and beats the code
            p = new Settings();
            var blocked = "{\"active\":true,\"codeSha256\":\"" + hash + "\",\"blockedDevices\":[\"" + DeviceIdentity.Pretty(dev).ToLower() + "\"],\"blockedMessage\":\"bye\"}";
            s = LicenseService.Apply(p, Data(blocked), dev);
            Check(!s.Allowed && s.Reason == LockReason.DeviceBlocked && s.Message == "bye" && !s.AcceptsCode, "blocked device (pretty/lower-case form) -> locked");
            RemoteSource.Fetcher = url => Task.FromResult(Tuple.Create(200, blocked));
            Check(!LicenseService.ActivateAsync(p, "ODR-ABCD-EFGH-2345", dev).Result, "code cannot unlock a blocked device");
            s = LicenseService.Apply(p, Data("{\"active\":true,\"blockedDevices\":[]}"), dev);
            Check(s.Allowed, "unblock -> open");

            // 6. offline grace
            p = new Settings();
            s = LicenseService.Apply(p, Data("{\"active\":true,\"offlineGraceDays\":7}"), dev);
            Check(s.Allowed, "grace set, online -> open");
            LicenseService.Now = () => t0.AddDays(6);
            Check(LicenseService.Apply(p, RemoteResult.OfflineResult, dev).Allowed, "6 days offline -> still open");
            LicenseService.Now = () => t0.AddDays(8);
            s = LicenseService.Apply(p, RemoteResult.OfflineResult, dev);
            Check(!s.Allowed && s.Reason == LockReason.OfflineExpired, "8 days offline -> expired");
            s = LicenseService.Apply(p, Data("{\"active\":true,\"offlineGraceDays\":7}"), dev);
            Check(s.Allowed, "back online -> reopens");

            // 7. corrupt json = offline, not a lock
            Check(RemoteSource.Parse("{not json") == null, "corrupt json parses to null (treated as offline)");
            Check(RemoteSource.Parse("[1,2]") == null, "non-object json is not data");

            // 8. brute-force slowdown
            p = new Settings();
            LicenseService.Now = () => t0;
            RemoteSource.Fetcher = url => Task.FromResult(Tuple.Create(200, locked));
            var svc = new LicenseService(p);
            for (int i = 0; i < 5; i++) svc.UnlockAsync("ODR-NOPE").Wait();
            Check(svc.WaitSeconds() >= 29, "5 failures -> ~30 s wait (" + svc.WaitSeconds() + ")");
            Check(!svc.UnlockAsync("ODR-ABCD-EFGH-2345").Result, "right code is refused during the wait");
            LicenseService.Now = () => t0.AddSeconds(31);
            Check(svc.UnlockAsync("ODR-ABCD-EFGH-2345").Result, "right code accepted after the wait");

            RemoteSource.Fetcher = null;
            LicenseService.Now = () => DateTime.UtcNow;
            DeviceIdentity.MachineSeed = null;
            log.Add($"RESULT: {pass} passed, {fail} failed");
            report = string.Join(Environment.NewLine, log);
            return fail == 0 && pass > 0 ? 0 : 1;
        }
    }
}
