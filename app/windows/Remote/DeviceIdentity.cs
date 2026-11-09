// DeviceIdentity.cs - stable per-machine id for blockedDevices. 8 hex digits of
// SHA-256("odr-device:" + MachineGuid). Never sent anywhere: the user reads it in the app and
// tells the developer; the developer puts it in license.json.
using System;
using System.Security.Cryptography;
using System.Text;
using Microsoft.Win32;

namespace OdrApp.Remote
{
    public static class DeviceIdentity
    {
        /// <summary>Replaceable in tests.</summary>
        public static Func<string> MachineSeed = DefaultSeed;

        static string DefaultSeed()
        {
            try
            {
                using (var k = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry64).OpenSubKey(@"SOFTWARE\Microsoft\Cryptography"))
                {
                    var g = k?.GetValue("MachineGuid") as string;
                    if (!string.IsNullOrEmpty(g)) return g;
                }
            }
            catch { }
            return Environment.MachineName + "|" + Environment.UserName;
        }

        public static string Id()
        {
            using (var sha = SHA256.Create())
            {
                var h = sha.ComputeHash(Encoding.UTF8.GetBytes("odr-device:" + (MachineSeed ?? DefaultSeed)()));
                return BitConverter.ToString(h, 0, 4).Replace("-", "").ToUpperInvariant();
            }
        }

        /// <summary>Pretty form for humans: ABCD-1234.</summary>
        public static string Pretty(string id) => id.Length == 8 ? id.Substring(0, 4) + "-" + id.Substring(4) : id;

        /// <summary>Upper-case hex only, so "abcd-1234", "ABCD 1234" and "ABCD1234" compare equal.</summary>
        public static string Normalize(string s)
        {
            var b = new StringBuilder();
            foreach (var c in (s ?? "").ToUpperInvariant()) if (Uri.IsHexDigit(c)) b.Append(c);
            return b.ToString();
        }
    }
}
